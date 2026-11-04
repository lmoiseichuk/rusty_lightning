//! lightning project, Copyright (c) 2026 Leonid Moiseichuk under MIT License
//!
//! How hard the sensor is trying to reject noise (§4.2), as **one 3-bit
//! number**.
//!
//! ## What the tuner is allowed to spend
//!
//! The AS3935 exposes four knobs for this. Only one of them is free, and the
//! search space is now that one knob and nothing else:
//!
//! * **`NF_LEV`** is a *noise-floor* gate. It decides when the chip complains
//!   the band is noisy, and **cannot reject a lightning waveform** — the only
//!   free knob on the part, and now the whole of the tuning point.
//! * **`WDTH`** is the watchdog *amplitude* gate. Raising it discards weaker
//!   arrivals, **distant strikes first**. A *setting* — see
//!   [`WATCHDOG_DEFAULT`].
//! * **`SREJ`** compares the signal against the chip's lightning *waveform
//!   template*. Raising it discards anything imperfectly shaped. A setting —
//!   see [`SPIKE_REJECTION_DEFAULT`].
//! * **`MIN_NUM_LIGH`** makes the chip wait for a *pattern* before reporting
//!   anything. Pinned — see [`MIN_STRIKES_COUNT`].
//!
//! ## Why the space kept shrinking
//!
//! It began as one 13-bit integer over all four, ordered so bisection settled
//! the cheap knob first and the destructive one last. That ordering was the
//! right instinct about a wrong premise: **a search that can reach a knob will
//! eventually spend it**, and three of these four are paid for in strikes.
//!
//! Each eviction was forced by a measurement, and each has its own note:
//! `MIN_NUM_LIGH` first, because it suppresses strikes outright; then `SREJ`,
//! because the tuner reliably walked it to zero and zero admits an electric
//! hammer as lightning; and now `WDTH`.
//!
//! **`WDTH` left because of what this device is for.** It is an early-warning
//! device: the strikes worth having are the distant ones, arriving before the
//! thunder does, and those are the weakest returns on the board — exactly what
//! the watchdog discards first. A tuner minimising noise reports and a person
//! wanting to see a storm approach are optimising against each other, and the
//! tuner was winning. The weights recorded the conflict before anyone acted on
//! it: `wd` carried 80 % of the harm against `nf`'s 20 %.
//!
//! Two things follow, and both are wanted:
//!
//! * **A sweep costs nothing that matters.** Three probes rather than seven,
//!   over a knob that cannot reject lightning — so the deaf-time budget that
//!   forced sweeps six hours apart no longer applies, and a perturbation to
//!   escape a stuck point is affordable rather than dangerous.
//! * **A noisy band is no longer a problem to be solved.** Hundreds of
//!   disturbers a minute cost nothing if every strike still arrives, and this
//!   site produces them. What the tuner still owes is a chip that is not
//!   sitting in permanent `NoiseTooHigh`, which is precisely and only `NF_LEV`.
//!
//! **[`FIELDS`] is the layout**, one row of mask-and-shift per register.
//! `tests/defence.rs` checks that whatever is in the table tiles
//! [`BITS`] with no gap and no overlap — which is still worth checking with one
//! row, because it is what would catch a second field being added back without
//! [`BITS`] following it.
//!
//! Every one of the values is a distinct, legal combination — no field is
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
//! This module stays free of ESP-IDF imports so `tests/defence.rs` can
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

