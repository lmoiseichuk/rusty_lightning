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
//! A lightning interrupt is not, by itself, a strike the device may learn
//! from. The chip's `overhead` reading has been man-made on the days it could
//! be checked and lightning on others -- see `golden::is_evidence` -- and
//! nothing in the reading tells the two apart. So a [`Batch`] counts every
//! lightning interrupt, and separately the ones that are evidence, and a
//! [`Window`] carries both. Which a decision reads depends on what it costs to
//! be wrong:
//!
//! * **Refusing to step reads every lightning interrupt** -- [`Window::walk`].
//!   A hold changes nothing, so holding for the hammer costs a window the walk
//!   did not take. Not holding for a real storm overhead costs the climb into
//!   deafness the hold exists to prevent, at the moment the device exists for.
//! * **Everything that commits the device to something reads only evidence**:
//!   a dip that stays wide open ([`Window::heard_lightning`]), the dip a strike
//!   makes unnecessary, the silence the rescue waits out, and the golden
//!   record. Those are what September's stream of `overhead`, one every
//!   7-10 s for nine days, would otherwise have steered.

use crate::golden;
use crate::strike::Strike;

/// Strikes in a dip window before the weather is believed.
///
/// **One.** A dip is already the rare case, and a storm that is starting is
/// exactly when being slow is most expensive -- the opening strikes are the
/// warning the whole device exists to give. The cost of believing a single
/// event is a false alarm from something man-made, which lasts until the next
/// window judges the band and walks the point back.
///
/// Moved here from `tuning` in 0.22.1 with [`Window::dip_stays_open`], so the
/// rule that reads it is host-tested.
pub const DIP_STRIKES_TO_BELIEVE: u32 = 1;

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
    /// Count one lightning interrupt, from the detail read exactly as the
    /// driver returned it.
    ///
    /// **The read itself, not a distance taken out of it**, so the call in
    /// `session::read_and_handle` hands it over untouched and every decision
    /// about it is made here, where the host checks run. That one line is the
    /// boundary: nothing on a workstation can drive the I²C read, so the less
    /// it does, the less there is that no check can see. Generic over the
    /// error because only whether there was one matters here, and so the
    /// checks need no ESP-IDF error type.
    ///
    /// A failed read is counted as unconfirmed rather than as evidence: nothing
    /// says what the chip measured, and the cost of the conservative reading is
    /// one strike the tuner does not learn from, where the other reading lets a
    /// fault vouch for a setting.
    pub fn lightning<E>(&mut self, read: &Result<Strike, E>) {
        self.strikes += 1;
        let evidence = match read {
            Ok(strike) => golden::is_evidence(strike.distance),
            Err(_) => false,
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

/// The ±1 walk's move for one window -- see [`Window::walk`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Walk {
    /// Stay where it is.
    ///
    /// **For any lightning interrupt, `overhead` included.** The storm of
    /// 2026-08-12 is why: 917 detections, every one in the nearest bin, while
    /// thunder was audible every 10-30 s, and a band at 208-299 noise/min
    /// against a 120/min threshold. Every window with a strike in it held, and
    /// that is what kept the point open for three and a half hours
    /// (doc/specs.md §9). 0.22.0 held only for a strike with a distance, so the
    /// same night would have climbed noise/threshold -- three or four notches a
    /// window -- to `nf 7` within two windows, with no record written to bring
    /// it back.
    ///
    /// A hold cannot deafen anything, which is why this is the one decision
    /// that does not ask whether a strike is evidence. What holding for a
    /// man-made `overhead` costs is a walk that did not step; September's
    /// stream would have held it for nine days, while the dip -- which reads
    /// only evidence -- still opened every ten minutes to look.
    Hold,
    /// Tighten, by as many notches as the rate is multiples of the threshold.
    Climb,
    /// Relax one notch, if the learned floor allows it.
    Relax,
}

/// The evidence one window gathered.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct Window {
    /// Noise-floor events. **The only input to [`Window::quiet`].**
    pub noise: u32,
    /// Validated waveforms the chip rejected. Counted, reported, and not
    /// evidence about the noise floor.
    pub disturbers: u32,
    /// Strikes that are evidence (`golden::is_evidence`). What a dip reads, both
    /// to stay open and to be skipped; the hold reads these and
    /// [`Window::unconfirmed`] together.
    pub strikes: u32,
    /// Lightning interrupts that were not evidence -- `overhead`, or unreadable.
    ///
    /// **They hold the walk** ([`Window::walk`]): a near strike throws
    /// harmonics that arrive as disturbers and raises the noise floor for its
    /// duration, so a window that heard one cannot judge the band, whatever the
    /// chip made of its distance. They are never a reason for a dip to stay
    /// open, for one to be skipped, or for the rescue's silence to start again.
    pub unconfirmed: u32,
    /// Batches folded in, and how many of them heard any noise at all.
    ///
    /// **For comparing with 0.20, and for nothing else.** Asleep, 0.20's edge
    /// interrupt could read the reason register about once per batch -- the
    /// pin was isolated while the chip slept, so only the batch-close poll saw
    /// an event -- which makes "batches with noise" the quantity an unattended
    /// 0.20 board actually counted. [`Window::quiet`] does not read it: the
    /// thresholds were fitted on awake counts, which are the raw rate this
    /// window carries in `noise`. See `doc/scanning.md` §5 for the arithmetic.
    pub batches: u32,
    pub noisy_batches: u32,
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
        self.batches += 1;
        if batch.noise > 0 {
            self.noisy_batches += 1;
        }
    }

    /// Whether this window heard lightning that is evidence: what keeps a dip
    /// open, and what makes the next one unnecessary.
    pub fn heard_lightning(&self) -> bool {
        self.strikes > 0
    }

    /// Whether this window heard any lightning interrupt, evidence or not.
    pub fn heard_any_lightning(&self) -> bool {
        self.strikes > 0 || self.unconfirmed > 0
    }

    /// For a window spent dipping wide open: whether it found weather, so the
    /// dip stays where it is instead of restoring the defended point.
    ///
    /// **Strikes that are evidence only.** A dip that stayed open for an
    /// `overhead` would sit wide open on the word of the impulse a fully open
    /// receiver is most likely to admit.
    pub fn dip_stays_open(&self) -> bool {
        self.strikes >= DIP_STRIKES_TO_BELIEVE
    }

    /// Whether a dip may start after this window, if one is due.
    ///
    /// **Not after a strike that is evidence**: the device is plainly not deaf,
    /// and the hold is doing its job. **But after an `overhead`, yes** -- a
    /// stream of them every 7-10 s, as September produced for nine days, would
    /// otherwise have stopped every dip from starting, and the dip is the one
    /// rule that can find a storm the setting itself is deaf to.
    pub fn dip_may_start(&self) -> bool {
        !self.heard_lightning()
    }

    /// What the ±1 walk does with this window, when neither a sweep nor a dip
    /// owns it.
    ///
    /// **Here rather than in `tuning`, so the hold is host-tested**: `tuning`
    /// drives an I²C sensor and nothing on a workstation can build it. The
    /// order of the tests is the rule -- a window that heard any lightning
    /// holds, whatever its noise; otherwise its noise decides.
    pub fn walk(&self, quiet_per_min: u32, window_s: u32) -> Walk {
        if self.heard_any_lightning() {
            return Walk::Hold;
        }
        match self.quiet(quiet_per_min, window_s) {
            true => Walk::Relax,
            false => Walk::Climb,
        }
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
    ///
    /// **Every `NoiseTooHigh` the interrupt delivered, not the batches that had
    /// one.** That is the quantity the 60/min threshold and the drowned multiple
    /// were fitted on -- console sessions, awake, where 0.20's edge interrupt
    /// already counted at the chip's own re-raise rate -- and it is what the
    /// level interrupt counts awake and asleep alike.
    pub fn quiet(&self, quiet_per_min: u32, window_s: u32) -> bool {
        self.noise_per_min(window_s) <= quiet_per_min
    }
}

