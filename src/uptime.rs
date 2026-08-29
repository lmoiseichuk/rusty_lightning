//! Comparing times on a counter that wraps.
//!
//! **Free of ESP-IDF so the wrap is host-tested**, like `defence`, `verdict`,
//! `csv` and `press`.
//!
//! `now_ms()` is `esp_timer_get_time() / 1000` narrowed to `u32`, so it wraps
//! every **49.7 days**. This device is meant to sit on a wall through a season;
//! it reaches that.
//!
//! ## Why `saturating_sub` is the wrong tool, and how it fails
//!
//! Every interval in this firmware is `now.saturating_sub(then) >= interval`.
//! That reads as defensive and is the opposite. After the wrap, `then` is a
//! large number near 2³² and `now` is small, so the subtraction goes negative,
//! **saturates to zero, and stays there** — not for one tick, but until `now`
//! climbs all the way back past `then`, which is another 49.7 days.
//!
//! Every one of those comparisons then reads "no time has passed", for ever:
//! the batch never closes, the tuner never steps, the screen never redraws, the
//! log never syncs. The interrupt still fires and the device still answers its
//! console, so it looks alive while doing nothing at all. That is the worst
//! shape a failure can have.
//!
//! ## Why not a wider counter
//!
//! `u64` would work and is the wrong fix: it touches every call site and every
//! stored sample to avoid one wrap in fifty days. The right fix for an interval
//! shorter than half the counter's range is modular arithmetic, which is what
//! the hardware counter already is.
//!
//! `wrapping_sub` gives the true elapsed time across a wrap for any interval
//! under 2³¹ ms — about 24 days. The longest interval here is the six-hourly
//! sweep, so there are three orders of magnitude of headroom.
//!
//! ## The one thing this cannot fix
//!
//! A sample older than 24 days is genuinely ambiguous: modular arithmetic
//! cannot tell "24 days ago" from "25 days in the future". Nothing here holds a
//! sample that long, and if something ever does it needs a different mechanism,
//! not a wider integer.

/// Elapsed time since `then`, correct across the counter's wrap.
///
/// Both arguments must come from the same counter and be in the same units.
pub fn since(now: u32, then: u32) -> u32 {
    now.wrapping_sub(then)
}

/// Whether `interval` has elapsed since `then`.
///
/// The shape almost every caller wants, so the wrap-correct subtraction is not
/// something each of them has to remember.
pub fn due(now: u32, then: u32, interval: u32) -> bool {
    since(now, then) >= interval
}