/// What `WDTH` is programmed to unless the operator says otherwise.
///
/// **Zero, measured against a storm on 2026-09-10** -- outdoor gain, `nf 1`,
/// `srej 1`, one audible strike every thirty seconds:
///
/// ```text
/// wd 2:  0 strikes,  11 disturbers  (100 s)
/// wd 0:  2 strikes, 294 disturbers   (90 s)
/// ```
///
/// Not "fewer strikes". *No* strikes, through a storm loud enough to count by
/// ear. The watchdog is an amplitude gate ahead of every classifier on the
/// chip, so at 2 the returns were discarded before anything could decide what
/// they were -- which is why nothing downstream could have recovered them and
/// why the log looked like calm weather.
///
/// **It was two because of a pairing this firmware no longer runs.**
/// `deep_demo.py` set `setWatchdogThreshold(2)` beside `setSpikeRejection(0)`,
/// and the 2 was inherited from that record while the 0 was deliberately left
/// behind -- `SPIKE_REJECTION_DEFAULT` is not 0 here, for reasons written there.
/// Half of a measured pair is not a measured value.
///
/// **The disturber mask is what makes zero affordable.** The 294 above is the
/// bill for opening the gate, and until `boot::configure` started masking
/// `INT_D` that flood cost missed edges on the wire strikes share. The two
/// defaults changed together and belong together: one admits weak returns, the
/// other keeps the interrupt path free enough to carry them.
///
/// This is also what the module header already argued for -- `wd` carries 80 %
/// of the harm against `nf`'s 20 %, and the weakest arrivals are the distant
/// strikes this device exists to see before the thunder. Taking `WDTH` out of
/// the tuner's reach stopped a loop from spending them; it left a hand-written
/// 2 spending them just as effectively.
///
/// Still adjustable, and still upward: a site with a different noise floor may
/// genuinely need some, chosen by a person who has decided to trade distance
/// for quiet.
pub const WATCHDOG_DEFAULT: u8 = 0;

/// The range `wdth <n>` accepts. The register is four bits.
pub const WATCHDOG_MAX: u8 = 15;

/// What `SREJ` holds after a reset: **2**, `0010` -- datasheet v1-04, the
/// register table, page 19.
pub const SPIKE_REJECTION_POWER_ON: u8 = 2;

/// What `SREJ` is programmed to unless the operator says otherwise.
///
/// **Two, the chip's own power-on value, since 0.22.0** -- and a *setting*
/// rather than a field, which is the whole point of this constant.
///
/// ## Why it moved back up from 1
///
/// One was chosen against a spread rather than measured at a point: "the
/// least rejection that is not none". The two ends of that spread, both on
/// this board:
///
/// * At **0** the chip validated an electric hammer on a neighbouring roof as
///   lightning: 503 "strikes" in three and a half hours on 2026-08-14, every
///   one `overhead`. That is what took `SREJ` out of the tuner's reach -- the
///   tuner had walked it to zero on every quiet spell.
/// * At **1**, from 2026-09-16 to 09-24, it let through 9,542 "strikes", 99 %
///   `overhead`, round the clock with 7-10 s between them -- false, and fed to
///   the golden record as "840 strikes heard here". The 2026-11-01 outing
///   that heard real lightning at 1 (seven strikes at 6-14 km) logged six
///   `overhead` beside them, and three more that afternoon landed on
///   electrical events.
///
/// And at **2**, in August: five false strikes in eight hours where 0 had
/// logged 503 -- at the cost of 5-6 real strikes an hour missed at 10+ km,
/// which is rejection showing up on the other side. At **8** nothing man-made
/// got through at all.
///
/// So 1 is not "the least rejection that works"; it is a value that has let a
/// man-made stream through for nine days. 2 is what the chip resets to, and
/// what [`crate::as3935::As3935::set_spike_rejection`] has always said a storm
/// was seen to work at. Whether 1 or 2 is right for this room is
/// mine to judge in the field; until then the resting value is the one with
/// the shorter list of known failures, and `srej 1` is one command away.
///
/// **A stored value is a person's, and the device does not change it on its
/// own account.** Only `srej <n>` ever writes one, so a board holding 1 was
/// told 1. Boot honours it and says, every time, that it is below this default
/// -- see [`spike_rejection_below_default`].
///
/// **The one thing that overrules it is a measurement**: a trusted golden
/// record at a more permissive value, at boot ([`spike_rejection_at_boot`]),
/// the rule since 2629a2d19aeb measured a stored watchdog deaf through a live
/// storm. That cuts both ways for a person's choice. A `srej 2` typed while
/// the record is untrusted, or before there is one, stands: the strikes after
/// it are heard at 2 and the record follows them. A `srej 2` typed after two
/// strikes with a distance have made a record at 1 lasts until the next
/// restart -- every open of the console port -- unless `golden clear` comes
/// first. Boot and `srej` both say so at the moment it matters.
///
/// ## The history, from `git log`, kept because it went wrong before
///
/// * **2** when it left the tuning space, 2026-08-14 (392335916765).
/// * **1** the same day -- "between the reference driver's 0 and the
///   datasheet's 2" (45e55184c746), whose whole subject was this decision.
/// * **0** on 2026-08-19 (aa155a727975), a commit about noticing a storm that
///   mentions spike rejection nowhere. Nothing failed, because nothing checked
///   a default against the reasoning printed next to it -- the device simply
///   ran the electric-hammer configuration for nine days, and the two
///   "overhead" strikes it logged on the morning of 2026-08-29, one of them
///   carrying zero energy, are what that looks like from the outside.
/// * **1** on 2026-08-29 (39577cae0dae), with the rule that the device may
///   not choose zero for itself.
/// * **2** in 0.22.0, for the reasons above. `tests/defence.rs` holds the value
///   to the reasoning rather than to a number: the default is the power-on
///   value.
///
/// SREJ rejects short man-made impulses, and it is the only knob that does.
/// While it was in the search space the tuner treated it as the cheapest thing
/// to give away — relaxation refunds the least valuable field first, and by the
/// sensitivity weighting that is SREJ — so every quiet spell walked it to zero.
///
/// The sweep could not have caught it either. `calibrate` searches for the most
/// sensitive point that stays *quiet*, and in a quiet room `sr 0` scores
/// perfectly because there is nothing to reject. Quiet is not correct.
pub const SPIKE_REJECTION_DEFAULT: u8 = SPIKE_REJECTION_POWER_ON;

