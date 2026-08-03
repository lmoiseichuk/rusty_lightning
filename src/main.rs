//! Lightning-detector terminal — bring-up.
//!
//! Build order steps 1–3 (§10): prove the toolchain and console, find who is on
//! the I2C bus, then bring the AS3935 up and decode its interrupts.

mod as3935;
mod battery;
mod display;
mod defence;
mod history;
mod i2c_scan;
mod power;
mod settings;
mod storage;
mod system;
mod ui;

use std::num::NonZeroU32;

use esp_idf_hal::delay::{FreeRtos, TickType};
use esp_idf_hal::gpio::{InterruptType, PinDriver, Pull};
use esp_idf_hal::i2c::{I2cConfig, I2cDriver};
use esp_idf_hal::peripherals::Peripherals;
use esp_idf_hal::task::notification::Notification;
use esp_idf_hal::units::Hertz;

use as3935::{As3935, Interrupt, Location};

/// I2C bus speed.
///
/// 100 kHz rather than the 400 kHz the MicroPython reference uses. This is
/// bring-up over hand-soldered wires and a QT cable chain, where the bus
/// capacitance is unknown and a marginal rise time shows up as intermittent
/// NACKs that look exactly like a missing device. Neither part needs the speed:
/// §3's register accesses are single bytes.
const I2C_HZ: u32 = 100_000;

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

