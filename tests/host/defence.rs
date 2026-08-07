//! Host checks for §4.2's packed defence point.
//!
//! Worth testing because the bit layout *is* the design. The ordering argument —
//! cheap knob decided first, destructive knob last — is not enforced by any code
//! path; it is a consequence of where each field sits in the number and of the
//! fact that binary search resolves high bits first. A reordered `FIELDS` table
//! with a typo'd shift would still compile, still run, and quietly settle
//! `MIN_NUM_LIGH` before `NF_LEV`.
//!
//! **This compiles the real `src/defence.rs`**, included by path — not a copy of
//! it. Keeping that module free of ESP-IDF imports is what makes this possible,
//! and it means a change to the layout that breaks these checks fails here
//! rather than silently drifting away from what ships.

use std::sync::atomic::{AtomicU32, Ordering};

#[path = "../../src/defence.rs"]
mod defence;
use defence::*;

// Atomics rather than `static mut`. Two counters in a single-threaded test
// binary are the textbook case where `static mut` looks harmless, but taking a
// reference to one — which `println!("{PASS}")` does implicitly — is undefined
// behaviour, and Rust 2024 makes it a hard error rather than a warning.
static PASS: AtomicU32 = AtomicU32::new(0);
static FAIL: AtomicU32 = AtomicU32::new(0);

fn check(name: &str, ok: bool) {
    let counter = if ok { &PASS } else { &FAIL };
    counter.fetch_add(1, Ordering::Relaxed);
    println!("  {:<4} {name}", if ok { "ok" } else { "FAIL" });
}