/// The lowest spike rejection the device may choose **for itself**.
///
/// **A person may set zero; the device may not choose it.** That is the whole
/// distinction, and it is the same one that took `SREJ` out of the tuning space
/// in the first place: what the machine cannot write, it cannot spend. An
/// operator asking for `srej 0` is running an experiment and knows what it
/// costs. A default, a fallback or a reset arriving at 0 on its own is the
/// device quietly re-entering the configuration that logged 503 false strikes
/// in three and a half hours.
///
/// So every automatic path clamps to this, and only the console can go below.
/// A stored 0 is honoured on the next boot, because restoring what somebody
/// deliberately set is not the device choosing.
pub const SPIKE_REJECTION_AUTO_MIN: u8 = 1;

/// What the device should use when it is choosing for itself.
///
/// `stored` is whatever NVS holds, or `None` on a device that has never been
/// told. Never returns below [`SPIKE_REJECTION_AUTO_MIN`] unless a person put
/// the value there.
pub fn spike_rejection_for_autorun(stored: Option<u8>) -> u8 {
    match stored {
        // Somebody set this deliberately, including zero. Honour it.
        Some(level) => level,
        // Nothing stored: the device is choosing, so it does not choose zero.
        None => SPIKE_REJECTION_DEFAULT.max(SPIKE_REJECTION_AUTO_MIN),
    }
}

/// Whether a stored spike rejection sits below the default, though not at the
/// zero the console already warns about -- the case boot announces.
///
/// **Announced, never migrated.** See [`SPIKE_REJECTION_DEFAULT`]: a stored
/// value was put there by a person, so the device keeps it and says so.
pub fn spike_rejection_below_default(stored: Option<u8>) -> bool {
    match stored {
        Some(level) => level >= SPIKE_REJECTION_AUTO_MIN && level < SPIKE_REJECTION_DEFAULT,
        None => false,
    }
}

