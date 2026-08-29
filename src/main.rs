//! Lightning-detector terminal — bring-up.
//!
//! Build order steps 1–3 (§10): prove the toolchain and console, find who is on
//! the I2C bus, then bring the AS3935 up and decode its interrupts.

mod as3935;
mod boot;
mod battery;
mod civil;
mod clock;
mod commands;
mod console;
mod display;
mod effects;
mod defence;
mod history;
mod i2c_scan;
mod listen;
mod log;
mod merger;
mod power;
mod press;
mod screen;
mod session;
mod settings;
mod storage;
mod strike;
mod system;
mod tuning;
mod ui;
mod uptime;
mod csv;
mod verdict;

use std::num::NonZeroU32;

use esp_idf_hal::delay::FreeRtos;
use esp_idf_hal::gpio::{InterruptType, PinDriver, Pull};
use esp_idf_hal::i2c::{I2cConfig, I2cDriver};
use esp_idf_hal::peripherals::Peripherals;
use esp_idf_hal::task::notification::Notification;
use esp_idf_hal::units::Hertz;

use as3935::{As3935, Location};

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
pub const NOTIFY_STRIKE: u32 = 1;
pub const NOTIFY_BUTTON: u32 = 2;

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

    let start_point = match boot::configure(&sensor, &mut i2c, location) {
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
    let (irq_confirmed, antenna_khz) = boot::antenna_self_test(&sensor, &mut i2c, &irq);

    println!("irq:  GPIO21 (D6), rising edge, pulldown");
    println!(
        "btn:  GPIO9 (BOOT), HOLD {} ms to switch indoor/outdoor",
        boot::BUTTON_HOLD_MS
    );
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

    listen::listen(
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
/// Milliseconds since boot.
fn now_ms() -> u32 {
    (unsafe { esp_idf_hal::sys::esp_timer_get_time() } / 1000) as u32
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
