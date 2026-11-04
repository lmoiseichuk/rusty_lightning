//! lightning project, Copyright (c) 2026 Leonid Moiseichuk under MIT License
//!
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
//! cd tests && rustc --edition 2024 -A dead_code -o /tmp/defence defence.rs && /tmp/defence
//! ```

use std::sync::atomic::{AtomicU32, Ordering};

#[path = "../src/defence.rs"]
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
    // **This check used to assert 0, and asserting it is what kept it.** The
    // default was set to 1 deliberately, reverted to 0 by an unrelated commit,
    // and this line -- written when 0 was current -- turned the accident into a
    // rule the suite enforced. A test that pins a value without pinning the
    // reasoning will defend a regression as loyally as it defends a decision.
    //
    // The reasoning, since 0.22.0: SREJ is the only knob that rejects short
    // man-made impulses; at zero the chip validated an electric hammer as
    // lightning 503 times in three and a half hours, and at one it let
    // September's 9,542 `overhead` through round the clock. The resting value
    // is the chip's own. **Pinned to that reasoning, not to a number**: a
    // default that drifts away from the power-on value fails here whatever the
    // number it drifts to.
    check("spike rejection defaults to the chip's power-on value",
        SPIKE_REJECTION_DEFAULT == SPIKE_REJECTION_POWER_ON);
    check("...which the datasheet gives as 2",
        SPIKE_REJECTION_POWER_ON == 2);
    check("...and is settable across the register's range",
        SPIKE_REJECTION_MAX == 15);
    // Left because it gates on amplitude: raising it discards weaker arrivals,
    // distant strikes first, which are the ones an early-warning device exists
    // to report.
    //
    // **This asserted 2 for a week after the default became 0**, and nothing ran
    // it: the commit that opened the gate changed `src/defence.rs` alone, and
    // the next `check.sh` refused to build. The same shape as the spike
    // rejection line above, from the other side -- here the value moved on
    // purpose and the check stayed behind. The reasoning is at
    // `WATCHDOG_DEFAULT`: measured in a storm, `wd 2` heard 0 strikes in 100 s
    // and `wd 0` heard 2 in 90 s, because the watchdog discards a return before
    // anything can classify it. The 2 was half of `deep_demo.py`'s pair, kept
    // without the `srej 0` it was measured beside.
    check("the watchdog defaults to 0 -- the amplitude gate open", WATCHDOG_DEFAULT == 0);
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
    println!("\n  the device may not choose zero spike rejection");
    // A person may set 0 -- it is a legitimate experiment. The device choosing
    // it for itself is how 503 false strikes got logged in three and a half
    // hours, so every automatic path goes through this.
    check(
        "nothing stored: the device picks the floor, not zero",
        spike_rejection_for_autorun(None) >= SPIKE_REJECTION_AUTO_MIN,
    );
    check(
        "and that is the documented default",
        spike_rejection_for_autorun(None) == SPIKE_REJECTION_DEFAULT,
    );
    check(
        "a deliberate zero is honoured, because a person set it",
        spike_rejection_for_autorun(Some(0)) == 0,
    );
    check(
        "a deliberate value is passed through unchanged",
        spike_rejection_for_autorun(Some(8)) == 8,
    );
    check(
        "the default is not none, which is the whole decision",
        SPIKE_REJECTION_DEFAULT >= 1,
    );

    println!("\n  what boot programs, from NVS and a trusted record");
    check("nothing stored, no record: the default", spike_rejection_at_boot(None, None) == 2);
    // **The board this was written on**: `srej 1` saved by hand, and its
    // golden record from before 0.22.0 set aside, so no record reaches here.
    check("a person's 1 is left alone", spike_rejection_at_boot(Some(1), None) == 1);
    check("...and announced as below the default", spike_rejection_below_default(Some(1)));
    // ...and once its owner moves on, nothing drags it back.
    check("their later `srej 2` stands with no record to lower it", spike_rejection_at_boot(Some(2), None) == 2);
    check("a deafer value a person chose stands too", spike_rejection_at_boot(Some(4), None) == 4);
    check("a person's 0 is honoured", spike_rejection_at_boot(Some(0), None) == 0);
    check("...and is warned about elsewhere, not here", !spike_rejection_below_default(Some(0)));
    check("the default itself is not below the default", !spike_rejection_below_default(Some(2)));
    check("nor is a deafer value", !spike_rejection_below_default(Some(4)));
    check("nor is nothing stored", !spike_rejection_below_default(None));
    // The more permissive of stored and heard, as boot has always done.
    check("a record heard at 1 lowers a stored 2", spike_rejection_at_boot(Some(2), Some(1)) == 1);
    check("a record heard at 2 leaves a person's 1", spike_rejection_at_boot(Some(1), Some(2)) == 1);
    check("a record fills in when nothing is stored", spike_rejection_at_boot(None, Some(3)) == 3);
    // **The guard boot's comment claimed and nothing enforced.** A record made
    // during a person's `srej 0` must not lower a later choice back to zero.
    check("a record at 0 cannot take a stored 2 below 1", spike_rejection_at_boot(Some(2), Some(0)) == 1);
    check("...nor fill an unset value with 0", spike_rejection_at_boot(None, Some(0)) == 1);
    check("a person's own 0 still beats any record", spike_rejection_at_boot(Some(0), Some(3)) == 0);


    println!(
        "\n{} passed, {} failed",
        PASS.load(Ordering::Relaxed),
        FAIL.load(Ordering::Relaxed)
    );
    std::process::exit(if FAIL.load(Ordering::Relaxed) == 0 { 0 } else { 1 });
}