/// The spike rejection boot programs, from what NVS holds and what a trusted
/// golden record heard lightning at.
///
/// `heard` is the record's `srej`, and **only a record the device acts on may
/// supply it** -- one counted since 0.22.0, trusted, at this gain. A record
/// from before 0.22.0 is set aside by `settings`, so it never reaches this,
/// which is what stops its `sr 1` dragging a board back down after its owner
/// has moved on.
///
/// The rule is the one `boot` documents: **the more permissive of the two
/// wins** -- a person who set something more open keeps it, and a stored
/// value deafer than one that has heard lightning is overruled -- with two
/// edges:
///
/// * **Nothing stored and no record: the default**, never zero.
/// * **A record cannot take it below [`SPIKE_REJECTION_AUTO_MIN`].** A person
///   who sets `srej 0` keeps it, because that is a person's experiment; a
///   record made during that experiment lowering a later `srej 1` back to 0
///   would be the device choosing zero. Boot's comment claimed this guard
///   while the code passed the record's value through
///   `spike_rejection_for_autorun`, which returns a stored 0 unchanged; this
///   is where it is actually enforced.
pub fn spike_rejection_at_boot(stored: Option<u8>, heard: Option<u8>) -> u8 {
    let heard = match heard {
        Some(level) => Some(level.max(SPIKE_REJECTION_AUTO_MIN)),
        None => None,
    };
    let chosen = match (stored, heard) {
        (Some(set), Some(heard)) => Some(set.min(heard)),
        (None, Some(heard)) => Some(heard),
        (stored, None) => stored,
    };
    spike_rejection_for_autorun(chosen)
}

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
pub const FIELDS: [Field; 1] = [
    Field { name: "nf", shift: 0, width: 3, weight: 100 },
];

/// Total width of the packed point.
pub const BITS: u32 = 3;

