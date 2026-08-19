//! How hard the sensor is trying to reject noise (§4.2), as **one 13-bit
//! number**.
//!
//! The four registers the AS3935 exposes for this are bit fields living in two
//! bytes, so the whole tunable state is 13 bits — and treating it as a single
//! integer 0..=8191 replaces the state machine that used to walk them.
//!
//! ## The layout, most valuable bits first
//!
//! ```text
//!   bit  10  9  8 | 7  6  5  4 | 3  2  1  0
//!         NF_LEV   |    WDTH    |    SREJ
//!        (3 bits)  |  (4 bits)  |  (4 bits)
//!
//! `MIN_NUM_LIGH` is deliberately absent -- it is pinned to "report every
//! strike" and is not representable. See [`MIN_STRIKES_COUNT`].
//! ```
//!
//! **The order is the whole design, and it works because binary search resolves
//! the high bits first.** A bisection over 0..=8191 probes 4096 first, which is
//! a decision about `NF_LEV` alone; the last probes decide the bottom two bits.
//! So the cheap knob is settled coarsely up front and the destructive one only
//! ever moves as a final fine adjustment:
//!
//! * **`NF_LEV`** is a *noise-floor* gate. It decides when the chip complains
//!   the band is noisy, and **cannot reject a lightning waveform** — the only
//!   free knob on the part, which is why it takes the top bits.
//! * **`WDTH`** is the watchdog *amplitude* gate. Raising it discards weaker
//!   arrivals, distant strikes first.
//! * **`SREJ`** compares the signal against the chip's lightning *waveform
//!   template*. Raising it discards anything imperfectly shaped.
//! * **`MIN_NUM_LIGH`** makes the chip wait for a *pattern* — 1, 5, 9 or 16
//!   strikes — before reporting anything. The most destructive of the four: at
//!   16, the first fifteen strikes of a storm are silent. Bottom two bits, so a
//!   search reaches it last and moves it least.
//!
//! **[`FIELDS`] is the layout**, one row of mask-and-shift per register. Trying
//! the opposite ordering — less valuable bits high — is reordering the `shift`
//! column and nothing else; `tests/host/defence.rs` checks that whatever order
//! is in the table still tiles the 13 bits with no gap and no overlap.
//!
//! Every one of the 8192 values is a distinct, legal combination — no field is
//! capped below its width, because a cap would fold several raw values onto one
//! point and strand the ±1 tuner in a short cycle. See the note by `MAX`.
//!
//! Deliberately *not* mirroring the two hardware register bytes, which would let
//! the writer send two raw bytes: `REG0x02` bit 6 is `CL_STAT`, so that register
//! needs read-modify-write regardless, and mirroring would put `MIN_NUM_LIGH`
//! above `SREJ` in significance — settling the most destructive register before
//! the waveform template.
//!
//! ## What this replaced, and why
//!
//! A `Param`/`Ladder` state machine with a cursor, per-register strides, a
//! mixed-radix `position()` and a recursive four-level search. Every one of
//! those existed to impose an ordering on the registers that the bit layout now
//! gives for free, and the hand-built version cost a full sweep of *hundreds* of
//! probes against thirteen here.
//!
//! It also carried the defect that made this rewrite worth doing. The ladder
//! stored **sensitivity** (`cur = max` meaning most sensitive, each writer
//! subtracting on the way to the bus) while the rest of the crate assumed
//! defence, so "noisy, so defend harder" made the receiver *more* sensitive —
//! a runaway to full sensitivity, and the mirror-image walk to permanent
//! deafness on silence. **That class of bug is now unrepresentable**: the fields
//! *are* the register values, so there is no second convention to disagree with.
//!
//! This module stays free of ESP-IDF imports so `tests/host/defence.rs` can
//! compile it directly; the register writes live in `session::apply`.

/// One register's slice of the packed number.
///
/// Mask and shift rather than a hand-written accessor each, so the layout is a
/// table that can be reordered rather than four functions that have to be
/// reordered in step with each other.
pub struct Field {
    /// Short label for the console — `describe` builds its line from these.
    pub name: &'static str,
    /// Bit position of the field's least significant bit.
    pub shift: u32,
    pub width: u32,
    /// Share of the gauge, in percent.
    ///
    /// **Harm, not magnitude.** The packed value is dominated by whichever
    /// register holds the top bits, and that is `NF_LEV` — the one knob with
    /// essentially no detection cost. So a raw-scaled bar read 92 % while the
    /// device was in fact reporting every strike. These weights answer the
    /// question a person actually asks of a bar: how deaf to lightning am I.
    ///
    /// Judgement, not measurement: the datasheet gives no detection-probability
    /// curves to derive them from, so they encode the cost ordering
    /// `NF_LEV` ≪ `WDTH` < `SREJ` and nothing more.
    pub weight: u32,
}

