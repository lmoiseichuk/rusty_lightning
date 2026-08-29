//! Host checks for §4.2's packed defence point.
//!
//! Worth testing because the bit layout *is* the design — and because the
//! design's central claim is now a negative one: **no value the tuner can reach
//! is allowed to cost a strike.** That is not enforced by any code path. It is a
//! consequence of `FIELDS` containing exactly one register, and a table with an
//! extra row added back would still compile, still run, and quietly start
//! trading distant strikes for a quieter room again.
//!
//! **This compiles the real `src/defence.rs`**, included by path — not a copy of
//! it. Keeping that module free of ESP-IDF imports is what makes this possible,
//! and it means a change to the layout that breaks these checks fails here
//! rather than silently drifting away from what ships.
//!
//! It drifted anyway, once, because nothing runs this file automatically:
//! `SPIKE_REJECTION_DEFAULT` went from 1 to 0 in `src/` on 2026-08-19 and the
//! assertion here still said 1 for four days. Run it after touching `defence`:
//!
//! ```sh
//! cd tests/host && rustc --edition 2024 -A dead_code -o /tmp/defence defence.rs && /tmp/defence
//! ```

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
    // Still worth checking with a single row, and arguably worth more: this is
    // what catches a second field being added back without `BITS` following it,
    // which would silently hand the tuner a register it must not have.
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
    check("3 bits means 0..=7", BITS == 3 && MAX == 7);

    // --- the point is the free knob, and only the free knob -----------------
    //
    // **The claim the whole module now rests on.** `NF_LEV` decides when the
    // chip complains the band is noisy; it cannot reject a lightning waveform.
    // Every other knob is paid for in strikes, so none of them is here.
    check("NF_LEV is 3 bits at 0", FIELDS[NOISE_FLOOR].shift == 0
        && FIELDS[NOISE_FLOOR].width == 3);
    check("the free knob is the only field", FIELDS.len() == 1);

    // The three that left, each now a constant or a setting. Getting any of
    // these wrong reintroduces the failure that evicted it.
    check("min strikes is pinned to report every strike", MIN_STRIKES_COUNT == 1);
    // Left because the tuner refunds the least valuable field first, so every
    // quiet spell walked it to zero -- and at zero the chip reported an electric
    // hammer as lightning, 503 times in three and a half hours.
    check("spike rejection defaults to the reference driver's 0",
        SPIKE_REJECTION_DEFAULT == 0);
    check("...and is settable across the register's range",
        SPIKE_REJECTION_MAX == 15);
    // Left because it gates on amplitude: raising it discards weaker arrivals,
    // distant strikes first, which are the ones an early-warning device exists
    // to report. 2 is `deep_demo.py`'s value, the record of what worked here.
    check("the watchdog defaults to the reference setup's 2", WATCHDOG_DEFAULT == 2);
    check("...and is settable across the register's range", WATCHDOG_MAX == 15);

    // --- what a bisection actually probes -----------------------------------
    //
    // Three probes rather than seven. That is the practical dividend: a sweep
    // is short enough that running one costs nothing worth defending against,
    // which is what makes perturbing a stuck point affordable.
    let target = 5u16;
    let mut low = 0u16;
    let mut high = MAX;
    let mut probes = 0;
    while low < high {
        let mid = low + (high - low) / 2;
        probes += 1;
        if mid >= target {
            high = mid;
        } else {
            low = mid + 1;
        }
    }
    check("a full bisection is 3 probes", probes == 3);
    check("...landing exactly on the answer", low == target);

    // --- packing round-trips ------------------------------------------------
    let p = Point::pack(5);
    check("noise floor round-trips", p.noise_floor() == 5);
    check("and the raw value is the field itself", p.raw() == 5);
    check("a packed point survives new()", Point::new(p.raw()) == p);

    check("OPEN is the field at zero", Point::OPEN.raw() == 0
        && Point::OPEN.noise_floor() == 0);

    // --- the compiled-in starting point -------------------------------------
    //
    // Mid-range rather than open, and the reason is measured twice: booting
    // fully open gave 7-9 noise events per batch continuously, and `nf 0` pinned
    // at indoor gain gave 5-8 per batch through an entire live storm with not
    // one strike reported.
    let start = Point::default_start();
    check("the default starts the noise floor mid-range", start.noise_floor() == 3);
    check("...and reporting every strike", start.min_strikes_count() == 1);
    check("...which is well clear of the noisy bottom", start.raw() > 0);

    // --- clamping -----------------------------------------------------------
    check("a raw value past MAX clamps", Point::new(60000).raw() <= MAX);
    check("the chip always reports every strike",
        Point::new(60000).min_strikes_count() == 1 && Point::OPEN.min_strikes_count() == 1);
    check("a field value past its width clamps", Point::pack(200).noise_floor() == 7);
    check("the noise floor reaches its full 7", Point::pack(7).noise_floor() == 7);

    // **Every raw value is a distinct point.** No field is capped below its
    // width, and this is the check that keeps it that way: a cap would fold
    // several raw values onto one point, which strands the +/-1 tuner in a short
    // cycle it can never climb out of.
    let mut distinct = true;
    for raw in 0..=MAX {
        if Point::new(raw).raw() != raw {
            distinct = false;
        }
    }
    check("...and all 8 raw values survive round-trip unchanged", distinct);

    // The consequence, stated directly: stepping up from anywhere always lands
    // somewhere new, so the tuner cannot cycle.
    let mut always_moves = true;
    for raw in 0..MAX {
        if Point::new(raw + 1) == Point::new(raw) {
            always_moves = false;
        }
    }
    check("a step up always changes the point", always_moves);

    // --- relaxing and climbing ----------------------------------------------
    //
    // With one field these are arithmetic again, which they emphatically were
    // not before: the borrow that made `raw - 1` land *deafer* than it started
    // needed two fields to exist at all. Kept as checks because the operations
    // are still field-wise, so re-adding a field must not quietly resurrect it.
    let firmer = Point::OPEN.tightened().unwrap();
    check("the first step up spends the noise floor", firmer.noise_floor() == 1);
    check("fully open cannot relax further", Point::OPEN.relaxed().is_none());
    check("the ceiling cannot tighten further", Point::new(MAX).tightened().is_none());

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
    check("relaxing from the ceiling empties the register", walk.noise_floor() == 0);
    check("...in one step per notch", steps == 7);

    // --- the gauge ----------------------------------------------------------
    //
    // NOT inverted. Both ends checked, because the pair of wrongs this design
    // replaces -- sensitivity in the field, an inversion on the display --
    // cancelled out here and hid a tuner running backwards.
    check("fully receptive reads as no defence", Point::OPEN.percent() == 0);
    check("the ceiling reads as full defence", Point::new(MAX).percent() == 100);
    // The weight is now the whole gauge, because the field is the whole point.
    // It reads as magnitude again only because harm and magnitude finally agree:
    // there is nothing left in the space that is more harmful than anything else.
    check("the sole field carries the whole gauge",
        FIELDS[NOISE_FLOOR].weight == 100);
    check("mid-range reads as mid-range", Point::pack(3).percent() == 42);

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

    // Monotonic along the walk -- which is what the bar tracks.
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

    println!(
        "\n{} passed, {} failed",
        PASS.load(Ordering::Relaxed),
        FAIL.load(Ordering::Relaxed)
    );
    std::process::exit(if FAIL.load(Ordering::Relaxed) == 0 { 0 } else { 1 });
}