/// When lightning that is evidence was last heard -- the clock the rescue's
/// patience (`golden::PATIENCE_MINUTES`) runs on.
///
/// **Here rather than as a bare field in `tuning`**, so the rule that starts it
/// again is host-tested: only a confirmed strike does. Until 0.22.1 that was a
/// line in `Tuning::observe`, which no check compiles, and putting
/// `batch.strikes` back in it -- which lets September's `overhead`, one every
/// 7-10 s, keep the rescue from ever running -- passed every host check.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct Silence {
    /// `None` until the first confirmed strike of this run.
    last_ms: Option<u32>,
}

impl Silence {
    /// Fold one batch in, at `now_ms`. A confirmed strike starts the silence
    /// again; nothing else does.
    pub fn observe(&mut self, batch: &Batch, now_ms: u32) {
        if batch.confirmed() > 0 {
            self.last_ms = Some(now_ms);
        }
    }

    /// Whole minutes since lightning that is evidence was last heard, or `None`
    /// if it never has been during this run.
    ///
    /// **Since the last strike, not since boot.** A device that has been up for
    /// a week and heard a storm an hour ago is not deaf; one that has been up
    /// an hour and heard nothing might be, and only the record of what worked
    /// can tell the two apart.
    pub fn minutes(&self, now_ms: u32) -> Option<u32> {
        match self.last_ms {
            Some(at) => Some(crate::uptime::since(now_ms, at) / 60_000),
            None => None,
        }
    }
}
