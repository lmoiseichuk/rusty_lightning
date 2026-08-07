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
    check("13 bits means 0..=8191", BITS == 13 && MAX == 8191);

    // --- the documented positions ------------------------------------------
    check("NF_LEV is 3 bits at 10", FIELDS[NOISE_FLOOR].shift == 10
        && FIELDS[NOISE_FLOOR].width == 3);
    check("WDTH is 4 bits at 6", FIELDS[WATCHDOG].shift == 6
        && FIELDS[WATCHDOG].width == 4);
    check("SREJ is 4 bits at 2", FIELDS[SPIKE].shift == 2
        && FIELDS[SPIKE].width == 4);
    check("MIN_NUM_LIGH is 2 bits at 0", FIELDS[MIN_STRIKES].shift == 0
        && FIELDS[MIN_STRIKES].width == 2);

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
    check("the destructive knob (MIN_NUM_LIGH) holds the lowest",
        least_significant == MIN_STRIKES);

    // --- what a bisection actually probes -----------------------------------
    //
    // The ordering claim stated as behaviour rather than as bit positions.
    let first = Point::new((MAX + 1) / 2);
    check("the first probe is a noise-floor decision", first.noise_floor() == 4
        && first.watchdog() == 0
        && first.spike_rejection() == 0
        && first.min_strikes() == 0);

    // Replay a full search for a known answer, watching when each field settles.
    let target = 1500u16;
    let mut low = 0u16;
    let mut high = MAX;
    let mut probes = 0;
    // "Settled at probe N" means every probe from N onwards already had this
    // field at its final value -- so a LATER number means the search was still
    // making up its mind about that register.
    let mut floor_settled_at = 0;
    let mut strikes_settled_at = 0;
    let goal = Point::new(target);
    while low < high {
        let mid = low + (high - low) / 2;
        probes += 1;
        if Point::new(mid).noise_floor() != goal.noise_floor() {
            floor_settled_at = probes + 1;
        }
        if Point::new(mid).min_strikes() != goal.min_strikes() {
            strikes_settled_at = probes + 1;
        }
        if mid >= target {
            high = mid;
        } else {
            low = mid + 1;
        }
    }
    check("a full bisection is 13 probes", probes == 13);
    check("...landing exactly on the answer", low == target);
    check("...with the noise floor decided early", floor_settled_at <= 4);
    check("...and min strikes still moving long after it",
        strikes_settled_at > floor_settled_at);

    // --- packing round-trips ------------------------------------------------
    let p = Point::pack(5, 9, 7, 2);
    check("noise floor round-trips", p.noise_floor() == 5);
    check("watchdog round-trips", p.watchdog() == 9);
    check("spike rejection round-trips", p.spike_rejection() == 7);
    check("min strikes round-trips", p.min_strikes() == 2);
    check("and the raw value is the four fields packed",
        p.raw() == (5 << 10) | (9 << 6) | (7 << 2) | 2);
    check("a packed point survives new()", Point::new(p.raw()) == p);

    check("OPEN is every field at zero", Point::OPEN.raw() == 0
        && Point::OPEN.noise_floor() == 0
        && Point::OPEN.min_strikes() == 0);

    // --- clamping -----------------------------------------------------------
    check("a raw value past MAX clamps", Point::new(60000).raw() <= MAX);
    check("a field value past its width clamps",
        Point::pack(200, 200, 200, 200).noise_floor() == 7);

    // **Every raw value is a distinct point.** No field is capped below its
    // width, and this is the check that keeps it that way: a cap would fold
    // several raw values onto one point, which strands the +/-1 tuner in a short
    // cycle it can never climb out of.
    check("spike rejection reaches its full 15",
        Point::pack(0, 0, 15, 0).spike_rejection() == 15);
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

    // --- the strike count is not the field ----------------------------------
    check("min strikes 0 waits for 1", Point::pack(0, 0, 0, 0).min_strikes_count() == 1);
    check("min strikes 1 waits for 5", Point::pack(0, 0, 0, 1).min_strikes_count() == 5);
    check("min strikes 2 waits for 9", Point::pack(0, 0, 0, 2).min_strikes_count() == 9);
    check("min strikes 3 waits for 16", Point::pack(0, 0, 0, 3).min_strikes_count() == 16);

    // --- the gauge ----------------------------------------------------------
    //
    // NOT inverted. Both ends checked, because the pair of wrongs this design
    // replaces -- sensitivity in the field, an inversion on the display --
    // cancelled out here and hid a tuner running backwards.
    check("fully receptive reads as no defence", Point::OPEN.percent() == 0);
    check("the ceiling reads as full defence", Point::new(MAX).percent() == 100);
    check("the midpoint reads as about half", {
        let half = Point::new(MAX / 2).percent();
        (48..=52).contains(&half)
    });
    let mut monotonic = true;
    let mut previous = 0;
    for raw in 0..=MAX {
        let percent = Point::new(raw).percent();
        if percent < previous {
            monotonic = false;
        }
        previous = percent;
    }
    check("the gauge never goes backwards as the number climbs", monotonic);

    // --- walking the raw number, which is what the tuner does ---------------
    //
    // Documents the chosen trade rather than approving of it: with the
    // destructive register in the bottom bits, the FIRST noisy step moves it.
    let one_step = Point::new(Point::OPEN.raw() + 1);
    check("a single step up moves min strikes, not the floor",
        one_step.min_strikes() == 1 && one_step.noise_floor() == 0);
    check("...which means the chip waits for five strikes",
        one_step.min_strikes_count() == 5);

    // The carry that makes the space imperfectly monotonic in *defence*, kept as
    // a check so the behaviour is written down rather than rediscovered.
    let below = Point::new(1023);
    let above = Point::new(1024);
    check("a carry raises the floor by one",
        above.noise_floor() == below.noise_floor() + 1);
    check("...while dropping every cheaper field to zero",
        above.watchdog() == 0 && above.spike_rejection() == 0 && above.min_strikes() == 0);
    check("...from having been at their maximums",
        below.watchdog() == 15 && below.spike_rejection() == 15
            && below.min_strikes() == 3);

    println!(
        "\n{} passed, {} failed",
        PASS.load(Ordering::Relaxed),
        FAIL.load(Ordering::Relaxed)
    );
    std::process::exit(if FAIL.load(Ordering::Relaxed) == 0 { 0 } else { 1 });
}
