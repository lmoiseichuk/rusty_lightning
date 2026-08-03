//! How hard the sensor is trying to reject noise (§4.2).
//!
//! ## Why this is a ladder and not one number
//!
//! §4.2 auto-tunes `NF_LEV` alone, 0–7. On the bench that ran out in seconds:
//!
//! ```text
//! tune: disturber -- noise floor up to 7
//! tune: disturber -- noise floor already at 7, cannot defend further
//! ```
//!
//! ...repeating indefinitely. A detector that has stopped defending and says so
//! once a second is not tuned, it is stuck — and `NF_LEV` is not the only knob
//! the AS3935 offers. The datasheet's other two do the same job at different
//! stages of the receive chain:
//!
//! | Rung | Register | What it rejects |
//! |---|---|---|
//! | `NF_LEV` 0–7 | `0x01` | continuous noise, by raising the detection floor |
//! | `WDTH` 2–15 | `0x01` | short events that do not look like a strike's envelope |
//! | `SREJ` 0–11 | `0x02` | spikes that pass the watchdog but fail the shape test |
//!
//! So "defence" is one integer walking all three in order. Climbing costs
//! sensitivity in roughly that order too, which is why they are tried in it: the
//! noise floor is the cheapest thing to give up and spike rejection is the
//! dearest.
//!
//! ## The asymmetry is the design
//!
//! Up by one **per batch** in which anything was heard; down by one after a
//! whole minute of silence. Quick to defend, slow to relax — a storm's first
//! strike should not arrive into a receiver that spent the afternoon relaxing
//! toward a noise floor it will immediately have to climb back up.
//!
//! **Per batch, not per event.** An earlier version escalated per interrupt and
//! saturated the whole ladder in under two seconds, which is not tuning — it is
//! a counter racing the interrupt rate.

use esp_idf_hal::i2c::I2cDriver;

use crate::as3935::As3935;

/// Where each rung starts, from §3 step 6.
pub const NOISE_FLOOR_BASE: u8 = 0;
pub const WATCHDOG_BASE: u8 = 2;
pub const SPIKE_REJECT_BASE: u8 = 0;

/// Where each rung stops.
///
/// `NF_LEV` and `WDTH` are 3- and 4-bit fields and use their full range.
/// `SREJ` is capped below its 4-bit maximum on purpose: the datasheet's own
/// curves flatten past ~11, and the last few settings reject so aggressively
/// that a genuine nearby strike can be discarded. A detector that hears nothing
/// is worse than one that hears noise, because the noise is at least visible.
pub const NOISE_FLOOR_MAX: u8 = 7;
pub const WATCHDOG_MAX: u8 = 15;
pub const SPIKE_REJECT_MAX: u8 = 11;

/// Total rungs: every step of all three knobs.
pub const MAX_LEVEL: u8 = (NOISE_FLOOR_MAX - NOISE_FLOOR_BASE)
    + (WATCHDOG_MAX - WATCHDOG_BASE)
    + (SPIKE_REJECT_MAX - SPIKE_REJECT_BASE);

/// The three register values a defence level maps to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Settings {
    pub noise_floor: u8,
    pub watchdog: u8,
    pub spike_reject: u8,
}

/// Translate a level into register values.
///
/// Pure, total, and the only place the ladder's shape is written down — so
/// changing the order of the rungs is a one-function edit rather than a hunt
/// through the event loop.
pub fn settings(level: u8) -> Settings {
    let level = level.min(MAX_LEVEL);

    let noise_span = NOISE_FLOOR_MAX - NOISE_FLOOR_BASE;
    let watchdog_span = WATCHDOG_MAX - WATCHDOG_BASE;

    if level <= noise_span {
        return Settings {
            noise_floor: NOISE_FLOOR_BASE + level,
            watchdog: WATCHDOG_BASE,
            spike_reject: SPIKE_REJECT_BASE,
        };
    }

    let past_noise = level - noise_span;
    if past_noise <= watchdog_span {
        return Settings {
            noise_floor: NOISE_FLOOR_MAX,
            watchdog: WATCHDOG_BASE + past_noise,
            spike_reject: SPIKE_REJECT_BASE,
        };
    }

    Settings {
        noise_floor: NOISE_FLOOR_MAX,
        watchdog: WATCHDOG_MAX,
        spike_reject: SPIKE_REJECT_BASE + (past_noise - watchdog_span),
    }
}

/// A short name for which knob a level is currently working on, for the console
/// and later for the screen.
pub fn rung(level: u8) -> &'static str {
    let noise_span = NOISE_FLOOR_MAX - NOISE_FLOOR_BASE;
    let watchdog_span = WATCHDOG_MAX - WATCHDOG_BASE;
    match level {
        l if l <= noise_span => "noise floor",
        l if l - noise_span <= watchdog_span => "watchdog",
        _ => "spike rejection",
    }
}

/// Push one defence level into the sensor's three registers.
///
/// Lives here rather than in `main` because it is the other half of
/// [`settings`] — that turns a level into three numbers, this puts them on the
/// chip, and splitting them across modules meant a rung change had to be
/// followed through two files.
pub fn apply(
    sensor: &As3935,
    i2c: &mut I2cDriver<'_>,
    level: u8,
) -> Result<(), esp_idf_hal::sys::EspError> {
    let settings = settings(level);
    sensor.set_noise_floor(i2c, settings.noise_floor)?;
    sensor.set_watchdog_threshold(i2c, settings.watchdog)?;
    sensor.set_spike_rejection(i2c, settings.spike_reject)
}

