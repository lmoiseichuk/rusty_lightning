//! Lightning-detector terminal — bring-up.
//!
//! Build order steps 1–3 (§10): prove the toolchain and console, find who is on
//! the I2C bus, then bring the AS3935 up and decode its interrupts.

mod as3935;
mod battery;
mod clock;
mod commands;
mod console;
mod display;
mod defence;
mod history;
mod i2c_scan;
mod log;
mod power;
mod session;
mod settings;
mod storage;
mod strike;
mod system;
mod ui;

use std::num::NonZeroU32;

use esp_idf_hal::delay::{FreeRtos, TickType};
use esp_idf_hal::gpio::{InterruptType, PinDriver, Pull};
use esp_idf_hal::i2c::{I2cConfig, I2cDriver};
use esp_idf_hal::peripherals::Peripherals;
use esp_idf_hal::task::notification::Notification;
use esp_idf_hal::units::Hertz;

use as3935::{As3935, Location};
use session::{collect, report, toggle_location, tune, Batch, Drawn, Totals};

/// I2C bus speed. **200 kHz: off the sensor's passband, and half of what broke
/// it.**
///
/// 500 kHz is not a harmonic of 200 — 500/200 is 2.5 — so this keeps the
/// harmonic away from the antenna while asking only 2× the rate that is known to
/// work, rather than the 4× that is known not to.
///
/// The rest of this comment is the record of how that was established.
///
/// ## The device was listening to its own bus
///
/// The AS3935 receives at 500 kHz with a Q worth tuning to ±3.5 %, and **the
/// fifth harmonic of 100 kHz is exactly 500 kHz** — carried on lines that run to
/// the sensor's own package. The bus rate was not a neutral choice here; it put
/// an interferer precisely on the passband.
///
/// **Measured, at maximum sensitivity, same board and same afternoon:**
///
/// | bus | `nf 0, wdth 0, srej 0` |
/// |---|---|
/// | 100 kHz | 8–10 `NoiseTooHigh` per second, continuously |
/// | 200 kHz | **none at all** |
///
/// So the "ambient noise floor" the chip kept reporting was self-inflicted, and
/// every configuration that looked quiet at `wdth 2` was quiet because the gate
/// was rejecting it — along with anything else weak enough to need that gate
/// open, which is what a distant strike is.
///
/// ## Why 200 and not 400
///
/// 400 was tried first, being what the MicroPython reference used. It fails
/// outright — the boot scan finds the gauge and nothing else:
///
/// ```text
/// scan: 1 device(s)
///       0x36  MAX17048 fuel gauge
/// FATAL: no AS3935 answered a reset at 0x01/0x02/0x03
/// ```
///
/// Note *which* part dropped out. The gauge kept answering on the same wires, so
/// this is not simply "too fast for the wiring" — the two devices differ in what
/// they tolerate and the AS3935 is the one that stops, on short well-soldered
/// leads under 10 cm. Whether that is its position on the chain or something in
/// the Gravity board's front end is not established.
///
/// 200 kHz needs neither answer: 500 / 200 is 2.5, so no harmonic lands on the
/// passband, and it asks only 2× the rate already known to work rather than the
/// 4× known not to. Lower rates would serve equally on the harmonic argument —
/// 500/80 and 500/40 are not integers either — but there is nothing to buy with
/// the extra slowness.
const I2C_HZ: u32 = 200_000;

/// Where the sensor is when nothing has ever been stored (§4.1).
///
/// The live value comes from NVS and is toggled by the BOOT button; this is
/// only what a virgin device starts as.
const DEFAULT_LOCATION: Location = Location::Indoor;

/// Notification **bits**, so one wait serves two sources.
///
/// ⚠ **Bits, not values.** `Notification` notifies with
/// `eNotifyAction_eSetBits`, so two notifications arriving before the waiter
/// runs are OR'd into one word. Testing `value == NOTIFY_BUTTON` therefore
/// works right up until a strike and a press land in the same window, at which
/// point the value is `3`, matches neither arm, and the press is silently lost.
/// That is exactly what a button that "sometimes does nothing" looks like.
///
/// So both are tested as bits and both are handled — a window can legitimately
/// contain both events.
const NOTIFY_STRIKE: u32 = 1;
const NOTIFY_BUTTON: u32 = 2;

/// How long to ignore further button edges after one is accepted.
///
/// A tactile switch bounces for a few milliseconds; 300 ms also stops a
/// deliberate double-press from toggling twice and landing back where it
/// started, which would look like the button not working at all.
///
/// Measured against a real clock rather than decremented per batch, which was
/// the earlier version's mistake: the batch is 1000 ms, so a 300 ms blanking
/// counter was clear again by the next window and blanked nothing at all.
const BUTTON_DEBOUNCE_MS: u32 = 300;

/// How long the button must be held to count as a press.
///
/// ⚠ **This is not debounce. It is telling a person from a USB host.**
///
/// GPIO9 is the BOOT strap, and the ESP32-C3 maps the host's CDC **DTR** line
/// onto it. So every time `espflash`, `esptool` or a serial monitor connects, it
/// asserts DTR, GPIO9 goes low, and the firmware sees a falling edge that is
/// electrically indistinguishable from a fingertip — because it *is* one. That
/// is the whole explanation for the "phantom presses": they were our own
/// tooling, firing every time anyone opened the port.
///
/// It also rules out the obvious fixes. Sampling the level harder does not help,
/// because the pin really is held down. Moving the button to another pin is not
/// available either — GPIO9 carries the only button on the board.
///
/// What does separate them is **duration**. A flashing tool pulses DTR for a few
/// hundred milliseconds; a person pressing a button holds it far longer without
/// trying. 1.5 s sits well clear of the first and is unnoticeable in the second.
const BUTTON_HOLD_MS: u32 = 1500;
/// How often to check during the hold.
const BUTTON_POLL_MS: u32 = 100;

/// Antenna tuning capacitance, picofarads. The SEN0290's factory value, and
/// changing it needs a scope on the IRQ pin (§3 step 5).
const TUNING_CAPS_PF: u8 = 120;


/// How long to collect events before summarising them (§4.2's "~1 s batch").
const BATCH_MS: u32 = 1000;

/// The one measurement window (§4.2).
///
/// **A single period for everything**: the noise level is reconsidered once per
/// window, and the same window's counts are what the screen reports. One
/// constant rather than three, so the ladder can never be deciding on evidence
/// the display is not showing.
///
/// **Sixty seconds**, matching the calibration probe window.
///
/// The +/-1 walk that runs between calibrations is judging the same question a
/// probe judges -- "did that window hear anything" -- so it deserves the same
/// span. At ten seconds it was deciding on a sample too short to distinguish a
/// quiet room from a gap between events, and stepping the point every ten
/// seconds on that evidence.
const MEASURE_INTERVAL_S: u32 = 60;

/// How often the learned tuning point may be written back to NVS.
///
/// Fifteen minutes, matching `clock::SAVE_INTERVAL_S` and for the same reason:
/// the value it protects re-learns in minutes, so a power cut costs almost
/// nothing while a write every window would cost flash endurance for real. A
/// settled room stops moving the point at all, and then this writes once and
/// never again.
const DEFENCE_SAVE_S: u32 = 15 * 60;



/// How often to read the reason register with no interrupt having asked for it.
///
/// **Plan B, and a diagnostic.** The register holds its reason until something
/// reads it, so an event whose `INT` edge was missed is still sitting there
/// afterwards. Thirty seconds is far slower than a storm but far faster than
/// never, and it costs one I2C transaction — against a device that already polls
/// a fuel gauge three times as often.
///
/// What makes it worth having is the asymmetry every measurement here has shown:
/// hundreds of `NoiseTooHigh`, which is a near-continuous condition, and never
/// once a disturber or a strike, which are impulsive and whose pulse is brief.
/// If polling starts finding events, the interrupt path is losing them — a
/// different defect from a sensor that hears nothing, and one that has been
/// indistinguishable from outside.
const IRQ_POLL_INTERVAL_S: u32 = 30;

/// How often to read the fuel gauge.
///
/// Slower than the batch loop because it is an I2C transaction for values that
/// move over hours — but far faster than the screen, because the *clock policy*
/// depends on it and a device that has been unplugged should not stay at
/// 160 MHz waiting for a redraw.
const GAUGE_POLL_S: u32 = 10;

