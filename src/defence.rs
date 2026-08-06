//! How hard the sensor is trying to reject noise (§4.2).
//!
//! A **state machine over every rejection register the AS3935 exposes**, walked
//! as an odometer — and **ordered by how much damage each register does**, which
//! is the whole design.
//!
//! The loop around it is deliberately plain — configure the registers, count
//! events for a minute, show the count, decide up or down, program the next
//! combination, repeat. See `listen` in `main`.
//!
//! ## Why an odometer, and what the two earlier shapes got wrong
//!
//! The registers are not grades of the same thing:
//!
//! * **`NF_LEV`** is a *noise-floor* gate. It decides when the chip complains
//!   the band is noisy, and **cannot reject a lightning waveform** — the safest
//!   knob on the part.
//! * **`WDTH`** is the watchdog *amplitude* gate. Raising it discards weaker
//!   arrivals, distant strikes first.
//! * **`SREJ`** compares the signal against the chip's lightning *waveform
//!   template*. Raising it discards anything imperfectly shaped.
//! * **`MIN_NUM_LIGH`** makes the chip wait for a *pattern* — 1, 5, 9 or 16
//!   strikes — before reporting anything at all. The most destructive of the
//!   four by a wide margin: at 16 the first fifteen strikes of a storm are
//!   silent.
//!
//! ## The order is the design
//!
//! **In an odometer the least significant digit moves on every single step**, so
//! the ordering decides which register the machine reaches for first. Cheapest
//! last, most damaging first:
//!
//! | Significance | Register | Moves |
//! |---|---|---|
//! | most | `MIN_NUM_LIGH` | rarest — only when everything else is exhausted |
//! | | `SREJ` | |
//! | | `WDTH` | |
//! | least | `NF_LEV` | every step |
//!
//! So the floor sweeps 0→7 before the watchdog is touched at all, and
//! `MIN_NUM_LIGH` is not reached until the other three have been swept in full.
//! Putting a high-impact register in the last row would have it changing
//! constantly and suppressing everything above it.
//!
//! **Sequential, 31 rungs.** `NF_LEV` 0→7, then `WDTH` 2→15, then `SREJ` 1→11.
//! Past rung 7 it moved only registers that reject lightning, and never gave
//! them back, so a storm's own disturbers ratcheted it to maximum and pinned it
//! there. That is a plausible mechanism for the "disturbers but never lightning"
//! this device showed for two days.
//!
//! **Capped at 7.** `NF_LEV` alone, which is what the MicroPython reference
//! tunes — and what finally produced disturbers here. Safe, but `NF_LEV` does
//! run out beside a strong interferer, and then there is nothing left to try.
//!
//! The odometer takes both: every combination is reachable, but the cheap
//! register is exhausted before an expensive one is touched, and everything
//! cheaper **resets to 0 whenever an expensive one moves** — so the machine
//! re-tries the whole cheap range at each new expensive setting rather than
//! ratcheting.
//!
//! ## The rules
//!
//! * **Up** — increment the least significant register that has room (the noise
//!   floor); on overflow, carry into the next one up and reset everything below
//!   it to 0.
//! * **Down** — decrement the least significant register that is not already 0.
//!
//! Down is deliberately *not* the exact inverse of up. A true borrow would
//! relax one register by winding every cheaper one to its maximum, which is the
//! opposite of relaxing.
//!
//! ## One object per register
//!
//! Each [`Param`] owns its own `min`, `cur`, `max` and the callback that
//! programs it, and knows how to step itself — so [`Ladder`] is only the carry
//! logic between them, and adding a fifth register is one more object rather
//! than an edit in four places.
//!
//! The writer is a **type parameter** rather than a concrete signature. This
//! module is deliberately free of ESP-IDF imports so `tests/host/defence.rs` can
//! compile it directly, and a callback naming `&As3935` would end that; `session`
//! supplies the real one.

/// How many registers the machine walks.
pub const PARAMS: usize = 4;

