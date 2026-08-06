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
//! ## One parameter at a time, in order of what it costs
//!
//! The registers are not grades of the same thing:
//!
//! * **`NF_LEV`** is a *noise-floor* gate. It decides when the chip complains
//!   the band is noisy, and **cannot reject a lightning waveform** — the only
//!   free knob on the part, which is why it is tuned first.
//! * **`WDTH`** is the watchdog *amplitude* gate. Raising it discards weaker
//!   arrivals, distant strikes first.
//! * **`SREJ`** compares the signal against the chip's lightning *waveform
//!   template*. Raising it discards anything imperfectly shaped.
//! * **`MIN_NUM_LIGH`** makes the chip wait for a *pattern* — 1, 5, 9 or 16
//!   strikes — before reporting anything. The most destructive of the four: at
//!   16, the first fifteen strikes of a storm are silent.
//!
//! So a **cursor** walks that list. Only the register under the cursor moves:
//!
//! * **Noisy** — step it up. If it is already at its maximum, **leave it there**
//!   and advance the cursor to the next register, which starts from 0.
//! * **Silent** — step it down. If it is already at 0, retreat the cursor and
//!   keep reducing the cheaper register behind it.
//!
//! **Fixing rather than resetting is the whole point.** An earlier version swept
//! every combination as an odometer, which threw the noise floor back to 0 each
//! time it reached the watchdog — discarding the calibration it had just spent
//! minutes finding. Here a register that has done its job keeps its value, and
//! the machine only ever reaches for a more expensive one when the cheaper ones
//! are genuinely exhausted.
//!
//! Two earlier shapes failed from opposite directions. A **sequential 31-rung
//! ladder** ran `NF_LEV` 0→7 then `WDTH` then `SREJ`, never giving the
//! lightning-rejecting registers back, so a storm's own disturbers ratcheted it
//! deaf. A **cap at `NF_LEV` alone** was safe and is what first produced
//! disturbers here — but `NF_LEV` does run out, and then there was nothing left.
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
    /// Cheapest first: `NF_LEV`, `WDTH`, `SREJ`, `MIN_NUM_LIGH`.
    pub params: [Param<W>; PARAMS],
    /// Which register is currently being tuned. Everything before it is fixed
    /// at the value that stopped working; everything after it is at 0.
    pub cursor: usize,
    /// Which way the last move went, for the stride adaptation in
    /// [`Param::slow`]. `None` before the first move.
    pub last_up: Option<bool>,
}

impl<W: Copy> Ladder<W> {
    /// Defend harder. Returns whether anything moved.
    ///
    /// `false` only when the last register is also at its maximum and there is
    /// nothing left to try.
    pub fn up(&mut self) -> bool {
        let reversed = self.last_up == Some(false);
        loop {
            let cursor = self.cursor;
            if reversed {
                self.params[cursor].slow();
            } else {
                self.params[cursor].restore();
            }
            if self.params[cursor].up() {
                self.zero_after_cursor();
                self.last_up = Some(true);
                return true;
            }
            // Exhausted. **Leave it where it is** -- that value is the
            // calibration this register earned -- and move on to the next.
            if cursor + 1 >= PARAMS {
                return false;
            }
            self.cursor += 1;
        }
    }

    /// Silence — the register under the cursor has done its job.
    ///
    /// **One step back, then hand over to the next register.** Stepping back
    /// returns to the most sensitive setting that was still working; advancing
    /// the cursor lets the next register refine from there.
    ///
    /// Waiting for each register to reach its *maximum* before moving on was
    /// accurate and far too slow — the noise floor alone took seven windows.
    /// Silence is the boundary, and the boundary is the answer.
    ///
    /// **A register already at its minimum does nothing.** It emphatically does
    /// not retreat the cursor. An earlier version did, and it broke the whole
    /// model: after handing over to the watchdog at 0, the next silent window
    /// found nothing to give back, walked the cursor home to the noise floor and
    /// returned `false` — logging nothing. The following noisy window then
    /// raised the noise floor again, and the machine oscillated `nf 0/1` forever
    /// with a wasted window every cycle. Observed on hardware as a perfectly
    /// regular 30-second loop.
    ///
    /// The cost of not retreating is that a room which goes quiet after a noisy
    /// spell does not walk its expensive registers all the way back. It does
    /// relax: every silent window steps the current register down before handing
    /// over, so the machine sheds one notch per register as the cursor advances.
    /// Full recovery needs a restart, which is a separate decision.
    pub fn down(&mut self) -> bool {
        let cursor = self.cursor;
        if self.params[cursor].cur <= self.params[cursor].min {
            return false;
        }
        if self.last_up == Some(true) {
            self.params[cursor].slow();
        } else {
            self.params[cursor].restore();
        }
        self.params[cursor].down();
        self.last_up = Some(false);
        // Hand over: the next register refines from here.
        if cursor + 1 < PARAMS {
            self.cursor += 1;
            self.zero_after_cursor();
        }
        true
    }

    /// Hold every register past the cursor at 0.
    ///
    /// The invariant the whole model rests on: **everything before the cursor is
    /// fixed at the value that stopped working, the cursor is being tuned, and
    /// everything after it is untouched.** It falls out of the walk naturally —
    /// the cursor only advances when the register behind it is exhausted — but
    /// asserting it here means a restored point or a future edit cannot quietly
    /// leave an expensive register engaged while a cheap one is still being
    /// tuned.
    fn zero_after_cursor(&mut self) {
        for param in self.params.iter_mut().skip(self.cursor + 1) {
            param.reset();
        }
    }

    /// Back to the most sensitive position.
    pub fn reset(&mut self) {
        for param in self.params.iter_mut() {
            param.reset();
        }
        self.cursor = 0;
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
        // The cursor goes to the dearest register still engaged **after the
        // rounding**, not before: a register whose stored value rounds down to
        // its minimum was never really engaged, and letting it hold the cursor
        // would leave the machine tuning something already at 0 while a cheaper
        // register carried the whole calibration.
        self.cursor = 0;
        for index in 0..PARAMS {
            if self.params[index].cur > self.params[index].min {
                self.cursor = index;
            }
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
        for param in self.params.iter().rev() {
            position = position * param.span() + param.index();
        }
        position
    }

    /// Which register the cursor is on, for the console.
    pub fn rung(&self) -> &'static str {
        self.params[self.cursor].name
    }
}