/// Shortest gap between panel refreshes.
///
/// **A full 800×480 refresh measured 3.9 s on this panel.** §6's nominal 5 s
/// cadence would leave it busy roughly 80 % of the time — the device would
/// spend its life redrawing, and during a storm every refresh would be stale
/// before it finished. So the screen is change-gated with a floor under it,
/// and this is the floor.
const REDRAW_MIN_GAP_S: u32 = 30;

/// Redraw even if nothing tracked has changed, at most this often.
///
/// The backstop for everything the change test deliberately ignores — the
/// uptime, the disturber count, the battery, the die temperature. Without it
/// those fields would be correct only at boot, which is exactly how "up 0m" was
/// still on the glass long after boot.
///
/// Five minutes rather than fifteen: at 3.8 s a refresh that is ~1.3 % of the
/// panel's time, which is affordable, and it keeps every slow-moving field on
/// screen within a period short enough that nobody mistakes it for frozen.
const REDRAW_BASELINE_S: u32 = 5 * 60;

/// How long the cold-boot logo stays up before the first status screen.
///
/// The splash is worth a moment and not worth more. Note the panel itself eats
/// most of any budget here — the logo's own refresh is ~3.8 s before this delay
/// even starts, and the status screen that replaces it costs another 3.8 s — so
/// this is a *dwell*, not the total time a logo is visible.
const LOGO_DWELL_MS: u32 = 5000;

fn main() {
    esp_idf_hal::sys::link_patches();

    let Ok(peripherals) = Peripherals::take() else {
        println!("FATAL: peripherals unavailable");
        return;
    };

    // === Claim the IRQ pad FIRST, before the console delay ==================
    //
    // ⚠ GPIO21 is **U0TXD**, and a UART transmit line idles **high**. The
    // AS3935's INT idles **low**. So from the moment the chip leaves reset until
    // this pad is reconfigured, the ROM bootloader's UART driver and the
    // sensor's INT output are both driving it, in opposite directions — two
    // push-pull CMOS outputs shorted through each other, bounded only by their
    // own impedances.
    //
    // Nothing here can prevent the ROM's share of that window. What this
    // ordering *does* prevent is extending it: the console settle delay below
    // is over two seconds, and taking it first would hold the contention for
    // that whole time on every single boot.
    //
    // The permanent fixes are hardware, not firmware — see the note in §2.
    //
    // ⚠⚠ And a second trap on the very same pad, one layer below the first.
    // **`PinDriver` calls `gpio_set_direction`, not `gpio_reset_pin`** — and it
    // is `gpio_reset_pin` that restores a pad's IO_MUX selection to plain GPIO.
    // Setting a direction on a pad still muxed to a peripheral changes nothing:
    // the peripheral keeps the pin, and reads return *its* state rather than
    // whatever is wired to the pad.
    //
    // The ROM muxes GPIO21 to UART0 at every reset, so without this call the
    // pin reads UART idle — a steady high — no matter what the sensor does.
    // That presents as "the IRQ is not connected", which is exactly the wrong
    // conclusion and costs a wiring investigation. The moisture project lost an
    // evening to the same call on the same pad, driving an e-paper DC line.
    //
    // SAFETY: a plain IDF call on a pad nothing else owns yet.
    unsafe {
        esp_idf_hal::sys::gpio_reset_pin(21);
    }

    let Ok(mut irq) = PinDriver::input(peripherals.pins.gpio21, Pull::Down) else {
        println!("FATAL: GPIO21 would not become an input");
        return;
    };

    // The USB-serial-JTAG console enumerates when the host opens the port, a
    // moment or two after boot. Anything printed before then goes into a FIFO
    // nobody is draining, so the banner would be the one thing never seen.
    FreeRtos::delay_ms(2000);

    println!();
    println!("=== lightning terminal ===");
    println!("fw {}", env!("CARGO_PKG_VERSION"));
    println!("boot: {}", system::reset_reason_name());

    // §5's clock: a persisted epoch plus uptime, with no network involved.
    // Said out loud either way -- a device that comes up without a time will
    // write unstamped records until somebody tells it one, and that is worth
    // knowing before the log is trusted.
    match clock::restore() {
        Some(epoch) => println!(
            "time: restored {} {}",
            clock::format_local(epoch),
            clock::tz_label()
        ),
        None => println!("time: NOT SET -- records will be unstamped. Use: time <unix-epoch>"),
    }

    // §2: the display consumes GPIO 2, 3, 4, 5, 8 and 10, which leaves the
    // XIAO's native I2C pads free. These are fixed by that, not chosen.
    let config = I2cConfig::new().baudrate(Hertz(I2C_HZ));
    let Ok(mut i2c) = I2cDriver::new(
        peripherals.i2c0,
        peripherals.pins.gpio6,
        peripherals.pins.gpio7,
        &config,
    ) else {
        println!("FATAL: I2C0 would not initialise");
        return;
    };
    println!("i2c:  SDA=GPIO6 SCL=GPIO7 @ {} kHz", I2C_HZ / 1000);

    let found = i2c_scan::scan(&mut i2c);
    println!("scan: {} device(s)", found.len());
    for device in found.iter() {
        match device.expected {
            Some(what) => println!("      0x{:02x}  {}", device.address, what),
            None => println!(
                "      0x{:02x}  UNEXPECTED -- nothing in §2 claims this address",
                device.address
            ),
        }
    }
    println!("      => {}", i2c_scan::verdict(&found));

    // --- the sensor ------------------------------------------------------
    let Some(sensor) = As3935::find(&mut i2c) else {
        println!("FATAL: no AS3935 answered a reset at 0x01/0x02/0x03");
        return;
    };
    println!("as:   found at 0x{:02x}", sensor.address());

    // §2.1's fuel gauge. Absence is not fatal -- the device detects lightning
    // whether or not it can say how much charge is left, and on USB it is not
    // even an interesting question.
    let gauge = match battery::Max17048::find(&mut i2c) {
        Some((gauge, version)) => {
            println!("bat:  MAX17048 at 0x36, version 0x{version:04x}");
            Some(gauge)
        }
        None => {
            println!("bat:  no fuel gauge answered at 0x36 -- running without a battery readout");
            None
        }
    };

    // Held open rather than installed per read: enabling the sensor takes time
    // to settle, and this value moves slowly.
    // §5's strike log, on raw flash. Absence is not fatal: the device still
    // detects and still shows, it just forgets across power cuts.
    let mut strike_log = match log::Log::open() {
        Some(log) => {
            println!(
                "log:  {} records, {} KB used, {} KB free",
                log.len(),
                // `div_ceil`, matching `system::health` -- a non-empty log
                // must never print as `0 KB used`, which reads as "no data".
                // Fixed in `health` first and missed here: same figure, two
                // formatters.
                log.used_bytes().div_ceil(1024),
                log.free_bytes() / 1024
            );
            Some(log)
        }
        None => {
            println!("log:  no `storage` partition -- strikes will not be recorded");
            None
        }
    };

    let die_temperature = system::DieTemperature::new();
    if die_temperature.is_none() {
        println!("sys:  die temperature sensor unavailable");
    }

    // §4.1: NVS is the source of truth, the constant is only the fallback.
    let mut location = match settings::location() {
        Some(stored) => stored,
        None => {
            println!("set:  no stored location -- defaulting to {}", DEFAULT_LOCATION.label());
            DEFAULT_LOCATION
        }
    };

    let start_point = match configure(&sensor, &mut i2c, location) {
        Ok(point) => point,
        Err(e) => {
            println!("FATAL: sensor configuration failed -- {e}");
            return;
        }
    };

    // --- the IRQ ---------------------------------------------------------
    //
    // The pad was claimed at the top of `main`; only the edge configuration is
    // left. §2: GPIO9 is out as an IRQ candidate -- it is the boot strap, and
    // the AS3935's INT idles low, so a low there at reset drops the chip into
    // the ROM downloader and our firmware never runs at all.
    if let Err(e) = irq.set_interrupt_type(InterruptType::PosEdge) {
        println!("FATAL: GPIO21 interrupt type -- {e}");
        return;
    }

    let notification = Notification::new();
    let notifier = notification.notifier();

    // SAFETY: the closure runs in interrupt context, so it may not allocate,
    // block, or touch the I2C bus. It does exactly one thing -- post a
    // notification -- which is the whole point of the pattern (§3): the
    // MicroPython reference does its register reads inside this callback, and
    // that must not be copied here.
    if let Err(e) = unsafe {
        irq.subscribe(move || {
            notifier.notify_and_yield(NonZeroU32::new(NOTIFY_STRIKE).unwrap());
        })
    } {
        println!("FATAL: could not subscribe to GPIO21 -- {e}");
        return;
    }

    // --- the mode button -------------------------------------------------
    //
    // GPIO9 is the XIAO's BOOT button. Using it at runtime is safe and using it
    // at reset is not: the strap is sampled only as the chip leaves reset, so a
    // press while running is just a button, while a press *during* a power
    // cycle drops the chip into the ROM downloader.
    //
    // It replaces an earlier plan to toggle on every boot. That would have been
    // silently stateful -- a power blip, a watchdog reset or a knocked cable
    // flips the mode with no intent behind it, and after two of those nobody
    // knows which mode the device is in without waiting for a strike.
    let Ok(mut button) = PinDriver::input(peripherals.pins.gpio9, Pull::Up) else {
        println!("FATAL: GPIO9 would not become an input");
        return;
    };
    // The button shorts to ground, so the press is the falling edge.
    if let Err(e) = button.set_interrupt_type(InterruptType::NegEdge) {
        println!("FATAL: GPIO9 interrupt type -- {e}");
        return;
    }
    let button_notifier = notification.notifier();
    // SAFETY: same contract as the IRQ above -- notify only.
    if let Err(e) = unsafe {
        button.subscribe(move || {
            button_notifier.notify_and_yield(NonZeroU32::new(NOTIFY_BUTTON).unwrap());
        })
    } {
        println!("FATAL: could not subscribe to GPIO9 -- {e}");
        return;
    }

    // Prove the wire before trusting silence. See `antenna_self_test`.
    let (irq_confirmed, antenna_khz) = antenna_self_test(&sensor, &mut i2c, &irq);

    println!("irq:  GPIO21 (D6), rising edge, pulldown");
    println!("btn:  GPIO9 (BOOT), HOLD {BUTTON_HOLD_MS} ms to switch indoor/outdoor");
    println!("as:   running {}", location.label());

    // --- the panel --------------------------------------------------------
    let mut panel = match display::Panel::new(display::Pins {
        spi: peripherals.spi2,
        sclk: peripherals.pins.gpio8,
        mosi: peripherals.pins.gpio10,
        cs: peripherals.pins.gpio3,
        dc: peripherals.pins.gpio5,
        rst: peripherals.pins.gpio2,
        busy: peripherals.pins.gpio4,
    }) {
        Ok(panel) => {
            println!("epd:  UC8179 800x480 up");
            println!("epd:  BUSY pad (GPIO4) is {}", display::Panel::busy_verdict());
            Some(panel)
        }
        Err(e) => {
            // Not fatal. A detector with no screen still detects, still logs,
            // and still reports over the console -- and saying so is more use
            // than halting.
            println!("epd:  INIT FAILED -- {e} (carrying on without a screen)");
            None
        }
    };

    // The splash goes up before anything else visible. It is the only sign the
    // device gives that it started, and the first status screen is gated behind
    // a batch window -- so without this the panel holds whatever was on it from
    // the last run, which reads exactly like a board that did not boot.
    if let Some(panel) = panel.as_mut() {
        let mut frame = display::Panel::frame();
        ui::logo(&mut frame);
        match panel.show(&frame) {
            Ok(0) => println!("epd:  logo sent, but BUSY never fell -- nothing was drawn"),
            Ok(busy_ms) => println!("epd:  logo drawn, panel busy {busy_ms} ms"),
            Err(e) => println!("epd:  logo FAILED -- {e}"),
        }
        FreeRtos::delay_ms(LOGO_DWELL_MS);
    }

    println!("--- listening ---");

    listen(
        &sensor,
        &mut i2c,
        &mut irq,
        &mut button,
        &notification,
        &mut location,
        panel.as_mut(),
        antenna_khz,
        irq_confirmed,
        gauge.as_ref(),
        die_temperature.as_ref(),
        strike_log.as_mut(),
        start_point,
    );
}

