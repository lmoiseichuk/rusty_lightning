//! What one tuning window saw, and the verdict it supports.
//!
//! **Free of ESP-IDF on purpose**, like `defence` and for the same reason: the
//! decision this holds is the one worth testing, and `tuning` cannot be compiled
//! on a workstation because it drives an I²C sensor. Everything here is
//! arithmetic over four counters, so `tests/verdict.rs` includes this file
//! by path and checks the real thing rather than a copy.
//!
//! ## The distinction this exists to hold
//!
//! `NF_LEV` is the only knob the noise decision has, and it gates the **noise
//! floor**. A disturber is a waveform the chip received, validated, and
//! rejected as not-lightning — raising the noise floor does nothing to one.
//!
//! So a window's disturbers are counted and reported, and are deliberately not
//! evidence about the noise floor. Folding them in made the tuner answer a
//! question its knob does not reach: watched live through the storm of
//! 2026-08-26, `NF_LEV` stepped 2 → 3 and opened the band (11 noise → 0), then
//! kept climbing on 5–8 disturbers a window that no setting could have removed.
//! It would burn the ladder's whole range and hand over to the stuck detector.
//!
//! ## And the second one, since 0.22.0
//!
//! A lightning interrupt is not, by itself, a strike the tuner may act on. The
//! chip's `overhead` reading has been man-made every time it could be checked
//! -- see `golden::is_evidence` -- and a window that held for it, or a dip that
//! stayed open for it, was being steered by the hammer next door. So a
//! [`Batch`] counts every lightning interrupt for the console, and separately
//! the ones that are evidence; a [`Window`] carries only the second kind into
//! the decisions, and the first only as a number to report.

use crate::golden;
use crate::strike::Distance;

/// What one batch window heard -- about a second of interrupts.
///
/// Moved here from `session` in 0.22.0 so the split between a lightning
/// interrupt and a strike that counts is host-tested where it is made, rather
/// than in a module that cannot be built on a workstation.
#[derive(Default)]
pub struct Batch {
    /// Every lightning interrupt, counted as it lands -- before the merge window
    /// and before anything can drop it. What the console's `batch:` line says.
    pub strikes: u32,
    /// Of those, the ones that are **not** evidence: the chip said `overhead`,
    /// or its distance could not be read. Logged and shown like any other; never
    /// an input to a decision.
    pub unconfirmed: u32,
    pub disturbers: u32,
    pub noise: u32,
    pub unknown: u32,
}

impl Batch {
    /// Count one lightning interrupt.
    ///
    /// `distance` is `None` when the detail read failed. That is counted as
    /// unconfirmed rather than as evidence: nothing says what the chip measured,
    /// and the cost of the conservative reading is one strike the tuner does not
    /// act on, where the other reading lets a fault vouch for a setting.
    pub fn lightning(&mut self, distance: Option<Distance>) {
        self.strikes += 1;
        let evidence = match distance {
            Some(distance) => golden::is_evidence(distance),
            None => false,
        };
        if !evidence {
            self.unconfirmed += 1;
        }
    }

    /// Lightning interrupts that are evidence: the ones a decision may read.
    pub fn confirmed(&self) -> u32 {
        self.strikes.saturating_sub(self.unconfirmed)
    }

    pub fn heard_interference(&self) -> bool {
        self.disturbers > 0 || self.noise > 0
    }

    /// **Uncalled since the ladder moved to a once-a-minute decision.** It
    /// answered "did this one-second batch hear nothing", which was the trigger
    /// for the old per-batch decay; the minute window asks the same question of
    /// a span long enough for the answer to mean something. Kept as the natural
    /// complement to `heard_interference`.
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.strikes == 0 && !self.heard_interference() && self.unknown == 0
    }
}

/// The evidence one window gathered.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct Window {
    /// Noise-floor events. **The only input to [`Window::quiet`].**
    pub noise: u32,
    /// Validated waveforms the chip rejected. Counted, reported, and not
    /// evidence about the noise floor.
    pub disturbers: u32,
    /// Strikes that are evidence (`golden::is_evidence`), which veto a climb —
    /// a near strike throws harmonics that arrive as disturbers, so a window
    /// that heard one cannot judge the band. **The only strike count a decision
    /// reads**: the hold, the dip that stays open, and the silence the rescue
    /// waits out.
    pub strikes: u32,
    /// Lightning interrupts that were not evidence -- `overhead`, or unreadable.
    /// Counted so the console can say the window had them; never a reason to
    /// hold, to stay open, or to reset the silence.
    pub unconfirmed: u32,
}

impl Window {
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    /// Fold one batch of readings into the window. `strikes` must already be
    /// the confirmed count; [`Window::fold_batch`] is how the loop calls it.
    pub fn fold(&mut self, noise: u32, disturbers: u32, strikes: u32) {
        self.noise += noise;
        self.disturbers += disturbers;
        self.strikes += strikes;
    }

    /// Fold one [`Batch`] in: the confirmed strikes as strikes, and the rest
    /// beside them where nothing decides on them.
    ///
    /// **The one place a batch becomes evidence**, so the split cannot be made
    /// differently on two paths -- passing `batch.strikes` here instead of
    /// `batch.confirmed()` is the whole bug this exists to prevent, and it
    /// compiles.
    pub fn fold_batch(&mut self, batch: &Batch) {
        self.fold(batch.noise, batch.disturbers, batch.confirmed());
        self.unconfirmed += batch.unconfirmed;
    }

    /// Whether this window heard lightning it may act on.
    pub fn heard_lightning(&self) -> bool {
        self.strikes > 0
    }

    /// A count as a per-minute rate.
    ///
    /// **Multiply before dividing**, so a window that is not a whole divisor of
    /// sixty still scales instead of collapsing to 1.
    pub fn per_min(count: u32, window_s: u32) -> u32 {
        count * 60 / window_s.max(1)
    }

    pub fn noise_per_min(&self, window_s: u32) -> u32 {
        Self::per_min(self.noise, window_s)
    }

    pub fn disturbers_per_min(&self, window_s: u32) -> u32 {
        Self::per_min(self.disturbers, window_s)
    }

    /// Whether the band is quiet enough to try a more sensitive point.
    ///
    /// A **rate**, not a count, so the verdict means the same thing whatever the
    /// window length.
    pub fn quiet(&self, quiet_per_min: u32, window_s: u32) -> bool {
        self.noise_per_min(window_s) <= quiet_per_min
    }
}
