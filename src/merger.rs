//! Folding return strokes into flashes (§4.3).
//!
//! **Free of ESP-IDF so it can be host-tested**, like `defence`, `verdict`,
//! `csv`, `press` and `uptime`. It lived in `session`, which drives an I²C
//! sensor and cannot be built on a workstation — so the merge window, which is
//! pure arithmetic over a strike and a clock, had no coverage at all. That
//! matters more than it sounds: the merge window is what the argument "930 of
//! those records were return strokes" rests on, and nothing had ever checked
//! that the window behaves as claimed.
//!
//! Everything here works on `strike::Strike`, which was split out of the driver
//! for this same reason.

use crate::strike::{Distance, Strike};

/// The default merge window.
///
/// **It has never fired on real data, and structurally cannot on the logs this
/// device has written.** Of 1040 records in storm-2026-08-12.csv not one shares
/// a second with another, so with a one-second window there was never a second
/// stroke inside one. That is what makes the "930 of those were return strokes"
/// argument circular, and why this module now has tests.
pub const MERGE_WINDOW_MS: u32 = 1_000;

/// One flash: the strokes that arrived inside a merge window, folded together.
pub struct Merged {
    pub strike: Strike,
    /// How many strokes went into it. Never zero.
    pub strokes: u32,
    /// The **first** stroke's clock, not the last. The flash began then, and a
    /// merge must not move an event later than it happened.
    pub epoch: Option<u64>,
    pub minute: u32,
    pub simulated: bool,
}

/// Folds return strokes into flashes (§4.3).
///
/// **The window runs from the first stroke, not the last.** A sliding window
/// would let a long enough train merge without limit — a storm directly
/// overhead could collapse into a single record hours long, which is the
/// opposite of what this is for.
pub struct Merger {
    window_ms: u32,
    pending: Option<Accumulator>,
}

struct Accumulator {
    started_ms: u32,
    energy_sum: u32,
    /// Kilometre readings only. Overhead and out-of-range are not distances and
    /// averaging them in as 1 and 63 is the bug `history::Bucket` documents.
    distance_km_sum: u32,
    distance_samples: u32,
    overhead: bool,
    strokes: u32,
    epoch: Option<u64>,
    minute: u32,
    simulated: bool,
}

impl Default for Merger {
    fn default() -> Self {
        Self {
            window_ms: MERGE_WINDOW_MS,
            pending: None,
        }
    }
}

impl Accumulator {
    fn fold(&mut self, strike: &Strike) {
        self.strokes += 1;
        self.energy_sum = self.energy_sum.saturating_add(strike.energy_raw);
        match strike.distance {
            Distance::Km(km) => {
                self.distance_km_sum = self.distance_km_sum.saturating_add(km as u32);
                self.distance_samples += 1;
            }
            Distance::Overhead => self.overhead = true,
            Distance::OutOfRange => {}
        }
    }

    /// The flash these strokes describe.
    ///
    /// Distance is the mean over *measured* kilometres. With none, overhead
    /// wins if any stroke reported it — it is the closest reading there is, and
    /// folding it into the mean as a number is what once made a storm read as
    /// permanently overhead.
    fn finish(self) -> Merged {
        let distance = match (self.distance_samples, self.overhead) {
            (0, true) => Distance::Overhead,
            (0, false) => Distance::OutOfRange,
            (samples, _) => Distance::Km((self.distance_km_sum / samples as u32) as u8),
        };
        Merged {
            strike: Strike {
                distance,
                energy_raw: self.energy_sum,
            },
            strokes: self.strokes,
            epoch: self.epoch,
            minute: self.minute,
            simulated: self.simulated,
        }
    }
}

impl Merger {
    pub fn window_ms(&self) -> u32 {
        self.window_ms
    }

    /// Change the window, flushing whatever is pending under the old one.
    ///
    /// Returned rather than dropped: those strokes were detected, and a setting
    /// change is no reason to lose them.
    pub fn set_window_ms(&mut self, window_ms: u32) -> Option<Merged> {
        let flushed = self.pending.take().map(Accumulator::finish);
        self.window_ms = window_ms;
        flushed
    }

    /// Fold one stroke in.
    ///
    /// Returns the *previous* flash when this stroke fell outside its window,
    /// because that flash is now complete and this stroke starts the next one.
    pub fn observe(
        &mut self,
        strike: &Strike,
        epoch: Option<u64>,
        minute: u32,
        simulated: bool,
        now_ms: u32,
    ) -> Option<Merged> {
        // A window of zero switches merging off entirely: every stroke is its
        // own flash, which is what the device did before 0.7.0 and what anyone
        // studying individual strokes wants back.
        if self.window_ms == 0 {
            return Some(
                Accumulator {
                    started_ms: now_ms,
                    energy_sum: 0,
                    distance_km_sum: 0,
                    distance_samples: 0,
                    overhead: false,
                    strokes: 0,
                    epoch,
                    minute,
                    simulated,
                }
                .tap_fold(strike),
            );
        }

        let expired = match self.pending.as_ref() {
            None => false,
            Some(pending) => crate::uptime::due(now_ms, pending.started_ms, self.window_ms),
        };

        let flushed = if expired {
            self.pending.take().map(Accumulator::finish)
        } else {
            None
        };

        match self.pending.as_mut() {
            Some(pending) => pending.fold(strike),
            None => {
                let mut fresh = Accumulator {
                    started_ms: now_ms,
                    energy_sum: 0,
                    distance_km_sum: 0,
                    distance_samples: 0,
                    overhead: false,
                    strokes: 0,
                    epoch,
                    minute,
                    simulated,
                };
                fresh.fold(strike);
                self.pending = Some(fresh);
            }
        }

        flushed
    }

    /// Release a flash whose window has closed.
    ///
    /// Called from the loop, because the last stroke of a storm has nothing
    /// after it to push it out — without this it would sit in memory until the
    /// next strike, which might be next week.
    pub fn take_due(&mut self, now_ms: u32) -> Option<Merged> {
        let expired = match self.pending.as_ref() {
            None => false,
            Some(pending) => crate::uptime::due(now_ms, pending.started_ms, self.window_ms),
        };
        match expired {
            true => self.pending.take().map(Accumulator::finish),
            false => None,
        }
    }
}

impl Accumulator {
    /// `fold` then `finish`, for the merging-disabled path.
    fn tap_fold(mut self, strike: &Strike) -> Merged {
        self.fold(strike);
        self.finish()
    }
}