/// §3's init sequence, in the order the datasheet and the reference agree on.
fn configure(
    sensor: &As3935,
    i2c: &mut I2cDriver<'_>,
    location: Location,
) -> Result<defence::Point, esp_idf_hal::sys::EspError> {
    sensor.power_up(i2c)?;
    sensor.set_location(i2c, location)?;

    // Disturbers stay ENABLED. It looks wrong for a lightning detector and is
    // not: §4.2's noise auto-tune is driven by disturber events, so masking
    // them leaves the floor pinned wherever it started.
    sensor.set_disturber_enabled(i2c, true)?;
    sensor.clear_irq_display(i2c)?;
    FreeRtos::delay_ms(500);

    sensor.set_tuning_caps(i2c, TUNING_CAPS_PF)?;

    // **And that is the end of the init, deliberately.**
    //
    // The reference's whole configuration is the six calls above — power up,
    // indoor/outdoor, disturbers on, IRQ source, 500 ms, tuning caps. It never
    // writes `NF_LEV`, `WDTH`, `SREJ` or `MIN_NUM_LIGH` at start-up, and never
    // writes the last three at all. That is the configuration which has actually
    // detected lightning on this hardware.
    //
    // Two calls used to live here and have been removed:
    //
    // * `apply_defence(0)`, which wrote all three registers and in particular
    //   forced **`SREJ` from its power-on 2 down to 0**. Spike rejection is part
    //   of the validation chain rather than a sensitivity knob, so "lower is
    //   more sensitive" was the wrong model — and no configuration that ever
    //   detected a strike had it anywhere but the default.
    // * `set_min_strikes(1)`, which is a no-op against a chip that powers up at
    //   one strike, and was therefore paying a bus transaction to state a
    //   preference the datasheet already guarantees.
    //
    // What the runtime still does is exactly what the reference does: walk
    // `NF_LEV` between 0 and 7 and touch nothing else (§4.2).
    let min_strikes = sensor.min_strikes(i2c)?;

    // **Every boot starts at the most sensitive noise floor**, written
    // explicitly so the chip and the ladder's idea of it cannot disagree.
    //
    // The chip powers up at `NF_LEV` 2, and leaving it there meant a `level`
    // variable of 0 describing hardware set to 2 -- so the first "climb" would
    // have written a *lower* floor than was already in force, making the
    // receiver more sensitive in response to noise. Reading the chip back would
    // fix the disagreement; starting from a known floor fixes it *and* makes
    // every session begin identically, which is worth more on a device whose
    // whole difficulty has been telling configuration apart from environment.
    //
    // Nothing is persisted. The ladder re-finds the room's noise floor within a
    // few minutes, so storing it would buy a few minutes of accuracy at the cost
    // of a flash write on a value that changes with the weather.
    //
    // `NF_LEV` is the one register the reference tunes, so writing it here is
    // its first tweak brought forward, not a new deviation -- unlike `WDTH` and
    // `SREJ`, which stay untouched at their power-on defaults.
    // Resume where this room left off, rather than climbing from zero again.
    let point = match settings::defence_point() {
        Some(stored) => {
            println!("as:   resumed defence point from NVS");
            stored
        }
        None => {
            // Not `OPEN`. A never-calibrated device booting fully open drowns --
            // measured here at 7-9 noise events per batch, continuously -- and
            // the +/-1 walk needs about a thousand windows to climb out of it.
            // `default_start` opens mid-range on the two volume knobs and leaves
            // the two that decide whether a strike is reported at all untouched.
            println!("as:   no stored defence point -- starting from the default");
            defence::Point::default_start()
        }
    };
    session::apply(sensor, i2c, point)?;

    println!(
        "as:   {}, {} pF, defence {}/{} ({}%), report after {} strike(s)",
        location.label(),
        TUNING_CAPS_PF,
        point.raw(),
        defence::MAX,
        point.percent(),
        min_strikes
    );
    Ok(point)
}