impl Field {
    /// The field's bits, in place.
    pub const fn mask(&self) -> u16 {
        ((1u16 << self.width) - 1) << self.shift
    }

    /// The largest value this field can hold.
    pub const fn ceiling(&self) -> u8 {
        ((1u16 << self.width) - 1) as u8
    }

    /// Read this field out of a packed value.
    pub const fn get(&self, raw: u16) -> u8 {
        ((raw & self.mask()) >> self.shift) as u8
    }

    /// Write this field into a packed value, clamping to the field's width.
    pub const fn set(&self, raw: u16, value: u8) -> u16 {
        let capped = if value > self.ceiling() {
            self.ceiling()
        } else {
            value
        };
        (raw & !self.mask()) | ((capped as u16) << self.shift)
    }
}

/// Index into [`FIELDS`], named so the accessors below read as English.
pub const NOISE_FLOOR: usize = 0;
pub const WATCHDOG: usize = 1;

/// What `SREJ` is programmed to unless the operator says otherwise.
///
/// **Zero, matching the reference driver and this project's own working Python
/// setup**, and a *setting* rather than a field, which is the whole point of
/// this constant.
///
/// It was 1 -- "between the reference driver's 0 and the datasheet's 2" -- until
/// the storm of 2026-08-19, when `deep_demo.py` turned out to be the record of
/// what had actually worked here: `setSpikeRejection(0)` beside
/// `setWatchdogThreshold(2)`. The watchdog is the filter that earns its keep on
/// this board; spike rejection was rejecting the weather with the hammers.
///
/// The measurements it sits between, all on this board: at **0** the chip
/// logged 503 false strikes in three and a half hours from electric hammers
/// next door. At **2** that fell to five in eight hours — but the same period
/// missed 5–6 real strikes an hour at 10+ km, which is the cost of rejection
/// showing up on the other side. At **8** nothing man-made got through at all.
///
/// One is chosen against that spread rather than measured at it. It is the
/// least rejection that is not none, on the reasoning that a detector which
/// reports hammers is useless and a detector which misses distant storms is
/// merely limited — and that the interference this room suffers is better
/// answered by moving the sensor than by deafening it.
///
/// SREJ rejects short man-made impulses, and it is the only knob that does.
/// While it was in the search space the tuner treated it as the cheapest thing
/// to give away — relaxation refunds the least valuable field first, and by the
/// sensitivity weighting that is SREJ — so every quiet spell walked it to zero.
/// At zero the chip validated an electric hammer on a neighbouring roof as
/// lightning: 503 "strikes" in three and a half hours on 2026-08-14 with no
/// storm within range, statistically indistinguishable from a real one.
///
/// The sweep could not have caught it either. `calibrate` searches for the most
/// sensitive point that stays *quiet*, and in a quiet room `sr 0` scores
/// perfectly because there is nothing to reject. Quiet is not correct.
pub const SPIKE_REJECTION_DEFAULT: u8 = 0;

/// The range `srej <n>` accepts. The register is four bits.
pub const SPIKE_REJECTION_MAX: u8 = 15;

/// What `MIN_NUM_LIGH` is programmed to, always: **report every strike.**
///
/// **Not a field, because it is not tunable.** It gates lightning *reporting*
/// only — the chip still validates, still fires `NoiseTooHigh`, still fires
/// `Disturber` — so raising it cannot reduce the number the tuner measures,
/// while costing the first 4, 8 or 15 strikes of a storm. Pure loss against this
/// objective.
///
/// It was briefly kept in the space and merely excluded from the walk. That left
/// the calibration sweep still setting it — which is how a 60 s sweep settled on
/// `ms 2`, waiting nine strikes — and needed a shift, a second maximum and a
/// consistency check to hold the two halves in step. A constant needs none of
/// that: what cannot be varied should not be representable.
pub const MIN_STRIKES_COUNT: u8 = 1;

/// **The layout.** Most valuable first — reorder the `shift` column to try the
/// opposite arrangement.
pub const FIELDS: [Field; 2] = [
    Field { name: "nf", shift: 4, width: 3, weight: 20 },
    Field { name: "wd", shift: 0, width: 4, weight: 80 },
];

/// Total width of the packed point.
pub const BITS: u32 = 7;