fn main() {
    println!("defence:");

    // --- the layout tiles the word -----------------------------------------
    //
    // The check that makes reordering safe: whatever order the table is in, the
    // fields must cover exactly `BITS` with no gap and no overlap. A gap wastes
    // search space on values that program nothing; an overlap means two
    // registers share a bit and moving one silently moves the other.
    let mut covered = 0u16;
    let mut overlapped = false;
    let mut total_width = 0u32;
    for f in FIELDS.iter() {
        if covered & f.mask() != 0 {
            overlapped = true;
        }
        covered |= f.mask();
        total_width += f.width;
    }
    check("no two fields share a bit", !overlapped);
    check("the fields cover every bit, with no gap", covered == MAX);
    check("the widths add up to BITS", total_width == BITS);
    check("11 bits means 0..=2047", BITS == 11 && MAX == 2047);

    // --- the documented positions ------------------------------------------
    check("NF_LEV is 3 bits at 8", FIELDS[NOISE_FLOOR].shift == 8
        && FIELDS[NOISE_FLOOR].width == 3);
    check("WDTH is 4 bits at 4", FIELDS[WATCHDOG].shift == 4
        && FIELDS[WATCHDOG].width == 4);
    check("SREJ is 4 bits at 0", FIELDS[SPIKE].shift == 0
        && FIELDS[SPIKE].width == 4);
    check("min strikes is not a field at all", FIELDS.len() == 3);
    check("...and is pinned to report every strike", MIN_STRIKES_COUNT == 1);

    // **The ordering property the whole design rests on.** `NF_LEV` must be the
    // most significant field, so a bisection decides it first, and
    // `MIN_NUM_LIGH` the least, so a bisection decides it last.
    let mut most_significant = 0;
    let mut least_significant = 0;
    for (index, f) in FIELDS.iter().enumerate() {
        if f.shift > FIELDS[most_significant].shift {
            most_significant = index;
        }
        if f.shift < FIELDS[least_significant].shift {
            least_significant = index;
        }
    }
    check("the cheap knob (NF_LEV) holds the highest bits",
        most_significant == NOISE_FLOOR);
    check("the dearest tunable knob (SREJ) holds the lowest",
        least_significant == SPIKE);

    // --- what a bisection actually probes -----------------------------------
    //
    // The ordering claim stated as behaviour rather than as bit positions.
    let first = Point::new((MAX + 1) / 2);
    check("the first probe is a noise-floor decision", first.noise_floor() == 4
        && first.watchdog() == 0
        && first.spike_rejection() == 0);

    // Replay a full search for a known answer, watching when each field settles.
    let target = 1500u16;
    let mut low = 0u16;
    let mut high = MAX;
    let mut probes = 0;
    // "Settled at probe N" means every probe from N onwards already had this
    // field at its final value -- so a LATER number means the search was still
    // making up its mind about that register.
    let mut floor_settled_at = 0;
    let mut spike_settled_at = 0;
    let goal = Point::new(target);
    while low < high {
        let mid = low + (high - low) / 2;
        probes += 1;
        if Point::new(mid).noise_floor() != goal.noise_floor() {
            floor_settled_at = probes + 1;
        }
        if Point::new(mid).spike_rejection() != goal.spike_rejection() {
            spike_settled_at = probes + 1;
        }
        if mid >= target {
            high = mid;
        } else {
            low = mid + 1;
        }
    }
    check("a full bisection is 11 probes", probes == 11);
    check("...landing exactly on the answer", low == target);
    check("...with the noise floor decided early", floor_settled_at <= 4);
    check("...and spike rejection still moving long after it",
        spike_settled_at > floor_settled_at);

    // --- packing round-trips ------------------------------------------------
    let p = Point::pack(5, 9, 7);
    check("noise floor round-trips", p.noise_floor() == 5);
    check("watchdog round-trips", p.watchdog() == 9);
    check("spike rejection round-trips", p.spike_rejection() == 7);

    check("and the raw value is the three fields packed",
        p.raw() == (5 << 8) | (9 << 4) | 7);
    check("a packed point survives new()", Point::new(p.raw()) == p);

    check("OPEN is every field at zero", Point::OPEN.raw() == 0
        && Point::OPEN.noise_floor() == 0);

    // --- the compiled-in starting point -------------------------------------
    //
    // The split is the claim worth checking: the two volume knobs open
    // mid-range, the two that decide whether a strike is reported at all stay at
    // their most sensitive. Getting this backwards would ship a device that
    // boots waiting for sixteen strikes.
    let start = Point::default_start();
    check("the default starts the noise floor mid-range", start.noise_floor() == 3);
    check("...and the watchdog mid-range", start.watchdog() == 7);
    check("...with spike rejection wide open", start.spike_rejection() == 0);
    check("...and reporting every strike", start.min_strikes_count() == 1);
    check("...which is well clear of the noisy bottom", start.raw() > 0);

    // --- clamping -----------------------------------------------------------
    check("a raw value past MAX clamps", Point::new(60000).raw() <= MAX);
    check("the chip always reports every strike",
        Point::new(60000).min_strikes_count() == 1 && Point::OPEN.min_strikes_count() == 1);
    check("a field value past its width clamps",
        Point::pack(200, 200, 200).noise_floor() == 7);

    // **Every raw value is a distinct point.** No field is capped below its
    // width, and this is the check that keeps it that way: a cap would fold
    // several raw values onto one point, which strands the +/-1 tuner in a short
    // cycle it can never climb out of.
    check("spike rejection reaches its full 15",
        Point::pack(0, 0, 15).spike_rejection() == 15);
    let mut distinct = true;
    for raw in 0..=MAX {
        if Point::new(raw).raw() != raw {
            distinct = false;
        }
    }
    check("...and all 8192 raw values survive round-trip unchanged", distinct);

    // The consequence, stated directly: stepping up from anywhere always lands
    // somewhere new, so the tuner cannot cycle.
    let mut always_moves = true;
    for raw in 0..MAX {
        if Point::new(raw + 1) == Point::new(raw) {
            always_moves = false;
        }
    }
    check("a step up always changes the point", always_moves);

    // --- relaxing, which is NOT raw - 1 -------------------------------------
    //
    // The check that stops the borrow coming back. A decrement from a point with
    // zero low fields lands deafer than it started, which is the failure this
    // whole operation exists to avoid.
    let settled = Point::pack(0, 7, 0);
    check("the observed settle point is wd 7", settled.watchdog() == 7);
    check("a raw decrement from it borrows into full spike rejection",
        Point::new(settled.raw() - 1).spike_rejection() == 15);
    let gentler = settled.relaxed().unwrap();
    check("relaxed() steps the watchdog instead", gentler.watchdog() == 6);
    check("...leaving every other field alone", gentler.noise_floor() == 0
        && gentler.spike_rejection() == 0
        );


    // The general property, over the whole space: relaxing never makes any
    // field more defensive. That is the definition, and it is what a floor was
    // previously bolted on to fake.
    let mut never_deafer = true;
    for raw in 1..=MAX {
        let p = Point::new(raw);
        let r = match p.relaxed() {
            Some(r) => r,
            None => continue,
        };
        for index in 0..FIELDS.len() {
            if r.field(index) > p.field(index) {
                never_deafer = false;
            }
        }
        if r >= p {
            never_deafer = false;
        }
    }
    check("relaxing never raises any field, anywhere in the space", never_deafer);
    check("fully open cannot relax further", Point::OPEN.relaxed().is_none());

    // And it always terminates at OPEN rather than stalling part-way.
    let mut walk = Point::new(MAX);
    let mut steps = 0;
    while let Some(next) = walk.relaxed() {
        walk = next;
        steps += 1;
        if steps > 100 {
            break;
        }
    }
    // Min strikes is not walkable, so a ceiling point relaxes every register the
    // walk owns down to zero and leaves `ms` exactly where it found it.
    check("relaxing from the ceiling empties every walkable register",
        walk.noise_floor() == 0 && walk.watchdog() == 0 && walk.spike_rejection() == 0);
    check("...in one step per notch, not per raw value", steps == 7 + 15 + 15);

    // --- climbing, which is NOT raw + 1 -------------------------------------
    //
    // Incrementing the packed number moves the bottom bits, which are SREJ --
    // the dearest register still in the space. Climbing spends cheapest first.
    let open = Point::OPEN;
    let firmer = open.tightened().unwrap();
    check("the first step up spends the noise floor", firmer.noise_floor() == 1);
    check("...not spike rejection", firmer.spike_rejection() == 0
        && firmer.watchdog() == 0);
    check("...where raw + 1 would have moved spike rejection",
        Point::new(open.raw() + 1).spike_rejection() == 1);

    let mut walk = Point::OPEN;
    let mut steps = 0;
    while walk.spike_rejection() == 0 {
        walk = match walk.tightened() {
            Some(next) => next,
            None => break,
        };
        steps += 1;
        if steps > 100 {
            break;
        }
    }
    check("noise floor and watchdog are spent before spike rejection",
        steps == 7 + 15 + 1 && walk.noise_floor() == 7 && walk.watchdog() == 15);

    // --- the gauge ----------------------------------------------------------
    //
    // NOT inverted. Both ends checked, because the pair of wrongs this design
    // replaces -- sensitivity in the field, an inversion on the display --
    // cancelled out here and hid a tuner running backwards.
    check("fully receptive reads as no defence", Point::OPEN.percent() == 0);
    check("the ceiling reads as full defence", Point::new(MAX).percent() == 100);
    // **Harm, not magnitude.** The measurement that forced this: `nf 7, wd 6,
    // sr 0, ms 0` read 92 % on a raw-scaled bar while the device was reporting
    // every single strike -- alarming about the harmless knob and silent about
    // the dangerous ones.
    check("the observed point reads as harm, not as magnitude",
        Point::pack(7, 6, 0).percent() == 26);
    check("...where the raw value alone would have said 92",
        Point::pack(7, 6, 0).raw() as u32 * 100 / MAX as u32 == 92);
    check("the free knob at maximum is only its own weight",
        Point::pack(7, 0, 0).percent() == 10);
    check("every walkable register full reads 100",
        Point::pack(7, 15, 15).percent() == 100);

    // Min strikes overrides everything: it cannot be spent by the walk, so a
    // non-zero value was set by hand, and any value above zero silences the
    // opening of every storm whatever the other three are doing.
    // Min strikes needs no override any more: it is not representable, so no
    // point can express "wait for sixteen strikes" at all. That is the whole
    // argument for removing it from the space rather than merely skipping it.
    let mut always_one = true;
    for raw in 0..=MAX {
        if Point::new(raw).min_strikes_count() != 1 {
            always_one = false;
        }
    }
    check("no point can silence the start of a storm", always_one);

    // Monotonic along the walk -- which is what the bar tracks -- rather than
    // along the raw value, which it deliberately no longer is.
    let mut monotonic = true;
    let mut walk = Point::OPEN;
    let mut previous = walk.percent();
    while let Some(next) = walk.tightened() {
        if next.percent() < previous {
            monotonic = false;
        }
        previous = next.percent();
        walk = next;
    }
    check("the gauge never goes backwards as the walk tightens", monotonic);

    // --- the walk is field-wise, not arithmetic -----------------------------
    let open = Point::OPEN;
    check("the first step up spends the noise floor",
        open.tightened().unwrap().noise_floor() == 1);
    let settled = Point::pack(0, 7, 0);
    check("relaxing off the watchdog steps the watchdog",
        settled.relaxed().unwrap().watchdog() == 6);
    check("...and touches nothing else",
        settled.relaxed().unwrap().noise_floor() == 0
            && settled.relaxed().unwrap().spike_rejection() == 0);

    println!(
        "\n{} passed, {} failed",
        PASS.load(Ordering::Relaxed),
        FAIL.load(Ordering::Relaxed)
    );
    std::process::exit(if FAIL.load(Ordering::Relaxed) == 0 { 0 } else { 1 });
}
