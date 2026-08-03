//! What a wake loop iteration accumulates, and what the screen is redrawn for.
//!
//! Split out of `main` because these grew there rather than belonging there:
//! three small state types and the four functions that move them. `listen` now
//! reads as a loop skeleton calling named steps instead of defining them
//! inline.

use esp_idf_hal::delay::FreeRtos;
use esp_idf_hal::i2c::I2cDriver;

use crate::as3935::{As3935, Distance, Interrupt, Location, Strike};

use crate::{as3935, clock, defence, history, log, settings};

/// What one batch window heard.
#[derive(Default)]
pub struct Batch {
    pub strikes: u32,
    pub disturbers: u32,
    pub noise: u32,
    pub unknown: u32,
}

impl Batch {
    pub fn heard_interference(&self) -> bool {
        self.disturbers > 0 || self.noise > 0
    }

    pub fn is_empty(&self) -> bool {
        self.strikes == 0 && !self.heard_interference() && self.unknown == 0
    }
}


/// Everything counted since boot.
#[derive(Default)]
pub struct Totals {
    pub strikes: u32,
    pub disturbers: u32,
    pub last_strike: Option<(Distance, u32, Option<u64>)>,
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
pub struct Drawn {
    pub strikes: u32,
    pub last_strike: Option<(Distance, u32, Option<u64>)>,
    pub location: Location,
    /// §4.2's level. **A change test, so 11 -> 11 cannot repaint** — only a
    /// genuine move does, and the 30 s floor bounds how often that can happen.
    ///
    /// It was excluded for a while after it caused a refresh every half minute
    /// in a marginal room, oscillating 0 -> 1 -> 0. That was the ladder sitting
    /// exactly on a threshold, which is the one place it does not settle.
    /// Everywhere else it **converges**: it climbs until the noise is rejected
    /// and then stops, so the churn is a property of one environment rather
    /// than of the value.
    pub defence: u8,
}


/// Read one interrupt and fold it into the batch.
pub fn collect(
    sensor: &As3935,
    i2c: &mut I2cDriver<'_>,
    batch: &mut Batch,
    totals: &mut Totals,
    history: &mut history::History,
    minute: u32,
    mut strike_log: Option<&mut log::Log>,
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
            // Strikes are reported individually and immediately. They are the
            // point of the device, they are rare, and a summary line would hide
            // the distance and energy that §4.3 needs.
            match sensor.strike(i2c) {
                Ok(strike) => record_strike(
                    totals,
                    history,
                    strike_log.as_deref_mut(),
                    &strike,
                    clock::now(),
                    minute,
                    false,
                ),
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


/// Record one strike: counters, rings, log, console — in that order.
///
/// **Shared by the real and simulated paths, which must stay behaviourally
/// identical.** They were written twice and did the same four things in the
/// same order; two copies of a sequence that has to match is two places for it
/// to stop matching, and the simulated path exists precisely so that the real
/// one can be trusted without a storm.
///
/// `simulated` changes only the console line. The record written to flash is
/// deliberately the same either way — a synthetic strike that logged
/// differently would not be exercising the path it exists to exercise.
pub fn record_strike(
    totals: &mut Totals,
    history: &mut history::History,
    strike_log: Option<&mut log::Log>,
    strike: &Strike,
    epoch: Option<u64>,
    minute: u32,
    simulated: bool,
) {
    totals.strikes += 1;
    totals.last_strike = Some((strike.distance, strike.intensity_milli(), epoch));
    history.record(minute, strike);

    // An unset clock still logs the strike, with 0 for the time. It happened;
    // what is unknown is when.
    if let Some(log) = strike_log {
        log.append(epoch.unwrap_or(0), strike);
    }

    let when = match epoch {
        Some(epoch) => clock::format_local(epoch),
        None => heapless::String::try_from("(no clock)").unwrap_or_default(),
    };
    println!(
        "STRIKE  {}  {:?}  energy {} (intensity {}.{:03}){}",
        when,
        strike.distance,
        strike.energy_raw,
        strike.intensity_milli() / 1000,
        strike.intensity_milli() % 1000,
        if simulated { "  [SIMULATED]" } else { "" }
    );
}

/// One line per batch that heard interference. Silent otherwise — a quiet
/// device should be quiet, and the earlier per-event printing made the console
/// unreadable in a noisy room.
pub fn report(batch: &Batch) {
    if !batch.heard_interference() && batch.unknown == 0 {
        return;
    }
    println!(
        "batch: {} disturber(s), {} noise, {} unknown",
        batch.disturbers, batch.noise, batch.unknown
    );
}


/// Move the defence level and say what it did in terms of the knob it is on.
pub fn tune(sensor: &As3935, i2c: &mut I2cDriver<'_>, level: u8, direction: &str) {
    let settings = defence::settings(level);
    match defence::apply(sensor, i2c, level) {
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
pub fn toggle_location(sensor: &As3935, i2c: &mut I2cDriver<'_>, location: &mut Location) {
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