/// One value per bit pattern: 128 of them, 0..=127.
///
/// **Seven bits, down from eleven.** `MIN_NUM_LIGH` left first because it
/// suppresses strikes outright; `SREJ` followed because the tuner reliably
/// walked it to zero and zero admits man-made impulses as lightning. What is
/// left is the two knobs that are genuinely volume controls, and a sweep over
/// them is seven probes rather than eleven.
pub const MAX: u16 = (1u16 << BITS) - 1;

// **`SREJ` is deliberately NOT capped**, though an earlier design capped it at
// 11 on the grounds that the datasheet's curves flatten past there and the last
// settings reject hard enough to discard a genuine nearby strike.
//
// A cap cannot coexist with a dense packed space, and the failure is not subtle.
// Clamping on construction makes four of every sixteen `SREJ` steps fold back
// onto 11, so the ±1 tuner walking up from `sr 11, ms 3` (raw 47) lands on raw
// 48, which clamps to raw 44 — and then cycles 44, 45, 46, 47, 44 forever,
// unable to ever climb past spike rejection 11. It also makes `percent` dip as
// the number rises, and puts the ceiling at 8175 so the gauge tops out at 99 %.
//
// The cap is not needed anyway: `session::calibrate` bisects for the **lowest**
// quiet point, so it prefers weak spike rejection without being told to, and the
// runtime tuner only climbs as far as the noise forces it. The judgement the cap
// encoded is now a property of the search rather than a wall in the space.

/// The whole defence configuration: four register fields in one integer.
///
/// A newtype rather than a bare `u16` so a raw count from the search cannot be
/// mistaken for a register value — and so `percent`, the field accessors and the
/// clamping all have one home.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct Point(u16);

impl Point {
    /// Fully receptive: every field at zero.
    ///
    /// This is the chip at its most sensitive, which is where a fresh boot, a
    /// `sensitive off` and every calibration start.
    pub const OPEN: Point = Point(0);

    /// **Where a device starts when it has never calibrated.**
    ///
    /// Each field at the middle of its range — *except* spike rejection and min
    /// strikes, which start at their most sensitive. That split is the whole
    /// point of the default:
    ///
    /// * `NF_LEV` and `WDTH` only decide how loud a signal has to be. Starting
    ///   them mid-range costs some distant strikes and buys a device that is not
    ///   drowning on its first window. Booting fully open was measured here at
    ///   7–9 noise events per batch, continuously, which is a receiver with
    ///   nothing left to say about anything.
    /// `SREJ` and `MIN_NUM_LIGH` are no longer here to be started at anything.
    /// Both decide whether a strike is *reported at all* rather than how loud
    /// the receiver is, and both left the space for that reason — see
    /// [`SPIKE_REJECTION_DEFAULT`] and [`MIN_STRIKES_COUNT`].
    ///
    /// Computed from [`FIELDS`] rather than written as a literal, so reordering
    /// the layout moves this with it.
    pub fn default_start() -> Point {
        Point::pack(
            FIELDS[NOISE_FLOOR].ceiling() / 2,
            FIELDS[WATCHDOG].ceiling() / 2,
        )
    }

    /// Build from a raw packed value, clamping to the representable range.
    ///
    /// The only clamp: every one of the 8192 values maps to a distinct, legal
    /// register combination, so nothing else needs adjusting on the way in.
    pub fn new(raw: u16) -> Point {
        Point(raw.min(MAX))
    }

    /// The packed value, for storage and for the search.
    pub fn raw(self) -> u16 {
        self.0
    }

    /// One field, by [`FIELDS`] index.
    pub fn field(self, index: usize) -> u8 {
        FIELDS[index].get(self.0)
    }

    pub fn noise_floor(self) -> u8 {
        self.field(NOISE_FLOOR)
    }

    pub fn watchdog(self) -> u8 {
        self.field(WATCHDOG)
    }

    /// Always [`MIN_STRIKES_COUNT`]. Kept as a method so callers that report
    /// the chip's configuration do not have to know it is fixed.
    pub fn min_strikes_count(self) -> u8 {
        MIN_STRIKES_COUNT
    }

    /// Assemble from the three field values, each clamped to its width.
    ///
    /// **Unused by the firmware**, which only ever walks raw integers — kept
    /// because it is how `tests/host/defence.rs` states what a layout should
    /// produce, and a packing with no way to write it a field at a time is a
    /// packing nobody can check.
    #[allow(dead_code)]
    pub fn pack(noise_floor: u8, watchdog: u8) -> Point {
        let mut raw = 0u16;
        raw = FIELDS[NOISE_FLOOR].set(raw, noise_floor);
        raw = FIELDS[WATCHDOG].set(raw, watchdog);
        Point(raw)
    }