/// The event loop: batch what arrives, then tune once per batch (§4.2).
fn listen(
    sensor: &As3935,
    i2c: &mut I2cDriver<'_>,
    irq: &mut PinDriver<'_, esp_idf_hal::gpio::Input>,
    button: &mut PinDriver<'_, esp_idf_hal::gpio::Input>,
    notification: &Notification,
    location: &mut Location,
    mut panel: Option<&mut display::Panel<'_>>,
    antenna_khz: u32,
    irq_confirmed: bool,
    gauge: Option<&battery::Max17048>,
    die_temperature: Option<&system::DieTemperature>,
    mut strike_log: Option<&mut log::Log>,
    start_point: defence::Point,
) {
    let mut point = start_point;
    // What is on flash, so a write only happens when the point has actually
    // moved away from it.
    let mut stored_point = start_point;
    let mut last_point_save_s: u32 = now_ms() / 1000;
    // Counted in the window now being judged, and when it started.
    let mut window_events: u32 = 0;
    let mut window_disturbers: u32 = 0;
    let mut tune_window_ms: u32 = now_ms();
    let mut last_irq_poll_ms: u32 = now_ms();
    // Set by `sensitive on`; see `session::force_max_sensitivity`. Deliberately
    // not persisted to NVS -- it is a diagnostic override for a storm happening
    // now, and a device that silently came back from a power cut with its noise
    // rejection disabled would be a trap. (`///` here would be a doc comment on
    // a local, which Rust warns about: locals have no docs to attach to.)
    let mut max_sensitivity = false;
    let mut last_button_ms: u32 = 0;
    // Set by a button press, cleared by the redraw it causes.
    let mut user_acted = false;
    let mut batch = Batch::default();
    let mut batch_started = now_ms();

    // Running totals, and what the glass currently shows. The screen is redrawn
    // when the two disagree -- never on a timer alone.
    let mut totals = Totals::default();
    let mut history = history::History::new();

    // Rebuild the rings from the file (§5). The charts are RAM and die with a
    // power cut; the CSV does not -- so without this every reboot would show a
    // device that had never seen a storm.
    if strike_log.is_some() {
        let mut replayed = 0u32;
        log::for_each(|epoch, strike| {
            history.record((epoch / 60) as u32, &strike);
            replayed += 1;
        });
        if replayed > 0 {
            println!("log:  replayed {replayed} record(s) into the charts");
        }
    }
    // Scratch for the chart series. Held here rather than built per redraw:
    // the day ring alone is 96 buckets, and a redraw already costs 3.8 s of
    // panel time without allocating on the way in.
    // Sized for the longest ring, so one buffer serves all three periods; the
    // shorter ones simply use a prefix.
    let mut chart_counts = [0u16; history::MEDIUM_LEN];
    let mut chart_scores = [0u32; history::MEDIUM_LEN];
    let mut chart_period = ui::ChartPeriod::Day;

    // §2.1's learned range. Seeded rather than empty, so the first reading has
    // something to widen from -- and `None` from NVS is a virgin device, not an
    // error.
    let mut range = settings::battery_range().unwrap_or(battery::SEED_RANGE);

    // §7's clock policy. Starts on the USB assumption -- the device is usually
    // plugged in, and being wrong that way costs power rather than a console.
    let mut policy = power::Policy::Awake;
    // `Some(mhz)` while `freq <mhz>` is in force. Deliberately not persisted:
    // it exists so a board can be watched over USB, and one that came back from
    // a power cut silently refusing to sleep would quietly cost the battery.
    let mut freq_override: Option<u32> = None;
    let mut console = console::Console::new();
    // Uptime at the last console input, and at the last clock save.
    let mut last_console_s: Option<u32> = None;
    let mut last_clock_save_s: u32 = 0;
    let mut last_log_sync_ms: u32 = 0;
    // The most recent gauge reading, or None until the first poll or if the
    // gauge is absent. Cached because the screen wants it far less often than
    // the loop runs, and it is an I2C transaction.
    let mut reading: Option<battery::Reading>;
    // Voltage over time, because `CRATE` cannot see a charger in taper -- see
    // `battery::Trend`.
    let mut trend: Option<battery::Trend> = None;
    // Read once up front rather than waiting out the first poll interval, so
    // the very first screen carries a real battery figure instead of "no gauge".
    reading = gauge.and_then(|g| g.read(i2c).ok());
    if let Some(reading) = reading {
        trend = Some(battery::Trend::new(reading.millivolts, now_ms() / 1000));
    }
    let mut last_gauge_ms: u32 = now_ms();

    // The discharge accumulator (§7). Restored rather than started fresh: it
    // averages over days, so a device that reset an hour ago must not go back
    // to "no estimate" -- that is exactly the window in which somebody is
    // watching it and wants one.
    let mut drain = settings::battery_drain().unwrap_or_default();
    // Seeded from the reading taken above, so the first interval measured is a
    // real one rather than the gap between boot and the first poll.
    let mut previous_mv: Option<u16> = reading.map(|r| r.millivolts);
    let mut last_drain_s: u32 = now_ms() / 1000;
    let mut last_drain_save_s: u32 = now_ms() / 1000;
    match power::apply(policy) {
        Ok(()) => match power::config() {
            Some((max, min, sleep)) => println!(
                "pm:   {} -- {}/{} MHz, light sleep {}",
                policy.label(),
                min,
                max,
                if sleep { "on" } else { "off" }
            ),
            None => println!("pm:   applied {} but could not read it back", policy.label()),
        },
        Err(e) => println!("pm:   could not apply {} -- {e}", policy.label()),
    }
    // A calibration in progress, driven one probe per window by the tune block
    // below. `None` when the +/-1 walk is in charge.
    let mut sweep: Option<session::Sweep> = None;
    // **The zero both the search and the +/-1 walk compare against.** One
    // variable rather than two constants, so they can never disagree about what
    // a quiet window is -- which is exactly how a 60 s sweep came to settle
    // deafer than a 10 s one in the same room. See `session::QUIET_PER_MIN`.
    let mut quiet_per_min = match settings::quiet_per_min() {
        Some(stored) => {
            println!("as:   quiet threshold {stored}/min (from NVS)");
            stored
        }
        None => {
            println!("as:   quiet threshold {}/min (default)", session::QUIET_PER_MIN);
            session::QUIET_PER_MIN
        }
    };
    let mut drawn: Option<Drawn> = None;
    let mut last_draw_ms: u32 = 0;

    loop {
        // Re-arming is required after every trigger: esp-idf disables the
        // interrupt when it fires, so a loop that forgets this hears exactly one
        // event and then waits forever. Both sources need it.
        if let Err(e) = irq.enable_interrupt() {
            println!("irq:  could not re-arm GPIO21 -- {e}");
            return;
        }
        if let Err(e) = button.enable_interrupt() {
            println!("btn:  could not re-arm GPIO9 -- {e}");
            return;
        }

        // Wait only for what is left of the current window, so the batch closes
        // on time however many events arrive inside it.
        let elapsed = now_ms().saturating_sub(batch_started);
        let remaining = BATCH_MS.saturating_sub(elapsed).max(1);
        let woke = notification.wait(TickType::new_millis(remaining as u64).into());

        if let Some(source) = woke {
            let bits = source.get();

            if bits & NOTIFY_BUTTON != 0 {
                // **Confirm the pin is actually held down.** An edge alone is
                // not a press: GPIO9 is a strapping pin on a board sharing a
                // bench with a lightning sensor, and a glitch on it produced a
                // falling edge indistinguishable from a fingertip.
                //
                // That mattered more than a stray toggle would suggest. A
                // spurious press changes `location`, which forces a redraw, and
                // `user_acted` deliberately bypasses the 30 s floor -- so each
                // glitch bought an immediate 3.8 s refresh AND an NVS write.
                // The symptom was a screen repainting without pause and no logo
                // between repaints, which ruled out a reboot and left this.
                //
                // A real press is held for tens of milliseconds; a glitch is
                // over in microseconds. Sampling after a short settle tells them
                // apart with no state and no timer.
                let held = button_held(button);
                let ready = now_ms().saturating_sub(last_button_ms) >= BUTTON_DEBOUNCE_MS;

                if held && ready {
                    last_button_ms = now_ms();
                    toggle_location(sensor, i2c, location);
                    // A deliberate press earns an immediate repaint -- see below.
                    user_acted = true;
                } else if !held {
                    // Almost always a flashing tool asserting DTR, which the C3
                    // wires to this pin. Said out loud rather than swallowed,
                    // because "the button does nothing" and "the button is
                    // being pressed by your laptop" look identical otherwise.
                    println!("btn:  edge not held {BUTTON_HOLD_MS} ms -- ignored (USB DTR?)");
                }
            }

            // Any bit that is not the button is the sensor. Not
            // `& NOTIFY_STRIKE` on purpose: an unrecognised bit is far more
            // likely to be a bug in a notifier than a real third source, and
            // dropping it silently would lose a strike.
            if bits & !NOTIFY_BUTTON != 0 {
                collect(
                    sensor,
                    i2c,
                    &mut batch,
                    &mut totals,
                    &mut history,
                    minute_now(),
                    strike_log.as_deref_mut(),
                );
            }
        }

        // Not yet at the end of the window -- go back and keep listening.
        if now_ms().saturating_sub(batch_started) < BATCH_MS {
            continue;
        }

        // Plan B: look for events the interrupt line never announced. Before
        // `report`, so anything found is summarised in this batch rather than
        // the next one.
        if now_ms().saturating_sub(last_irq_poll_ms) >= IRQ_POLL_INTERVAL_S * 1000 {
            last_irq_poll_ms = now_ms();
            session::poll(
                sensor,
                i2c,
                &mut batch,
                &mut totals,
                &mut history,
                minute_now(),
                strike_log.as_deref_mut(),
            );
        }

        report(&batch);

        // Keep the rings' idea of "now" current even in a lull, so a chart drawn
        // during quiet weather shows the quiet rather than the last storm shoved
        // against its right edge.
        history.tick(minute_now());

        // --- the console ----------------------------------------------------
        //
        // Any input counts as "somebody is here", which is what holds the awake
        // policy -- see `power::decide`. It is the only supply signal that has
        // not lied, because it is about a person rather than a cable.
        if let Some(command) = console.poll() {
            last_console_s = Some(now_ms() / 1000);
            let effects = commands::run(
                command,
                &mut commands::Ctx {
                    location,
                    totals: &mut totals,
                    history: &mut history,
                    strike_log: strike_log.as_deref_mut(),
                    chart_period: &mut chart_period,
                    reading,
                    range,
                    level: point.percent(),
                    die_temperature,
                    antenna_khz,
                    irq_confirmed,
                    minute: minute_now(),
                    uptime_minutes: now_ms() / 60_000,
                    max_sensitivity,
                },
            );
            if effects.clock_saved {
                last_clock_save_s = now_ms() / 1000;
            }
            if effects.redraw_now {
                drawn = None;
                user_acted = true;
            }
            if effects.read_battery {
                match gauge {
                    // One read, decoded and printed raw, so the two lines always
                    // describe the same instant and can be checked against each
                    // other. Also deliberately fresh rather than the main loop's
                    // cached sample: `battery` is asked when somebody wants to
                    // know *now*.
                    Some(gauge) => match gauge.read_raw(i2c) {
                        Ok((vcell, soc, crate_raw)) => {
                            let now = battery::Reading::from_raw(vcell, soc, crate_raw);
                            println!(
                                "batt: {}% -- {}.{:02} V, {} mV",
                                now.percent,
                                now.millivolts / 1000,
                                (now.millivolts % 1000) / 10,
                                now.millivolts
                            );
                            println!(
                                "batt: {} -- CRATE {}.{:02} %/hr",
                                battery::flow(&now, trend.as_ref()).label(),
                                now.crate_centi_per_hour / 100,
                                (now.crate_centi_per_hour % 100).abs()
                            );
                            match trend.as_ref() {
                                Some(t) if t.span_s() >= 60 => println!(
                                    "batt: trend {:+} mV over {} min (>= {} mV counts)",
                                    t.delta_mv(),
                                    t.span_s() / 60,
                                    battery::TREND_THRESHOLD_MV
                                ),
                                // Say so rather than printing a delta that has
                                // had no time to mean anything.
                                _ => println!("batt: trend -- not enough history yet"),
                            }
                            println!("batt: learned range {}-{} mV", range.0, range.1);
                            println!(
                                "batt: raw VCELL 0x{vcell:04X}  SOC 0x{soc:04X}  CRATE 0x{crate_raw:04X}"
                            );
                        }
                        Err(e) => println!("batt: read failed -- {e}"),
                    },
                    None => println!("batt: no gauge found on the bus"),
                }
            }
            if let Some(request) = effects.freq {
                use crate::console::FreqRequest;
                match request {
                    FreqRequest::Pin(mhz) if !power::PINNABLE_MHZ.contains(&mhz) => println!(
                        "freq: {mhz} MHz is not one of {:?} -- unchanged",
                        power::PINNABLE_MHZ
                    ),
                    FreqRequest::Pin(mhz) => match power::pin(mhz) {
                        Ok(()) => {
                            freq_override = Some(mhz);
                            println!("freq: pinned at {mhz} MHz, light sleep off, policy paused");
                        }
                        Err(e) => println!("freq: could not pin {mhz} MHz -- {e}"),
                    },
                    FreqRequest::Auto => {
                        freq_override = None;
                        // Re-apply immediately rather than waiting for the next
                        // tick: the loop below only acts on a *change*, and the
                        // policy it last recorded is what the pin displaced.
                        let want = power::decide(now_ms() / 1000, last_console_s);
                        match power::apply(want) {
                            Ok(()) => {
                                policy = want;
                                println!("freq: back on the {} policy", policy.label());
                            }
                            Err(e) => println!("freq: could not restore the policy -- {e}"),
                        }
                    }
                    FreqRequest::Report => {}
                }
                // Always report what the chip is enforcing, not what we asked
                // for -- `esp_pm_configure` can reject a config and leave the
                // previous one running.
                match power::config() {
                    Some((max, min, sleep)) => println!(
                        "freq: now {} MHz -- pm {}/{} MHz, light sleep {}, {}",
                        system::cpu_mhz(),
                        min,
                        max,
                        if sleep { "on" } else { "off" },
                        match freq_override {
                            Some(mhz) => format!("pinned at {mhz} MHz"),
                            None => format!("policy {}", policy.label()),
                        }
                    ),
                    None => println!("freq: {} MHz (pm read-back failed)", system::cpu_mhz()),
                }
            }
            if let Some(on) = effects.light_sleep {
                // Keep whatever clock is in force and change only the sleep
                // flag, so `sleep off` after `freq 80` does not silently drag
                // the frequency back to something else.
                match power::config() {
                    Some((max, min, _)) => match power::set_light_sleep(max, min, on) {
                        Ok(()) => {
                            // Counts as an override either way: the policy loop
                            // would otherwise restore its own idea of both.
                            freq_override = Some(max);
                            println!(
                                "sleep: light sleep {} at {min}/{max} MHz, policy paused",
                                if on { "on" } else { "off" }
                            );
                            if on {
                                println!("sleep: the USB port will go with it -- `freq auto` or a power cycle to return");
                            }
                        }
                        Err(e) => println!("sleep: could not change it -- {e}"),
                    },
                    None => println!("sleep: could not read the current config -- unchanged"),
                }
            }
            if let Some(indoor) = effects.set_indoor {
                *location = if indoor {
                    as3935::Location::Indoor
                } else {
                    as3935::Location::Outdoor
                };
                match sensor.set_location(i2c, *location) {
                    // Outdoor is the LOWER gain, which is the counter-intuitive
                    // half: indoor gain is ~4x, and a strike close enough to hear
                    // saturates the front end, fails validation, and is reported
                    // as a disturber. Less gain is what recovers a near storm.
                    Ok(()) => println!("mode: {} gain applied", location.label()),
                    Err(e) => println!("mode: could not apply -- {e}"),
                }
            }
            if effects.dump_registers {
                match sensor.dump_registers(i2c) {
                    Ok(r) => {
                        for (n, value) in r.iter().enumerate() {
                            println!("regs: 0x0{n} = 0x{value:02X}  {:08b}", value);
                        }
                        // Decoded, because the failure this exists to catch is a
                        // bit being set that nobody meant to set.
                        println!(
                            "regs: pwd {}  afe 0x{:02X} ({})",
                            r[0] & 0x01,
                            (r[0] & 0x3E) >> 1,
                            if (r[0] & 0x3E) >> 1 == 0x12 { "indoor" } else { "outdoor" }
                        );
                        println!(
                            "regs: nf {}  wdth {}  srej {}  min-strikes-bits {}",
                            (r[1] & 0x70) >> 4,
                            r[1] & 0x0F,
                            r[2] & 0x0F,
                            (r[2] & 0x30) >> 4
                        );
                        println!(
                            "regs: int 0x{:X}  mask_dist {}  lco_fdiv {}",
                            r[3] & 0x0F,
                            (r[3] & 0x20) >> 5,
                            (r[3] & 0xC0) >> 6
                        );
                        // The one that makes the sensor deaf: any of these three
                        // set means the IRQ pin is a clock output, not an
                        // interrupt line.
                        let display = (r[8] & 0xE0) >> 5;
                        println!(
                            "regs: tun_cap {}  irq_display {:03b}{}",
                            r[8] & 0x0F,
                            display,
                            if display != 0 {
                                "  *** IRQ PIN IS A CLOCK OUTPUT -- SENSOR IS DEAF ***"
                            } else {
                                ""
                            }
                        );
                    }
                    Err(e) => println!("regs: read failed -- {e}"),
                }
            }

            // The sweep owns the loop while it runs: it needs the sensor, the
            // bus and the notification, and it deliberately holds bad settings
            // between probes.
            if effects.show_point {
                println!(
                    "def:  {}/{} ({}%) -- {}",
                    point.raw(),
                    defence::MAX,
                    point.percent(),
                    session::describe(point)
                );
            }

            // Set by hand -- how a room with a known answer skips the sweep.
            if let Some(raw) = effects.set_point {
                point = defence::Point::new(raw);
                match session::apply(sensor, i2c, point) {
                    Ok(()) => println!(
                        "def:  set to {}/{} ({}%) -- {}",
                        point.raw(),
                        defence::MAX,
                        point.percent(),
                        session::describe(point)
                    ),
                    Err(e) => println!("def:  could not program -- {e}"),
                }
                window_events = 0;
                window_disturbers = 0;
                tune_window_ms = now_ms();
            }

            // Start a sweep. It runs as ordinary measurement windows from here
            // on -- the tune block below spends each one on the search instead
            // of on the +/-1 walk -- so the loop, the console and the screen all
            // keep working throughout.
            if let Some((window_s, requested_quiet)) = effects.calibrate {
                // `u32::MAX` is the "not given" marker -- the stored threshold
                // stands unless the operator names a new one.
                if requested_quiet != u32::MAX && requested_quiet != quiet_per_min {
                    quiet_per_min = requested_quiet;
                    match settings::store_quiet_per_min(quiet_per_min) {
                        Ok(()) => println!("cal:  quiet threshold now {quiet_per_min}/min (saved)"),
                        Err(e) => {
                            println!("cal:  threshold {quiet_per_min}/min but NOT saved -- {e}")
                        }
                    }
                }
                let started = session::Sweep::new(window_s);
                println!(
                    "cal:  starting -- 0..={}, {} s per probe, quiet is <={}/min, about {} probes",
                    defence::MAX,
                    started.window_s,
                    quiet_per_min,
                    started.remaining()
                );
                point = started.point();
                if let Err(e) = session::apply(sensor, i2c, point) {
                    println!("cal:  could not program the first probe -- {e}");
                }
                FreeRtos::delay_ms(session::CALIBRATE_SETTLE_MS);
                sweep = Some(started);
                window_events = 0;
                window_disturbers = 0;
                tune_window_ms = now_ms();
                user_acted = true;
            }


            // Applied here rather than in the command handler because `Ctx` has
            // no sensor or bus -- see `Effects::sensitivity`.
            if let Some(on) = effects.sensitivity {
                max_sensitivity = on;
                let outcome = if on {
                    session::force_max_sensitivity(sensor, i2c)
                } else {
                    // Back to fully receptive, not to wherever the point
                    // happened to be: the auto-tune was frozen, so it is stale
                    // by however long the override was on.
                    point = defence::Point::OPEN;
                    window_events = 0;
                    window_disturbers = 0;
                    tune_window_ms = now_ms();
                    session::apply(sensor, i2c, point)
                };
                match outcome {
                    Ok(()) if on => println!(
                        "sens: MAX -- nf 0, wdth 0, srej 0, min strikes 1; auto-tune frozen"
                    ),
                    Ok(()) => println!("sens: normal -- defence back to 0"),
                    Err(e) => println!("sens: could not apply -- {e}"),
                }
            }
        }

        // Flush the strike log on its own cadence. Buffered rather than synced
        // per strike so a storm producing events every few seconds does not
        // become a flash write every few seconds -- the trade being that a
        // power cut loses at most a minute. Safe on LittleFS specifically: an
        // unsynced write is lost, not corrupting.
        if now_ms().saturating_sub(last_log_sync_ms) >= log::SYNC_INTERVAL_MS {
            last_log_sync_ms = now_ms();
            if let Some(log) = strike_log.as_deref_mut() {
                match log.sync() {
                    Ok(0) => {}
                    Ok(n) => println!("log:  synced {n} record(s), {} total", log.len()),
                    Err(e) => println!("log:  sync FAILED -- {e}"),
                }
            }
        }

        // Re-save the clock periodically, so a power cut costs only the time the
        // device spent off rather than everything since it was last told.
        if let Some(epoch) = clock::now() {
            if now_ms() / 1000 - last_clock_save_s >= clock::SAVE_INTERVAL_S {
                last_clock_save_s = now_ms() / 1000;
                if let Err(e) = clock::save(epoch) {
                    println!("time: periodic save failed -- {e}");
                }
            }
        }

        // The fuel gauge, on its own slow cadence -- it is an I2C transaction
        // for values that move over hours -- and the clock policy that follows
        // from it (§7).
        if now_ms().saturating_sub(last_gauge_ms) >= GAUGE_POLL_S * 1000 {
            last_gauge_ms = now_ms();
            reading = gauge.and_then(|g| g.read(i2c).ok());
            if let Some(reading) = reading {
                let now_s = now_ms() / 1000;
                match trend.as_mut() {
                    Some(trend) => trend.observe(reading.millivolts, now_s),
                    None => trend = Some(battery::Trend::new(reading.millivolts, now_s)),
                }

                // The long-baseline accumulator behind the runtime estimate.
                // `CRATE` cannot supply one -- on the frugal policy this cell
                // drains at ~0.14 %/hr against a gauge whose LSB is 0.208, so
                // the register reads a hard zero for the whole run. Millivolts
                // over hours is the only measurement left, and it lives here
                // rather than in the redraw path because it must see *every*
                // sample: a rate assembled from the handful of polls that
                // happened to coincide with a repaint would be an average of
                // nothing in particular.
                //
                // `previous_mv` is deliberately RAM-only. Persisting it would
                // cost a flash write every ten seconds to protect a value the
                // very next poll re-establishes.
                match previous_mv {
                    None => previous_mv = Some(reading.millivolts),
                    Some(previous) => {
                        let elapsed = now_s.saturating_sub(last_drain_s);
                        let (next, reset) =
                            battery::drained(drain, previous, reading.millivolts, elapsed);
                        previous_mv = Some(reading.millivolts);
                        last_drain_s = now_s;
                        drain = next;

                        // A reset is the one event worth both saying and saving
                        // immediately: it throws away the baseline, and a power
                        // cut that restored the discarded one would put a stale
                        // rate back into service.
                        if reset {
                            println!("bat:  charging or rebounding -- discharge baseline reset");
                            if let Err(e) = settings::store_battery_drain(drain) {
                                println!("bat:  baseline reset but NOT saved -- {e}");
                            }
                            last_drain_save_s = now_s;
                        } else if now_s.saturating_sub(last_drain_save_s)
                            >= battery::DRAIN_SAVE_S
                        {
                            last_drain_save_s = now_s;
                            if let Err(e) = settings::store_battery_drain(drain) {
                                println!("bat:  baseline NOT saved -- {e}");
                            }
                        }
                    }
                }
            }
            if let Some(reading) = reading {
                println!(
                    "bat:  {} mV, {}%, rate {}.{:02} %/hr",
                    reading.millivolts,
                    reading.percent,
                    reading.crate_centi_per_hour / 100,
                    (reading.crate_centi_per_hour % 100).abs()
                );
            }

            // Skipped entirely while pinned -- otherwise the next tick would
            // quietly undo the override and the console would go away again,
            // which is the exact problem `freq` exists to solve.
            let want = power::decide(now_ms() / 1000, last_console_s);
            if freq_override.is_none() && want != policy {
                match power::apply(want) {
                    Ok(()) => {
                        policy = want;
                        match power::config() {
                            Some((max, min, sleep)) => println!(
                                "pm:   -> {} -- {}/{} MHz, light sleep {}",
                                policy.label(),
                                min,
                                max,
                                if sleep { "on" } else { "off" }
                            ),
                            None => println!("pm:   -> {} (read-back failed)", policy.label()),
                        }
                    }
                    Err(e) => println!("pm:   could not switch to {} -- {e}", want.label()),
                }
            }
        }

        // --- §4.2's noise auto-tune ----------------------------------------
        //
        // The asymmetry is the whole design: up by one per BATCH that heard
        // anything -- not per event, which saturates the ladder in a second and
        // is a counter racing the interrupt rate rather than tuning -- and down
        // by one only after a full minute of silence. Quick to defend, slow to
        // relax: a storm's first strike should not arrive into a receiver that
        // spent the afternoon relaxing toward a floor it will have to climb
        // straight back up.
        // Frozen while `sensitive on` is in force: the override sits below the
        // ladder's floor, so the first disturber would otherwise climb straight
        // off it and undo exactly what was asked for.
        // Everything this batch heard goes into the window. The window is both
        // the measurement and the decision -- the rate on screen used to come
        // from a separate 5-minute probe, so it read `0/min` while the ladder
        // was visibly climbing on events it had just counted.
        window_events += batch.noise + batch.disturbers;
        window_disturbers += batch.disturbers;

        // While a sweep runs it sets the window, so `calibrate 60` means 60 s
        // probes even if the ordinary cadence is something else.
        let window_s = match sweep.as_ref() {
            Some(sweep) => sweep.window_s,
            None => MEASURE_INTERVAL_S,
        };

        if !max_sensitivity && now_ms().saturating_sub(tune_window_ms) >= window_s * 1000 {
            tune_window_ms = now_ms();
            // Scaled by the window actually used, so a 60 s probe and a 10 s
            // window both report a per-minute rate rather than a raw count.
            // Multiply before dividing, so a window that is not a whole divisor
            // of 60 still scales correctly instead of collapsing to 1.
            let per_min = |count: u32| count * 60 / window_s.max(1);
            totals.noise_per_min = per_min(window_events);
            totals.disturbers_per_min = per_min(window_disturbers);

            // **The one place "quiet" is decided.** A rate, not a count, so the
            // verdict means the same thing whatever the window length -- see
            // `session::QUIET_PER_MIN` for what testing `== 0` cost.
            let quiet = totals.noise_per_min <= quiet_per_min;

            // A sweep, if one is running, spends this window on the search.
            // Everything below -- the +/-1 walk -- is what happens the rest of
            // the time.
            // Captured before the branch: a sweep that finishes inside it sets
            // `sweep` to `None`, and the +/-1 walk must still be skipped for
            // this window rather than stepping the point the search just chose.
            let sweeping = sweep.is_some();

            if let Some(active) = sweep.as_mut() {
                let tested = point;
                active.record(quiet);
                println!(
                    "cal:  probe {} [{}..{}] {} -> {} -- {} event(s) = {}/min, {}, ~{} left",
                    active.probe,
                    active.low,
                    active.high,
                    tested.raw(),
                    session::describe(tested),
                    window_events,
                    totals.noise_per_min,
                    if quiet { "quiet" } else { "noisy" },
                    active.remaining()
                );

                let finished = active.done();
                point = match finished {
                    true => active.settled(),
                    false => active.point(),
                };
                if let Err(e) = session::apply(sensor, i2c, point) {
                    println!("cal:  could not program -- {e}");
                }
                // Let the new thresholds take effect before the next window
                // opens, so a probe measures its own setting rather than the
                // tail of the previous one.
                FreeRtos::delay_ms(session::CALIBRATE_SETTLE_MS);
                tune_window_ms = now_ms();

                if finished {
                    println!(
                        "cal:  settled at {}/{} ({}%) -- {}",
                        point.raw(),
                        defence::MAX,
                        point.percent(),
                        session::describe(point)
                    );
                    match settings::store_defence_point(point) {
                        Ok(()) => {
                            stored_point = point;
                            last_point_save_s = now_ms() / 1000;
                            println!("cal:  point stored -- +/-1 from here");
                        }
                        Err(e) => println!("cal:  point NOT stored -- {e}"),
                    }
                    sweep = None;
                }
            }
            // The counters are zeroed once, below, for both paths.

            // **NOT `continue`.** Skipping the rest of the iteration would skip
            // the screen block below, so the panel would hold whatever was on it
            // for the whole sweep -- which is the entire reason the sweep stopped
            // being one long blocking call.
            //
            // **One step of the packed number per window.** Noisy climbs,
            // quiet relaxes, and which register that lands on is decided by the
            // bit layout rather than by a cursor -- see `defence`.
            //
            // Worth knowing at the console: the bottom two bits are
            // `MIN_NUM_LIGH`, so the very first noisy step moves it from 0 to 1
            // and the chip begins waiting for five strikes. That is the chosen
            // trade for a tuner that walks the whole number; `describe` spells
            // the strike count out on every line so it is never a surprise.
            let raw = point.raw();
            let moved = if sweeping {
                // The search owns this window; it has already moved the point.
                None
            } else if !quiet {
                match raw < defence::MAX {
                    true => {
                        point = defence::Point::new(raw + 1);
                        Some("up")
                    }
                    false => None,
                }
            } else {
                // `relaxed`, not `raw - 1`: see `defence::Point::relaxed`. A
                // decrement borrows across fields and lands deafer than it
                // started, which is how a quiet room used to walk itself into
                // waiting for sixteen strikes.
                match point.relaxed() {
                    Some(gentler) => {
                        point = gentler;
                        Some("down")
                    }
                    None => None,
                }
            };
            window_events = 0;
            window_disturbers = 0;

            // Programmed only when it actually moved. At either end the decision
            // is taken every window and changes nothing.
            if let Some(direction) = moved {
                tune(sensor, i2c, point, direction);
            }

            // Persist the point, rarely. The machine can move every window, and
            // a flash write at that cadence to protect a value that re-learns in
            // minutes would spend endurance for nothing -- but a settled room
            // stops moving, so in practice this writes once and then never.
            let now_s = now_ms() / 1000;
            if point != stored_point
                && now_s.saturating_sub(last_point_save_s) >= DEFENCE_SAVE_S
            {
                last_point_save_s = now_s;
                match settings::store_defence_point(point) {
                    Ok(()) => stored_point = point,
                    Err(e) => println!("as:   defence point NOT saved -- {e}"),
                }
            }
        }

        // --- the screen ---------------------------------------------------
        //
        // Change-gated, with a floor and a backstop. The panel takes ~3.9 s per
        // refresh, so "redraw when anything might have changed" is not an option
        // -- it would be busy most of the time and every image would be stale
        // before it finished.
        if let Some(panel) = panel.as_deref_mut() {
            let want = Drawn {
                strikes: totals.strikes,
                last_strike: totals.last_strike,
                location: *location,
                defence: point.raw() as u32,
            };
            let since_draw_s = now_ms().saturating_sub(last_draw_ms) / 1000;
            let changed = drawn.as_ref() != Some(&want);
            let stale = since_draw_s >= REDRAW_BASELINE_S;

            // **A button press bypasses the rate limit.** The 30 s floor exists
            // to stop the panel being pinned by things that change on their own;
            // a person pressing the only button on the device is not one of
            // those. Waiting out the floor made a press taken shortly after any
            // other redraw appear to do nothing for half a minute, which reads
            // as a broken button rather than as a considered refresh policy.
            //
            // The worst case is bounded anyway: `show` blocks for the whole
            // refresh, so a second press during one is handled after it rather
            // than queued on top of it.
            let allowed = user_acted || since_draw_s >= REDRAW_MIN_GAP_S;

            if (changed || stale) && allowed {
                // Uses the cached reading from the poll above rather than
                // taking a second one: it is at most GAUGE_POLL_S old, against a
                // value that moves over hours.
                //
                // Shadowing it with a fresh read here was also what made the
                // outer binding look unused to the compiler -- the warning was
                // pointing at a real duplicate transaction, not at a style
                // preference.

                // Widen the learned range on the way past. Written to NVS only
                // when an endpoint actually moves, which the midpoint rule makes
                // rare: it takes a NEW extreme, and new extrema in a noisy
                // series get rarer the longer it runs.
                if let Some(reading) = reading {
                    if let Some(moved) = battery::widened(range, reading.millivolts) {
                        match settings::store_battery_range(moved.0, moved.1) {
                            Ok(()) => {
                                println!(
                                    "bat:  range {}-{} -> {}-{} mV",
                                    range.0, range.1, moved.0, moved.1
                                );
                                range = moved;
                            }
                            Err(e) => println!("bat:  range moved but NOT saved -- {e}"),
                        }
                    }
                }

                // Flatten the ring just before drawing it, so the chart shows
                // the state at draw time rather than whenever it last changed.
                let (chart_len, chart_capacity) = fill_chart(
                    &history,
                    chart_period,
                    &mut chart_counts,
                    &mut chart_scores,
                );

                redraw(
                    panel,
                    &ui::Status {
                        location: *location,
                        health: system::health(
                            die_temperature,
                            strike_log.as_deref().map(|l| (l.free_bytes(), l.used_bytes())),
                        ),
                        battery: reading,
                        battery_flow: match reading {
                            Some(reading) => battery::flow(&reading, trend.as_ref()),
                            None => battery::Flow::Unknown,
                        },
                        now: clock::now(),
                        uptime_minutes: now_ms() / 60_000,
                        antenna_khz,
                        irq_confirmed,
                        defence_level: point.percent(),
                        defence_max: 100,
                        noise_per_min: totals.noise_per_min,
                        strikes_total: totals.strikes,
                        last_strike: totals.last_strike,
                        disturbers_per_min: totals.disturbers_per_min,
                        last_hour: history.last_hour(),
                        battery_range: range,
                        battery_drain: drain,
                        chart_period,
                        chart_counts: &chart_counts[..chart_len],
                        chart_scores: &chart_scores[..chart_len],
                        chart_capacity,
                        light_sleep: power::config().map(|(_, _, ls)| ls).unwrap_or(false),
                    },
                    if changed { "content changed" } else { "baseline" },
                );
                drawn = Some(want);
                last_draw_ms = now_ms();
                user_acted = false;
            }
        }


        batch = Batch::default();
        batch_started = now_ms();
    }
}

