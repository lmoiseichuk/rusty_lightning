//! How hard the sensor is trying to reject noise (§4.2).
//!
//! One integer, 0..=[`MAX_LEVEL`], walking **`NF_LEV` and nothing else**. Up by
//! one per *batch* in which anything was heard, down by one after a whole
//! minute of silence: quick to defend, slow to relax.
//!
//! **See §4.2 for why** the step is per batch rather than per event.
//!
//! ## Why the ladder stops at the noise floor
//!
//! It did not always. It ran to 31 rungs: `NF_LEV` 0→7, then `WDTH` 3→15, then
//! `SREJ` 1→11 — and in that form the device reported disturbers during a storm
//! and never a strike, on hardware whose MicroPython reference had detected
//! strikes on the same bench.
//!
//! The reference tunes **only `NF_LEV`, 0 through 7**. That turns out to be the
//! whole difference, because the three registers are not three grades of the
//! same thing:
//!
//! * **`NF_LEV`** is a *noise-floor* gate. It decides when the chip complains
//!   that the band is noisy. It cannot reject a lightning waveform, which is
//!   why sweeping it over its full range is safe.
//! * **`WDTH`** is the watchdog *amplitude* gate on the incoming signal.
//!   Raising it discards weaker arrivals — distant strikes first.
//! * **`SREJ`** compares the signal against the chip's lightning *waveform*
//!   template. Raising it discards anything imperfectly shaped, and the
//!   datasheet's own curves flatten past ~11 while the last few settings reject
//!   so aggressively that a genuine nearby strike goes with them.
//!
//! So the two rungs above the noise floor tuned exactly the knobs that can
//! throw lightning away, and the climb rule guaranteed they would be reached:
//! +1 for any batch with activity, −1 only after a *full minute* of total
//! silence. A storm never grants that minute — and neither does a sensor
//! mounted against an e-paper panel, which produced batches of noise events on
//! a clear day. The ladder ratcheted to maximum rejection and stayed there.
//!
//! `WDTH` and `SREJ` therefore hold at their power-on defaults now, which is
//! what the working reference leaves them at. Deliberate over-sensitivity is
//! still reachable, but only by hand: `sensitive on`, via
//! `session::force_max_sensitivity`.
//!
//! This module is deliberately free of ESP-IDF imports, which is what lets
//! `tests/host/defence.rs` compile it directly rather than copying it. The
//! register writes live in `session::apply_defence`.

/// Where the ladder starts, from §3 step 6.
pub const NOISE_FLOOR_BASE: u8 = 0;

/// What `WDTH` and `SREJ` hold at, at every rung.
///
/// These are the chip's power-on defaults and the values the MicroPython
/// reference never moves. They are constants rather than rungs — see the module
/// comment for what happened when they were rungs.
pub const WATCHDOG_BASE: u8 = 2;
pub const SPIKE_REJECT_BASE: u8 = 0;

/// Where the ladder stops. `NF_LEV` is a 3-bit field and uses its full range.
pub const NOISE_FLOOR_MAX: u8 = 7;

/// Total rungs — now exactly the noise floor's own range.
pub const MAX_LEVEL: u8 = NOISE_FLOOR_MAX - NOISE_FLOOR_BASE;

/// The three register values a defence level maps to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Settings {
    pub noise_floor: u8,
    pub watchdog: u8,
    pub spike_reject: u8,
}

/// Translate a level into register values.
///
/// Pure, total, and the only place the ladder's shape is written down. It is
/// now one arm rather than three: saturating at [`MAX_LEVEL`] is what keeps a
/// caller that still thinks in 31 rungs — a stale console command, a value read
/// back from an older build — from indexing past the noise floor.
pub fn settings(level: u8) -> Settings {
    Settings {
        noise_floor: NOISE_FLOOR_BASE + level.min(MAX_LEVEL),
        watchdog: WATCHDOG_BASE,
        spike_reject: SPIKE_REJECT_BASE,
    }
}

/// A short name for which knob a level is working on, for the console and the
/// screen.
///
/// One answer now, and kept as a function rather than folded into its callers
/// so the two display sites do not have to learn that the ladder became
/// single-knob — and so a future second rung has somewhere to be named.
pub fn rung(_level: u8) -> &'static str {
    "noise floor"
}