/// One tunable register: its range, where it is now, how far it jumps, and how
/// to program it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Param<W> {
    /// What this register is called, for the console and the screen.
    pub name: &'static str,
    pub min: u8,
    pub cur: u8,
    pub max: u8,
    /// The stride this register moves by when travelling — halved on every
    /// reversal, restored on every continuation. See [`Param::slow`].
    pub step: u8,
    /// The stride at full speed, **2^(distance from the least significant row)**.
    ///
    /// The noise floor steps by 1, the watchdog by 2, spike rejection by 4, min
    /// strikes by 8. A more significant register is one the machine reaches for
    /// only when the cheaper ones are exhausted — so when it finally does move,
    /// creeping is wasted time and it should take a decisive bite.
    ///
    /// It also shrinks the reachable space by roughly an order of magnitude,
    /// which is what makes a position readable as a percentage at all.
    pub base_step: u8,
    /// Programs the chip with `cur`. See `session::new_ladder`.
    pub write: W,
}

impl<W> Param<W> {
    /// One step harder. `false` when already at `max`.
    ///
    /// Clamped rather than refused at the top of a stride, so a register whose
    /// range is not a whole number of steps still reaches its maximum exactly —
    /// the watchdog steps 0, 2, 4 … 14 and then lands on 15.
    pub fn up(&mut self) -> bool {
        if self.cur >= self.max {
            return false;
        }
        self.cur = self.cur.saturating_add(self.step).min(self.max);
        true
    }

    /// One step easier. `false` when already at `min`.
    pub fn down(&mut self) -> bool {
        if self.cur <= self.min {
            return false;
        }
        self.cur = self.cur.saturating_sub(self.step).max(self.min);
        true
    }

    pub fn reset(&mut self) {
        self.cur = self.min;
        self.step = self.base_step;
    }

    /// Halve the stride, to a floor of 1.
    ///
    /// **Called on a direction reversal**, which is the signature of having
    /// overshot: the machine went up, the band went quiet, it came back down,
    /// and the band got noisy again. That is the sweet spot being straddled, and
    /// continuing to arrive at it in strides of 8 would straddle it forever.
    /// Each reversal halves the stride until it is 1 and the register can settle
    /// exactly.
    pub fn slow(&mut self) {
        self.step = (self.step / 2).max(1);
    }

    /// Double the stride back toward [`Param::base_step`].
    ///
    /// Called whenever a move continues in the same direction as the last. A
    /// sustained climb means the setting is nowhere near right, and creeping
    /// there in ones after an earlier reversal would be slow for no reason.
    pub fn restore(&mut self) {
        self.step = self.step.saturating_mul(2).min(self.base_step.max(1));
    }

    /// How many positions this register can occupy, counting both ends.
    ///
    /// Ceiling division, because the last stride is usually short: 0..=15 by 2
    /// is nine positions, not eight — the eight full strides plus the clamped
    /// landing on 15.
    /// Measured against [`Param::base_step`], not the live stride: the gauge
    /// must not change scale underneath the reader every time the machine slows
    /// down.
    pub fn span(&self) -> u32 {
        let range = (self.max - self.min) as u32;
        let step = self.base_step.max(1) as u32;
        range.div_ceil(step) + 1
    }

    /// Which of those positions `cur` currently is.
    pub fn index(&self) -> u32 {
        let offset = (self.cur - self.min) as u32;
        let step = self.base_step.max(1) as u32;
        offset.div_ceil(step).min(self.span() - 1)
    }
}

/// The machine: the four registers, most significant first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ladder<W> {
    pub params: [Param<W>; PARAMS],
    /// Which way the last move went, for the stride adaptation in
    /// [`Param::slow`]. `None` before the first move.
    pub last_up: Option<bool>,
}

