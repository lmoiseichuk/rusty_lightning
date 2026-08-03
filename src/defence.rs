//! How hard the sensor is trying to reject noise (§4.2).
//!
//! One integer, 0..=[`MAX_LEVEL`], walking three registers in order — noise
//! floor, then watchdog, then spike rejection. Up by one per *batch* in which
//! anything was heard, down by one after a whole minute of silence: quick to
//! defend, slow to relax.
//!
//! **See §4.2 for why**: what `NF_LEV` alone did on the bench, what the other
//! two knobs reject, why `SREJ` stops at 11 of 15, and why the step is per
//! batch rather than per event.
//!
//! This module is deliberately free of ESP-IDF imports, which is what lets
//! `tests/host/defence.rs` compile it directly rather than copying it. The
//! register writes live in `session::apply_defence`.

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

