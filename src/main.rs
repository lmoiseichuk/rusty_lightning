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
mod effects;
mod defence;
mod history;
mod i2c_scan;
mod log;
mod power;
mod screen;
mod session;
mod settings;
mod storage;
mod strike;
mod system;
mod tuning;
mod ui;

use std::num::NonZeroU32;

use esp_idf_hal::delay::{FreeRtos, TickType};
use esp_idf_hal::gpio::{InterruptType, PinDriver, Pull};
use esp_idf_hal::i2c::{I2cConfig, I2cDriver};
use esp_idf_hal::peripherals::Peripherals;
use esp_idf_hal::task::notification::Notification;
use esp_idf_hal::units::Hertz;

use as3935::{As3935, Location};
use session::{collect, report, toggle_location, Batch, Drawn, Totals};

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
    let mut last_irq_poll_ms: u32 = now_ms();
    let mut last_button_ms: u32 = 0;
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
    let mut screen = screen::Screen::new();

    let mut fuel = battery::Fuel::new(gauge, i2c, now_ms());
    // §7's clock policy. Starts on the USB assumption -- the device is usually
    // plugged in, and being wrong that way costs power rather than a console.
    let mut policy = power::Policy::Awake;
    // `Some(mhz)` while `freq <mhz>` is in force. Deliberately not persisted:
    // it exists so a board can be watched over USB, and one that came back from
    let mut freq_override: Option<u32> = None;
    let mut console = console::Console::new();
    // Uptime at the last console input, and at the last clock save.
    let mut last_console_s: Option<u32> = None;
    let mut last_clock_save_s: u32 = 0;
    let mut last_log_sync_ms: u32 = 0;
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
    let mut tuning = tuning::Tuning::new(start_point, now_ms());

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
                    screen.user_acted = true;
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
            effects::handle(
                command,
                &mut effects::Hardware {
                    sensor,
                    i2c,
                    gauge,
                    die_temperature,
                    antenna_khz,
                    irq_confirmed,
                },
                &mut effects::Runtime {
                    location,
                    totals: &mut totals,
                    history: &mut history,
                    strike_log: strike_log.as_deref_mut(),
                    tuning: &mut tuning,
                    screen: &mut screen,
                    fuel: &mut fuel,
                    policy: &mut policy,
                    freq_override: &mut freq_override,
                    clock_saved_s: &mut last_clock_save_s,
                },
                now_ms(),
                minute_now(),
            );
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
        if fuel.due(now_ms()) {
            fuel.poll(gauge, i2c, now_ms());

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
        tuning.observe(&batch);

        if tuning.due(now_ms()) {
            tuning.step(sensor, i2c, &mut totals, now_ms());
        }

        // --- the screen ---------------------------------------------------
        //
        // The policy lives in `screen`; what stays here is the one side effect
        // that is not the screen's business -- widening the learned battery
        // range -- kept on this path because it must happen only when a redraw
        // actually does, which is what makes new extrema rare enough to persist.
        if let Some(panel) = panel.as_deref_mut() {
            let want = Drawn {
                strikes: totals.strikes,
                last_strike: totals.last_strike,
                location: *location,
                defence: tuning.point.raw() as u32,
            };

            if let Some(why) = screen.due(&want, now_ms()) {
                // On the redraw path on purpose: a new extreme is only worth a
                // flash write at the panel's cadence, not the gauge's.
                fuel.widen();

                screen.draw(
                    panel,
                    &screen::View {
                        location: *location,
                        point: tuning.point,
                        totals: &totals,
                        history: &history,
                        battery: fuel.reading,
                        trend: fuel.trend.as_ref(),
                        range: fuel.range,
                        drain: fuel.drain,
                        die_temperature,
                        log_bytes: strike_log
                            .as_deref()
                            .map(|l| (l.free_bytes(), l.used_bytes())),
                        antenna_khz,
                        irq_confirmed,
                    },
                    why,
                    now_ms(),
                );
                screen.mark_drawn(want, now_ms());
            }
        }


        batch = Batch::default();
        batch_started = now_ms();
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