/// Render and push one status screen.
fn redraw(panel: &mut display::Panel<'_>, status: &ui::Status<'_>, why: &str) {
    let mut frame = display::Panel::frame();
    ui::status(&mut frame, status);

    let started = now_ms();
    match panel.show(&frame) {
        Ok(0) => println!("epd:  *** sent, but BUSY never fell -- nothing was drawn ***"),
        Ok(busy_ms) => println!(
            "epd:  redrawn ({why}) -- {} ms total, panel busy {} ms",
            now_ms().saturating_sub(started),
            busy_ms
        ),
        Err(e) => println!("epd:  draw FAILED -- {e}"),
    }
}

/// Milliseconds since boot.
fn now_ms() -> u32 {
    (unsafe { esp_idf_hal::sys::esp_timer_get_time() } / 1000) as u32
}

/// Drive the IRQ pin from the antenna oscillator and count the edges.
///
/// **This answers a question that silence cannot.** A lightning detector that
/// reports nothing is indistinguishable from one whose IRQ is on the wrong pad —
/// both look like a quiet day, and the second only reveals itself during the
/// storm you built the thing for. §2's own pin table says GPIO21 while §2.1's
/// diagram and §10's build order say GPIO20, so the ambiguity is real.
///
/// With `DISP_LCO` set the sensor puts LCO ÷ 16 on the pin — roughly 31 kHz. So
/// counting transitions for a fixed window says two things at once:
///
/// * **zero edges** → the pin is not connected to this sensor;
/// * **the rate** → the antenna's resonant frequency, which §3 step 5 otherwise
///   defers to a scope. It should be 500 kHz ±3.5 %.
///
/// Polled rather than interrupt-driven on purpose: 31 000 interrupts a second
/// would swamp the notifier this loop shares with the button, and the whole
/// measurement is over in a tenth of a second.
fn antenna_self_test(
    sensor: &As3935,
    i2c: &mut I2cDriver<'_>,
    irq: &PinDriver<'_, esp_idf_hal::gpio::Input>,
) -> (bool, u32) {
    const WINDOW_MS: u32 = 100;

    if let Err(e) = sensor.set_irq_display_lco(i2c) {
        println!("self: could not enable LCO output -- {e}");
        return (false, 0);
    }

    // Settle: the pin has just changed function, and the first transitions
    // after that are not the oscillator.
    FreeRtos::delay_ms(10);

    let started = unsafe { esp_idf_hal::sys::esp_timer_get_time() };
    let window_us = (WINDOW_MS as i64) * 1000;
    let mut edges: u32 = 0;
    let mut previous = irq.is_high();

    while unsafe { esp_idf_hal::sys::esp_timer_get_time() } - started < window_us {
        let now = irq.is_high();
        if now && !previous {
            edges += 1;
        }
        previous = now;
    }

    if let Err(e) = sensor.clear_irq_display(i2c) {
        println!("self: could not restore the IRQ pin -- {e}");
    }

    if edges == 0 {
        println!("self: *** NO EDGES on the IRQ pin while the sensor drove it at ~31 kHz ***");
        println!("self:     The IRQ wire is not on GPIO21 (D6). The other candidate is");
        println!("self:     GPIO20 (D7), on the opposite side of the board.");
        println!("self:     Until this passes, 'no strikes' means nothing.");
        return (false, 0);
    }

    // edges over WINDOW_MS -> Hz on the pin -> x16 for the antenna, in kHz.
    let pin_hz = edges * 1000 / WINDOW_MS;
    let antenna_khz = pin_hz * as3935::LCO_DIVISOR / 1000;

    // Tolerance is carried in tenths of a percent to stay in integers.
    let tolerance_khz =
        as3935::ANTENNA_NOMINAL_KHZ * as3935::ANTENNA_TOLERANCE_PERCENT / 1000;
    let low = as3935::ANTENNA_NOMINAL_KHZ - tolerance_khz;
    let high = as3935::ANTENNA_NOMINAL_KHZ + tolerance_khz;

    println!("self: {edges} edges in {WINDOW_MS} ms -> antenna {antenna_khz} kHz");
    if (low..=high).contains(&antenna_khz) {
        println!("self: IRQ wire confirmed, antenna in tune ({low}-{high} kHz)");
    } else {
        println!("self: IRQ wire confirmed, but antenna is OUT OF TUNE");
        println!("self:     wanted {low}-{high} kHz -- adjust TUNING_CAPS_PF (§3 step 5)");
    }
    (true, antenna_khz)
}