/// Quiet time before the noise floor relaxes by one (§4.2).
const NOISE_DECAY_S: u32 = 60;

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

    let peripherals = match Peripherals::take() {
        Ok(peripherals) => peripherals,
        Err(e) => {
            println!("FATAL: peripherals unavailable -- {e}");
            return;
        }
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

    let mut irq = match PinDriver::input(peripherals.pins.gpio21, Pull::Down) {
        Ok(pin) => pin,
        Err(e) => {
            println!("FATAL: GPIO21 would not become an input -- {e}");
            return;
        }
    };

    // The USB-serial-JTAG console enumerates when the host opens the port, a
    // moment or two after boot. Anything printed before then goes into a FIFO
    // nobody is draining, so the banner would be the one thing never seen.
    FreeRtos::delay_ms(2000);

    println!();
    println!("=== lightning terminal ===");
    println!("fw {}", env!("CARGO_PKG_VERSION"));
    println!("boot: {}", system::reset_reason_name());

    // §2: the display consumes GPIO 2, 3, 4, 5, 8 and 10, which leaves the
    // XIAO's native I2C pads free. These are fixed by that, not chosen.
    let config = I2cConfig::new().baudrate(Hertz(I2C_HZ));
    let mut i2c = match I2cDriver::new(
        peripherals.i2c0,
        peripherals.pins.gpio6,
        peripherals.pins.gpio7,
        &config,
    ) {
        Ok(i2c) => i2c,
        Err(e) => {
            println!("FATAL: I2C0 would not initialise -- {e}");
            return;
        }
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
    let sensor = match As3935::find(&mut i2c) {
        Some(sensor) => sensor,
        None => {
            println!("FATAL: no AS3935 answered a reset at 0x01/0x02/0x03");
            return;
        }
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

    if let Err(e) = configure(&sensor, &mut i2c, location) {
        println!("FATAL: sensor configuration failed -- {e}");
        return;
    }

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
    let mut button = match PinDriver::input(peripherals.pins.gpio9, Pull::Up) {
        Ok(pin) => pin,
        Err(e) => {
            println!("FATAL: GPIO9 would not become an input -- {e}");
            return;
        }
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
    );
}

/// §3's init sequence, in the order the datasheet and the reference agree on.
fn configure(
    sensor: &As3935,
    i2c: &mut I2cDriver<'_>,
    location: Location,
) -> Result<(), esp_idf_hal::sys::EspError> {
    sensor.power_up(i2c)?;
    sensor.set_location(i2c, location)?;

    // Disturbers stay ENABLED. It looks wrong for a lightning detector and is
    // not: §4.2's noise auto-tune is driven by disturber events, so masking
    // them leaves the floor pinned wherever it started.
    sensor.set_disturber_enabled(i2c, true)?;
    sensor.clear_irq_display(i2c)?;
    FreeRtos::delay_ms(500);

    sensor.set_tuning_caps(i2c, TUNING_CAPS_PF)?;
    apply_defence(sensor, i2c, 0)?;

    // Explicit rather than relying on the reset default. One strike reports
    // immediately; the alternatives make the sensor wait for a pattern before
    // saying anything, and the first four strikes of a real storm are exactly
    // the ones an early-warning device cannot afford to sit on.
    let min_strikes = sensor.set_min_strikes(i2c, 1)?;

    // Read the noise floor back rather than trusting the write. Every register
    // access here is a read-modify-write over I2C, and a bus that NACKs
    // mid-sequence leaves the sensor running settings nobody chose -- which
    // presents as "the auto-tune does nothing", one of the harder things to
    // notice from the outside.
    let readback = sensor.noise_floor(i2c)?;
    let expected = defence::settings(0).noise_floor;
    if readback != expected {
        println!("as:   ⚠ noise floor read back as {readback}, expected {expected}");
    }

    println!(
        "as:   {}, {} pF, defence 0/{} ({}), report after {} strike(s)",
        location.label(),
        TUNING_CAPS_PF,
        defence::MAX_LEVEL,
        defence::rung(0),
        min_strikes
    );
    Ok(())
}

/// Push one defence level into the sensor's three registers.
fn apply_defence(
    sensor: &As3935,
    i2c: &mut I2cDriver<'_>,
    level: u8,
) -> Result<(), esp_idf_hal::sys::EspError> {
    let settings = defence::settings(level);
    sensor.set_noise_floor(i2c, settings.noise_floor)?;
    sensor.set_watchdog_threshold(i2c, settings.watchdog)?;
    sensor.set_spike_rejection(i2c, settings.spike_reject)
}

/// What one batch window heard.
#[derive(Default)]
struct Batch {
    strikes: u32,
    disturbers: u32,
    noise: u32,
    unknown: u32,
}

impl Batch {
    fn heard_interference(&self) -> bool {
        self.disturbers > 0 || self.noise > 0
    }

    fn is_empty(&self) -> bool {
        self.strikes == 0 && !self.heard_interference() && self.unknown == 0
    }
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
) {
    let mut level: u8 = 0;
    let mut quiet_ms: u32 = 0;
    let mut last_button_ms: u32 = 0;
    // Set by a button press, cleared by the redraw it causes.
    let mut user_acted = false;
    let mut batch = Batch::default();
    let mut batch_started = now_ms();

    // Running totals, and what the glass currently shows. The screen is redrawn
    // when the two disagree -- never on a timer alone.
    let mut totals = Totals::default();
    let mut history = history::History::new();

    // §2.1's learned range. Seeded rather than empty, so the first reading has
    // something to widen from -- and `None` from NVS is a virgin device, not an
    // error.
    let mut range = settings::battery_range().unwrap_or(battery::SEED_RANGE);

    // §7's clock policy. Starts on the USB assumption -- the device is usually
    // plugged in, and being wrong that way costs power rather than a console.
    let mut policy = power::Policy::Usb;
    // The most recent gauge reading, or None until the first poll or if the
    // gauge is absent. Cached because the screen wants it far less often than
    // the loop runs, and it is an I2C transaction.
    let mut reading: Option<battery::Reading>;
    // Read once up front rather than waiting out the first poll interval, so
    // the very first screen carries a real battery figure instead of "no gauge".
    reading = gauge.and_then(|g| g.read(i2c).ok());
    let mut last_gauge_ms: u32 = now_ms();
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
                collect(sensor, i2c, &mut batch, &mut totals, &mut history, now_ms() / 60_000);
            }
        }

        // Not yet at the end of the window -- go back and keep listening.
        if now_ms().saturating_sub(batch_started) < BATCH_MS {
            continue;
        }

        report(&batch);

        // Keep the rings' idea of "now" current even in a lull, so a chart drawn
        // during quiet weather shows the quiet rather than the last storm shoved
        // against its right edge.
        history.tick(now_ms() / 60_000);

        // The fuel gauge, on its own slow cadence -- it is an I2C transaction
        // for values that move over hours -- and the clock policy that follows
        // from it (§7).
        if now_ms().saturating_sub(last_gauge_ms) >= GAUGE_POLL_S * 1000 {
            last_gauge_ms = now_ms();
            reading = gauge.and_then(|g| g.read(i2c).ok());
            if let Some(reading) = reading {
                println!(
                    "bat:  {} mV, {}%, rate {}.{:02} %/hr",
                    reading.millivolts,
                    reading.percent,
                    reading.crate_centi_per_hour / 100,
                    (reading.crate_centi_per_hour % 100).abs()
                );
            }

            let want = power::decide(reading.map(|r| r.crate_centi_per_hour), now_ms() / 1000);
            if want != policy {
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
        if batch.heard_interference() {
            quiet_ms = 0;
            if level < defence::MAX_LEVEL {
                level += 1;
                tune(sensor, i2c, level, "up");
            }
        } else if batch.is_empty() {
            quiet_ms += BATCH_MS;
            if quiet_ms >= NOISE_DECAY_S * 1000 && level > 0 {
                quiet_ms = 0;
                level -= 1;
                tune(sensor, i2c, level, "down");
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
                defence: level,
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

                redraw(
                    panel,
                    &ui::Status {
                        location: *location,
                        health: system::health(die_temperature),
                        battery: reading,
                        uptime_minutes: now_ms() / 60_000,
                        antenna_khz,
                        irq_confirmed,
                        defence_level: level,
                        defence_max: defence::MAX_LEVEL,
                        strikes_total: totals.strikes,
                        last_strike: totals.last_strike,
                        disturbers_total: totals.disturbers,
                        last_hour: history.last_hour(),
                        battery_range: range,
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

/// Everything counted since boot.
#[derive(Default)]
struct Totals {
    strikes: u32,
    disturbers: u32,
    last_strike: Option<(as3935::Distance, u32)>,
}

/// The subset of state the screen is redrawn for.
///
/// **Strikes and the mode button vote. Nothing else.** Everything else rides
/// the baseline
/// redraw, and that is a deliberate narrowing rather than an oversight:
///
/// * the **disturber count** moves every second in a noisy room;
/// * the **defence level** oscillates across a rung whenever the environment
///   sits near a threshold — measured here, 0→1→0 inside ninety seconds, which
///   at a 30 s floor is a 3.9 s refresh every half minute, forever;
/// * **battery, temperature and heap** move far slower than the baseline.
///
/// Each of those would pin the panel busy to report something nobody is waiting
/// on. A strike is the one event this device exists to announce, and it is the
/// one thing worth spending four seconds of panel time on the moment it
/// happens. Same conclusion the moisture project reached about its pack
/// voltage, arrived at from the opposite direction.
///
/// **The mode is here for the opposite reason.** It cannot churn — it changes
/// only when somebody deliberately presses the one button on the device — and
/// that press is exactly when a person is standing in front of the glass
/// waiting to see whether it worked. A setting that takes fifteen minutes to
/// appear reads as a button that does not work.
#[derive(PartialEq)]
struct Drawn {
    strikes: u32,
    last_strike: Option<(as3935::Distance, u32)>,
    location: Location,
    /// §4.2's level. **A change test, so 11 -> 11 cannot repaint** — only a
    /// genuine move does, and the 30 s floor bounds how often that can happen.
    ///
    /// It was excluded for a while after it caused a refresh every half minute
    /// in a marginal room, oscillating 0 -> 1 -> 0. That was the ladder sitting
    /// exactly on a threshold, which is the one place it does not settle.
    /// Everywhere else it **converges**: it climbs until the noise is rejected
    /// and then stops, so the churn is a property of one environment rather
    /// than of the value.
    defence: u8,
}

/// Render and push one status screen.
fn redraw(panel: &mut display::Panel<'_>, status: &ui::Status, why: &str) {
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

/// Read one interrupt and fold it into the batch.
fn collect(
    sensor: &As3935,
    i2c: &mut I2cDriver<'_>,
    batch: &mut Batch,
    totals: &mut Totals,
    history: &mut history::History,
    minute: u32,
) {
    // The datasheet's settle time, and the reason this is in the main task
    // rather than the ISR (§3).
    FreeRtos::delay_ms(as3935::IRQ_SETTLE_MS);

    let reason = match sensor.interrupt_reason(i2c) {
        Ok(reason) => reason,
        Err(e) => {
            println!("irq:  reason read failed -- {e}");
            return;
        }
    };

    match reason {
        Interrupt::Lightning => {
            batch.strikes += 1;
            totals.strikes += 1;
            // Strikes are reported individually and immediately. They are the
            // point of the device, they are rare, and a summary line would hide
            // the distance and energy that §4.3 needs.
            match sensor.strike(i2c) {
                Ok(strike) => {
                    totals.last_strike = Some((strike.distance, strike.intensity_milli()));
                    history.record(minute, &strike);
                    println!(
                        "STRIKE  {:?}  energy {} (intensity {}.{:03})",
                        strike.distance,
                        strike.energy_raw,
                        strike.intensity_milli() / 1000,
                        strike.intensity_milli() % 1000
                    )
                }
                Err(e) => println!("STRIKE  -- but the detail read failed: {e}"),
            }
        }
        Interrupt::Disturber => {
            batch.disturbers += 1;
            totals.disturbers += 1;
        }
        Interrupt::NoiseTooHigh => batch.noise += 1,
        Interrupt::Unknown(_) => batch.unknown += 1,
    }
}

/// One line per batch that heard interference. Silent otherwise — a quiet
/// device should be quiet, and the earlier per-event printing made the console
/// unreadable in a noisy room.
fn report(batch: &Batch) {
    if !batch.heard_interference() && batch.unknown == 0 {
        return;
    }
    println!(
        "batch: {} disturber(s), {} noise, {} unknown",
        batch.disturbers, batch.noise, batch.unknown
    );
}

/// Move the defence level and say what it did in terms of the knob it is on.
fn tune(sensor: &As3935, i2c: &mut I2cDriver<'_>, level: u8, direction: &str) {
    let settings = defence::settings(level);
    match apply_defence(sensor, i2c, level) {
        Ok(()) => println!(
            "tune: {direction} to {level}/{} -- {} (nf {}, wdth {}, srej {})",
            defence::MAX_LEVEL,
            defence::rung(level),
            settings.noise_floor,
            settings.watchdog,
            settings.spike_reject
        ),
        Err(e) => println!("tune: could not move to level {level} -- {e}"),
    }
}

/// Switch indoor/outdoor, apply it to the sensor, and remember it.
///
/// The order matters: the sensor is told first, and NVS is written only if that
/// succeeded. The reverse would leave a device that reports one mode on screen
/// and runs the other -- and the stored value would survive the reboot that
/// would otherwise have fixed it.
fn toggle_location(sensor: &As3935, i2c: &mut I2cDriver<'_>, location: &mut Location) {
    let next = match location {
        Location::Indoor => Location::Outdoor,
        Location::Outdoor => Location::Indoor,
    };

    if let Err(e) = sensor.set_location(i2c, next) {
        println!("btn:  could not switch to {} -- {e}", next.label());
        return;
    }
    *location = next;

    match settings::store_location(next) {
        Ok(()) => println!("btn:  switched to {} (saved)", next.label()),
        // Worth saying out loud rather than swallowing: the device is running
        // the new mode but will forget it, which is the sort of thing that
        // wastes an afternoon the next time it is power-cycled.
        Err(e) => println!("btn:  switched to {} but NOT saved -- {e}", next.label()),
    }
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