/// One value per bit pattern: eight of them, 0..=7.
///
/// **Three bits, down from seven, down from eleven.** Each step removed a knob
/// that was paid for in strikes: `MIN_NUM_LIGH` because it suppresses them
/// outright, `SREJ` because the tuner reliably walked it to zero and zero
/// admits man-made impulses as lightning, `WDTH` because it discards the
/// distant arrivals this device exists to report.
///
/// What is left is the one knob that cannot cost a strike, which is why a sweep
/// is now three probes and why perturbing the point is safe.
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
    /// `NF_LEV` at the middle of its range.
    ///
    /// **Mid-range rather than open, and this is the one measurement that says
    /// so.** Booting fully open was measured here at 7–9 noise events per batch,
    /// continuously — a receiver with nothing left to say about anything. The
    /// same figure turned up again on 2026-08-23 at indoor gain with `nf 0`
    /// pinned: 5–8 noise per batch and not one strike through a live storm.
    ///
    /// Starting mid-range costs nothing that matters, because `NF_LEV` cannot
    /// reject a lightning waveform. That is the difference from the knobs that
    /// used to start here beside it: `WDTH`, `SREJ` and `MIN_NUM_LIGH` all
    /// decide whether a strike is reported at all rather than how loud the room
    /// is, and all three left the space for that reason — see
    /// [`WATCHDOG_DEFAULT`], [`SPIKE_REJECTION_DEFAULT`] and
    /// [`MIN_STRIKES_COUNT`].
    ///
    /// Computed from [`FIELDS`] rather than written as a literal, so widening
    /// the field moves this with it.
    pub fn default_start() -> Point {
        Point::pack(FIELDS[NOISE_FLOOR].ceiling() / 2)
    }

    /// Build from a raw packed value, clamping to the representable range.
    ///
    /// The only clamp: every one of the eight values maps to a distinct, legal
    /// `NF_LEV` setting, so nothing else needs adjusting on the way in. That was
    /// the property the packed layout was built for when the space held four
    /// registers and 8192 values; with one three-bit field left it is close to
    /// trivially true, which is the point — the invariant survived the space
    /// shrinking around it.
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

    /// **Deliberately absent: there is no `watchdog()`.** `WDTH` is no longer
    /// part of the point, so a point cannot answer what the watchdog is — the
    /// setting can, and callers read it from `settings::watchdog()`. A method
    /// here returning [`WATCHDOG_DEFAULT`] would be the same lie the old field
    /// told, that this is something the tuner decides.
    ///
    /// Always [`MIN_STRIKES_COUNT`]. Kept as a method so callers that report
    /// the chip's configuration do not have to know it is fixed.
    pub fn min_strikes_count(self) -> u8 {
        MIN_STRIKES_COUNT
    }

    /// Assemble from the three field values, each clamped to its width.
    ///
    /// **Unused by the firmware**, which only ever walks raw integers — kept
    /// because it is how `tests/defence.rs` states what a layout should
    /// produce, and a packing with no way to write it a field at a time is a
    /// packing nobody can check.
    #[allow(dead_code)]
    pub fn pack(noise_floor: u8) -> Point {
        let mut raw = 0u16;
        raw = FIELDS[NOISE_FLOOR].set(raw, noise_floor);
        Point(raw)
    }

    /// One notch deafer, or `None` when every field is at its ceiling.
    ///
    /// One step deafer: the next `NF_LEV` up, or `None` at the ceiling.
    ///
    /// **With one field left this is `raw + 1`**, and the machinery around it
    /// looks like more than it is. It stays because the shape is the argument,
    /// not the arithmetic: `tightened` walks [`FIELDS`] *cheapest first* and
    /// `relaxed` walks it backwards, so whatever is in the table is spent in
    /// cost order and refunded in the reverse. Put a register back in the table
    /// and the ordering is already right.
    ///
    /// That ordering was learned expensively. While the point packed four
    /// registers, `raw + 1` moved the *bottom* bits — `MIN_NUM_LIGH` — so the
    /// first answer to a noisy minute was "wait five strikes", and a room that
    /// stayed noisy ground up through `SREJ` and `MIN_NUM_LIGH` before it
    /// reached the watchdog. Measured on this board: relaxing off `wd 7` landed
    /// on `wd 6` at 13–17/min, and climbing back by ones took 63 minutes spent
    /// almost entirely at "wait 5" or worse. Those states cannot occur now —
    /// the tuner cannot reach those registers — but the reason they were
    /// unreachable is this ordering.
    ///
    /// The raw number rises on every step, so the gauge and [`Point::percent`]
    /// keep their direction.
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
    /// So relax the **least significant non-zero field** instead — gentler in
    /// exactly one register and unchanged in the rest, which is what the word
    /// means. With `NF_LEV` alone in the table that is plain `raw - 1`; the
    /// walk is kept for the same reason as in [`Point::tightened`].
    ///
    /// The worked example this used to give — 448 relaxing to 384, `wd 6, sr 0,
    /// ms 0` — describes a point that no longer exists. `MAX` is 7.
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
    /// **A weighted sum over [`FIELDS`], not the raw value scaled.** With one
    /// field of weight 100 that reduces to `100 · nf / 7`, and it is monotonic
    /// in the raw value again — which it was not while the space held four
    /// registers, and the difference is worth keeping written down.
    ///
    /// The raw number was then dominated by whichever register held the top
    /// bits, `NF_LEV`, which is the one register with essentially no detection
    /// cost. Measured on this board: `nf 7, wd 6, sr 0, ms 0` read **92 %**
    /// while the device was reporting every single strike — alarming about the
    /// harmless knob and silent about the dangerous ones. Weighting each
    /// register by its own cost brought the same point to **25 %**.
    ///
    /// Those registers left the point entirely, so the weighting has nothing
    /// left to correct. It stays because it is what makes the bar mean "how
    /// deaf" rather than "how far up the search space", and that distinction
    /// comes straight back the moment anything rejoins [`FIELDS`].
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