impl<W: Copy> Ladder<W> {
    /// Defend harder. Returns whether anything moved.
    ///
    /// `false` only at the very top, where every register is at its maximum and
    /// there is nothing left to try.
    pub fn up(&mut self) -> bool {
        let reversed = self.last_up == Some(false);
        for index in (0..PARAMS).rev() {
            if self.params[index].cur < self.params[index].max {
                // Slow down if this is a turn, speed up if it is a continuation
                // -- decided before the move, so the new stride is the one used.
                if reversed {
                    self.params[index].slow();
                } else {
                    self.params[index].restore();
                }
                self.params[index].up();
                self.last_up = Some(true);
                // The carry, and the reason this shape exists: everything less
                // significant starts its sweep again, so the registers that can
                // reject lightning are handed back each time a more significant
                // one takes the strain.
                for lower in index + 1..PARAMS {
                    self.params[lower].reset();
                }
                return true;
            }
        }
        false
    }

    /// Relax. Returns whether anything moved.
    ///
    /// Walks back the least significant register that is not already at its
    /// minimum — the noise floor first, since that is what the machine reached
    /// for first. Deliberately *not* the inverse of [`Ladder::up`]: a true borrow
    /// would relax one register by winding every cheaper one to its maximum,
    /// which is the opposite of relaxing.
    pub fn down(&mut self) -> bool {
        let reversed = self.last_up == Some(true);
        for index in (0..PARAMS).rev() {
            if self.params[index].cur > self.params[index].min {
                if reversed {
                    self.params[index].slow();
                } else {
                    self.params[index].restore();
                }
                self.params[index].down();
                self.last_up = Some(false);
                return true;
            }
        }
        false
    }

    /// Back to the most sensitive position.
    pub fn reset(&mut self) {
        for param in self.params.iter_mut() {
            param.reset();
        }
        self.last_up = None;
    }

    /// The current value of every register, most significant first.
    pub fn point(&self) -> [u8; PARAMS] {
        let mut out = [0u8; PARAMS];
        for (slot, param) in out.iter_mut().zip(self.params.iter()) {
            *slot = param.cur;
        }
        out
    }

    /// Resume from a previously learned point.
    ///
    /// **Rounded down onto the nominal stride grid, never up.** Two reasons, and
    /// both are about being wrong safely:
    ///
    /// * The room may have gone quiet since. Restoring a *more* defensive
    ///   position than the environment needs starts the device deaf, which is
    ///   the failure this whole subsystem exists to avoid; restoring a more
    ///   sensitive one costs a few windows of climbing.
    /// * Landing between grid points would leave `index` and the gauge
    ///   disagreeing with where the strides can actually go.
    ///
    /// Values outside a register's range are clamped rather than rejected, so a
    /// stored point from an older firmware with different ceilings degrades to
    /// something usable instead of refusing to load.
    pub fn restore_point(&mut self, point: [u8; PARAMS]) {
        for (param, value) in self.params.iter_mut().zip(point.iter()) {
            let clamped = (*value).clamp(param.min, param.max);
            let stride = param.base_step.max(1);
            let offset = clamped - param.min;
            param.cur = param.min + (offset / stride) * stride;
            param.step = param.base_step;
        }
        self.last_up = None;
    }

    /// Total positions the machine can occupy — the product of every span.
    ///
    /// Computed rather than written down, so changing a ceiling or adding a
    /// register cannot leave a stale constant behind.
    pub fn total(&self) -> u32 {
        self.params.iter().map(|p| p.span()).product()
    }

    /// Where the machine sits in its whole space, for the gauge.
    ///
    /// The odometer read as one number — most significant digit first, each
    /// weighted by the spans below it. Monotonic with [`Ladder::up`], which is
    /// what makes it meaningful as a bar.
    pub fn position(&self) -> u32 {
        let mut position = 0u32;
        for param in self.params.iter() {
            position = position * param.span() + param.index();
        }
        position
    }

    /// Which register is currently doing the work, for the console.
    pub fn rung(&self) -> &'static str {
        // The least significant register that is off its minimum is the one
        // being leaned on; if none is, the cheapest is where the machine starts.
        for param in self.params.iter().rev() {
            if param.cur > param.min {
                return param.name;
            }
        }
        self.params[PARAMS - 1].name
    }
}
