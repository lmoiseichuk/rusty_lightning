//! Bring-up: everything that happens once, before the loop.
//!
//! **Split from `main` because it is a different kind of code.** Bring-up runs
//! once, fails loudly, and is read when something is wrong at start-up; the loop
//! runs forever, must not panic, and is read when something is wrong at three in
//! the morning. Keeping them in one file meant scrolling past three hundred
//! lines of one-time setup to reach the thing that actually runs.
//!
//! What stays in `main` is the part that cannot move: the peripheral handles are
//! lifetime-bound to `Peripherals`, so returning them from a function would need
//! a self-referential struct. `main` therefore owns the hardware and calls in
//! here for the work.

use esp_idf_hal::delay::FreeRtos;
use esp_idf_hal::gpio::PinDriver;
use esp_idf_hal::i2c::I2cDriver;

use crate::as3935::{As3935, Location};
use crate::{as3935, defence, session, settings};


/// Antenna tuning capacitance, picofarads. The SEN0290's factory value, and
/// changing it needs a scope on the IRQ pin (§3 step 5).
pub const TUNING_CAPS_PF: u8 = 120;


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
pub const BUTTON_HOLD_MS: u32 = 1500;

/// How often to check during the hold.
pub const BUTTON_POLL_MS: u32 = 100;


/// §3's init sequence, in the order the datasheet and the reference agree on.
pub fn configure(
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

    // **Written here and nowhere else on the hot path.** Spike rejection is a
    // setting rather than part of the point, so `apply` no longer touches it --
    // which is the whole fix: while the tuner could write it, it walked it to
    // zero on every quiet spell and the chip began reporting man-made impulses
    // as lightning.
    let spike = crate::settings::spike_rejection()
        .unwrap_or(defence::SPIKE_REJECTION_DEFAULT);
    match sensor.set_spike_rejection(i2c, spike) {
        Ok(()) => println!("as:   spike rejection {spike}"),
        Err(e) => println!("as:   could not set spike rejection -- {e}"),
    }
    // The watchdog is written by `session::apply` above, on every apply -- but
    // it is reported here, because a setting the tuner can no longer reach is
    // one a person has to be able to see without asking for it. It left the
    // search space for a sharper reason than spike rejection did: it gates on
    // amplitude, so the tuner spending it cost the distant strikes this device
    // exists to report before the thunder arrives.
    let watchdog = crate::settings::watchdog().unwrap_or(defence::WATCHDOG_DEFAULT);
    println!("as:   watchdog threshold {watchdog}");
    // `reset` issues PRESET_DEFAULT, which restores registers -- the datasheet
    // treats discarding the statistics as a separate operation, so until this
    // line no power cycle in the device's life had ever cleared them.
    session::restart_statistics(sensor, i2c, "boot");

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
pub fn antenna_self_test(
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





