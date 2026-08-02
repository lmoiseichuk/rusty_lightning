//! Lightning-detector terminal — bring-up.
//!
//! Build order steps 1–3 (§10): prove the toolchain and console, find who is on
//! the I2C bus, then bring the AS3935 up and decode its interrupts.

mod as3935;
mod i2c_scan;
mod settings;
mod storage;

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

/// Notification values, so one wait serves two sources.
///
/// `Notification::wait` hands back the value the notifier posted, which makes a
/// second task or a second queue unnecessary — the loop stays single-threaded
/// and the two events cannot race each other's handling.
const NOTIFY_STRIKE: u32 = 1;
const NOTIFY_BUTTON: u32 = 2;

/// How long to ignore further button edges after one is accepted.
///
/// A tactile switch bounces for a few milliseconds; 300 ms also stops a
/// deliberate double-press from toggling twice and landing back where it
/// started, which would look like the button not working at all.
const BUTTON_DEBOUNCE_MS: u32 = 300;

/// Antenna tuning capacitance, picofarads. The SEN0290's factory value, and
/// changing it needs a scope on the IRQ pin (§3 step 5).
const TUNING_CAPS_PF: u8 = 120;

/// §3 step 6's starting points.
const NOISE_FLOOR_START: u8 = 0;
const WATCHDOG_THRESHOLD: u8 = 2;
const SPIKE_REJECTION: u8 = 0;

/// How long to collect events before summarising them (§4.2's "~1 s batch").
const BATCH_MS: u32 = 1000;

/// Quiet time before the noise floor relaxes by one (§4.2).
const NOISE_DECAY_S: u32 = 60;

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
            notifier.notify_and_yield(NonZeroU32::new(1).unwrap());
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
    antenna_self_test(&sensor, &mut i2c, &irq);

    println!("irq:  GPIO21 (D6), rising edge, pulldown");
    println!("btn:  GPIO9 (BOOT), press to switch indoor/outdoor");
    println!("as:   running {}", location.label());
    println!("--- listening ---");

    listen(
        &sensor,
        &mut i2c,
        &mut irq,
        &mut button,
        &notification,
        &mut location,
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
    sensor.set_noise_floor(i2c, NOISE_FLOOR_START)?;
    sensor.set_watchdog_threshold(i2c, WATCHDOG_THRESHOLD)?;
    sensor.set_spike_rejection(i2c, SPIKE_REJECTION)?;

    println!(
        "as:   {}, {} pF, noise floor {}, watchdog {}, spike reject {}",
        location.label(),
        TUNING_CAPS_PF,
        sensor.noise_floor(i2c)?,
        WATCHDOG_THRESHOLD,
        SPIKE_REJECTION
    );
    Ok(())
}

/// The event loop: wait for an edge, decode it, and keep the noise floor tuned.
fn listen(
    sensor: &As3935,
    i2c: &mut I2cDriver<'_>,
    irq: &mut PinDriver<'_, esp_idf_hal::gpio::Input>,
    button: &mut PinDriver<'_, esp_idf_hal::gpio::Input>,
    notification: &Notification,
    location: &mut Location,
) {
    let mut noise_floor = NOISE_FLOOR_START;
    let mut quiet_ms: u32 = 0;
    let mut button_blanked_ms: u32 = 0;

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

        // Wait with a timeout rather than blocking forever, because §4.2's decay
        // is driven by the *absence* of events -- a loop that only wakes on an
        // edge can never notice that nothing happened.
        let woke = notification.wait(TickType::new_millis(BATCH_MS as u64).into());

        let Some(source) = woke else {
            quiet_ms += BATCH_MS;
            button_blanked_ms = button_blanked_ms.saturating_sub(BATCH_MS);
            if quiet_ms >= NOISE_DECAY_S * 1000 && noise_floor > 0 {
                noise_floor -= 1;
                quiet_ms = 0;
                match sensor.set_noise_floor(i2c, noise_floor) {
                    Ok(()) => println!(
                        "tune: quiet for {NOISE_DECAY_S} s -- noise floor down to {noise_floor}"
                    ),
                    Err(e) => println!("tune: could not lower the noise floor -- {e}"),
                }
            }
            continue;
        };

        if source.get() == NOTIFY_BUTTON {
            if button_blanked_ms > 0 {
                continue;
            }
            button_blanked_ms = BUTTON_DEBOUNCE_MS;
            toggle_location(sensor, i2c, location);
            continue;
        }

        // Anything else is the sensor. Not `== NOTIFY_STRIKE` on purpose: a
        // notification value this loop does not recognise is far more likely to
        // be a bug in the notifier than a real third source, and dropping it
        // silently would lose a strike.
        let _ = NOTIFY_STRIKE;
        quiet_ms = 0;

        // The datasheet's settle time, and the reason this is in the main task
        // rather than the ISR (§3).
        FreeRtos::delay_ms(as3935::IRQ_SETTLE_MS);

        let reason = match sensor.interrupt_reason(i2c) {
            Ok(reason) => reason,
            Err(e) => {
                println!("irq:  reason read failed -- {e}");
                continue;
            }
        };

        match reason {
            Interrupt::Lightning => match sensor.strike(i2c) {
                Ok(strike) => println!(
                    "STRIKE  {:?}  energy {} (intensity {}.{:03})",
                    strike.distance,
                    strike.energy_raw,
                    strike.intensity_milli() / 1000,
                    strike.intensity_milli() % 1000
                ),
                Err(e) => println!("STRIKE  -- but the detail read failed: {e}"),
            },

            // §4.2 is asymmetric on purpose: quick to defend, slow to relax.
            // Any disturber or noise event raises the floor immediately.
            Interrupt::Disturber | Interrupt::NoiseTooHigh => {
                let what = if reason == Interrupt::Disturber {
                    "disturber"
                } else {
                    "noise too high"
                };
                if noise_floor < 7 {
                    noise_floor += 1;
                    match sensor.set_noise_floor(i2c, noise_floor) {
                        Ok(()) => println!("tune: {what} -- noise floor up to {noise_floor}"),
                        Err(e) => println!("tune: {what} -- could not raise the floor: {e}"),
                    }
                } else {
                    println!("tune: {what} -- noise floor already at 7, cannot defend further");
                }
            }

            Interrupt::Unknown(bits) => {
                println!("irq:  unexpected reason 0x{bits:02x} -- read too soon after the edge?")
            }
        }
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
) {
    const WINDOW_MS: u32 = 100;

    if let Err(e) = sensor.set_irq_display_lco(i2c) {
        println!("self: could not enable LCO output -- {e}");
        return;
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
        return;
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
}
