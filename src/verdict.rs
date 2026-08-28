//! What one tuning window saw, and the verdict it supports.
//!
//! **Free of ESP-IDF on purpose**, like `defence` and for the same reason: the
//! decision this holds is the one worth testing, and `tuning` cannot be compiled
//! on a workstation because it drives an I²C sensor. Everything here is
//! arithmetic over four counters, so `tests/host/verdict.rs` includes this file
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

/// The evidence one window gathered.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct Window {
    /// Noise-floor events. **The only input to [`Window::quiet`].**
    pub noise: u32,
    /// Validated waveforms the chip rejected. Counted, reported, and not
    /// evidence about the noise floor.
    pub disturbers: u32,
    /// Strikes, which veto a climb — a near strike throws harmonics that arrive
    /// as disturbers, so a window that heard one cannot judge the band.
    pub strikes: u32,
}

impl Window {
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    /// Fold one batch of readings into the window.
    pub fn fold(&mut self, noise: u32, disturbers: u32, strikes: u32) {
        self.noise += noise;
        self.disturbers += disturbers;
        self.strikes += strikes;
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