    /// One notch deafer, or `None` when every field is at its ceiling.
    ///
    /// **Not `raw + 1`, and the reason is the mirror of [`Point::relaxed`].**
    /// Incrementing the packed number moves the *bottom* bits, which are
    /// `MIN_NUM_LIGH` — so the first answer to a noisy minute would be "wait
    /// five strikes", and a room that stays noisy grinds up through all of
    /// `SREJ` and `MIN_NUM_LIGH` before it reaches the watchdog. Traced against
    /// measured rates on this board: relaxing off `wd 7` lands on `wd 6` at
    /// 13–17/min, and climbing back by ones takes 63 minutes spent almost
    /// entirely at "wait 5" or worse.
    ///
    /// So this walks [`FIELDS`] **cheapest first** — the same cost order the bit
    /// layout encodes, read forwards: `NF_LEV`, which cannot reject a strike, is
    /// spent before `WDTH`, and `MIN_NUM_LIGH` only when nothing else is left.
    /// `relaxed` walks the same list backwards, refunding the dearest first. The
    /// pair is not a strict inverse and does not need to be; what matters is
    /// that the device is reluctant to go deaf and eager to come back.
    ///
    /// The raw number still rises on every step, so the gauge and
    /// [`Point::percent`] keep their direction.
    pub fn tightened(self) -> Option<Point> {
        for index in 0..FIELDS.len() {
            let value = self.field(index);
            if value < FIELDS[index].ceiling() {
                return Some(Point(FIELDS[index].set(self.0, value + 1)));
            }
        }
        None
    }

    /// One notch gentler, or `None` when already fully open.
    ///
    /// **Not `raw - 1`.** Decrementing the packed number is only a relaxation
    /// when it does not borrow, and it borrows exactly when the low fields are
    /// zero — which is the common case, because a calibration settles with them
    /// zero. Measured on this board: the sweep settled at 448 (`wd 7, sr 0,
    /// ms 0`, reporting every strike) and one decrement produced 447, which is
    /// `wd 6, sr 15, ms 3` — waiting for sixteen strikes. The device then walked
    /// on down, because a chip waiting for sixteen strikes hears nothing, which
    /// reads as "no noise, relax".
    ///
    /// So relax the **least significant non-zero field** instead. From 448 that
    /// is the watchdog, giving 384 (`wd 6, sr 0, ms 0`) — gentler in exactly one
    /// register and unchanged in the rest, which is what the word means.
    ///
    /// Climbing stays plain `raw + 1`: a borrow on the way up can only carry
    /// *out* of the destructive low fields, which resets them toward reporting
    /// every strike rather than away from it.
    pub fn relaxed(self) -> Option<Point> {
        // Least significant first, which is the reverse of the table order.
        for index in (0..FIELDS.len()).rev() {
            let value = self.field(index);
            if value > 0 {
                return Some(Point(FIELDS[index].set(self.0, value - 1)));
            }
        }
        None
    }

    /// How deaf to lightning the device currently is, 0–100.
    ///
    /// **A weighted sum, not the raw value scaled.** The raw number is dominated
    /// by whichever register holds the top bits, which is `NF_LEV` — and that is
    /// the one register with essentially no detection cost. Measured on this
    /// board: `nf 7, wd 6, sr 0, ms 0` read **92 %** while the device was
    /// reporting every single strike. The bar was alarming about the harmless
    /// knob and silent about the dangerous ones.
    ///
    /// Each walkable register contributes its [`Field::weight`] scaled by how
    /// far up its own range it sits, so the same point now reads **25 %**.
    ///
    /// **`MIN_NUM_LIGH` overrides everything.** It cannot be spent by the walk,
    /// so a non-zero value means somebody set it by hand — and any value above
    /// zero silences the opening of every storm, which is maximal harm for an
    /// early-warning device whatever the other three are doing. A bar that
    /// averaged that away would be hiding the worst state the part can be in.
    ///
    /// The consequence, stated plainly: this is **not** monotonic in the raw
    /// value any more. It is a harm reading, not a position in the search space.
    pub fn percent(self) -> u32 {
        let mut total = 0u32;
        for (index, field) in FIELDS.iter().enumerate() {
            let ceiling = field.ceiling() as u32;
            if ceiling == 0 {
                continue;
            }
            total += field.weight * self.field(index) as u32 / ceiling;
        }
        total.min(100)
    }
}
