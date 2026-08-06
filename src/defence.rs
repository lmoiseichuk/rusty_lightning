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

/// One tunable register: its range, where it is now, and how to program it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Param<W> {
    /// What this register is called, for the console and the screen.
    pub name: &'static str,
    pub min: u8,
    pub cur: u8,
    pub max: u8,
    /// Programs the chip with `cur`. See `session::WRITERS`.
    pub write: W,
}

impl<W> Param<W> {
    /// One step harder. `false` when already at `max`.
    pub fn up(&mut self) -> bool {
        if self.cur < self.max {
            self.cur += 1;
            true
        } else {
            false
        }
    }

    /// One step easier. `false` when already at `min`.
    pub fn down(&mut self) -> bool {
        if self.cur > self.min {
            self.cur -= 1;
            true
        } else {
            false
        }
    }

    pub fn reset(&mut self) {
        self.cur = self.min;
    }

    /// How many distinct values this register can take.
    pub fn span(&self) -> u32 {
        (self.max - self.min) as u32 + 1
    }
}

/// The machine: the four registers, most significant first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ladder<W> {
    pub params: [Param<W>; PARAMS],
}

impl<W: Copy> Ladder<W> {
    /// Defend harder. Returns whether anything moved.
    ///
    /// `false` only at the very top, where every register is at its maximum and
    /// there is nothing left to try.
    pub fn up(&mut self) -> bool {
        for index in (0..PARAMS).rev() {
            if self.params[index].up() {
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
        for index in (0..PARAMS).rev() {
            if self.params[index].down() {
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
            position = position * param.span() + (param.cur - param.min) as u32;
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