/// Was the button held long enough to be a person rather than a USB host?
///
/// See [`BUTTON_HOLD_MS`] for why duration is the only signal that separates
/// them: a host asserting DTR drives GPIO9 low exactly as a fingertip does, and
/// GPIO9 is the only button on this board.
///
/// Returns as soon as the pin comes back up, so a rejected press costs nothing
/// — which matters because the rejected ones are every flash attempt.
fn button_held(button: &PinDriver<'_, esp_idf_hal::gpio::Input>) -> bool {
    let mut held_ms = 0;
    while held_ms < BUTTON_HOLD_MS {
        if !button.is_low() {
            return false;
        }
        FreeRtos::delay_ms(BUTTON_POLL_MS);
        held_ms += BUTTON_POLL_MS;
    }
    button.is_low()
}


/// Flatten whichever ring the chart period selects, returning how many buckets
/// were written.
///
/// One buffer sized for the longest ring rather than three: the rings differ in
/// length but not in what a chart does with them, and a prefix is cheaper than
/// three allocations that are each idle two thirds of the time.
/// Returns `(live, capacity)`: how many buckets hold data, and how many the
/// ring holds when full.
///
/// The chart needs both. `capacity` fixes the column width so bars do not
/// change size as the ring fills, and `live` says how many to draw — which is
/// what makes a chart fill from the left and only scroll once it is full.
fn fill_chart(
    history: &history::History,
    period: ui::ChartPeriod,
    counts: &mut [u16],
    scores: &mut [u32],
) -> (usize, usize) {
    // One generic helper rather than three arms differing only in a const: the
    // rings have different lengths but a chart does the same thing to all of
    // them, and three near-identical blocks is three places to fix a bug.
    fn flatten<const N: usize>(
        ring: &history::Ring<N>,
        counts: &mut [u16],
        scores: &mut [u32],
    ) -> (usize, usize) {
        let mut c = [0u16; N];
        let mut s = [0u32; N];
        let live = history::series_of(ring, &mut c, &mut s);
        counts[..N].copy_from_slice(&c);
        scores[..N].copy_from_slice(&s);
        (live, N)
    }

    match period {
        ui::ChartPeriod::Day => flatten(&history.day, counts, scores),
        ui::ChartPeriod::Week => flatten(&history.week, counts, scores),
        ui::ChartPeriod::Month => flatten(&history.month, counts, scores),
    }
}

/// The current bucket index for the history rings: minutes since the Unix
/// epoch.
///
/// Absolute rather than since-boot, so a record replayed from the CSV lands in
/// the bucket it actually belongs to. Falls back to uptime when the clock has
/// never been set — self-correcting, because setting the clock later jumps the
/// index by decades and a jump larger than a ring clears it, which is right:
/// nothing recorded before the device knew the time can be placed anyway.
fn minute_now() -> u32 {
    match clock::now() {
        Some(epoch) => (epoch / 60) as u32,
        None => now_ms() / 60_000,
    }
}
