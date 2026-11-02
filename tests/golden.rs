//! Host checks for the known-good settings record.
//!
//! **This compiles the real `src/golden.rs`**, included by path.
//!
//! Worth testing because both ways of being wrong are expensive and neither
//! announces itself. Fall back too eagerly and the tuner cannot defend against
//! a genuinely noisy room — it is dragged back to a setting that was right in
//! different weather and sits there reporting disturbers. Fall back too
//! reluctantly, or not at all, and the device sits deaf through a storm, which
//! is the failure this record exists to end and the one that looks exactly like
//! a quiet night.
//!
//! ```sh
//! cd tests && rustc --edition 2024 -A dead_code -o /tmp/golden golden.rs && /tmp/golden
//! ```

use std::sync::atomic::{AtomicU32, Ordering};

// `golden` names `crate::strike` for the distance a detection carried, and the
// mixed-flash checks below run strokes through the real merger, which reaches
// for `crate::uptime`. All three at the root, where those paths resolve.
#[path = "../src/strike.rs"]
mod strike;
#[path = "../src/uptime.rs"]
mod uptime;
#[path = "../src/merger.rs"]
mod merger;
#[path = "../src/golden.rs"]
mod golden;
use golden::*;
use merger::Merger;
use strike::{Distance, Strike};

static PASS: AtomicU32 = AtomicU32::new(0);
static FAIL: AtomicU32 = AtomicU32::new(0);

fn check(name: &str, ok: bool) {
    let counter = if ok { &PASS } else { &FAIL };
    counter.fetch_add(1, Ordering::Relaxed);
    println!("  {:<4} {name}", if ok { "ok" } else { "FAIL" });
}

fn combo(nf: u8, wdth: u8, srej: u8, outdoor: bool) -> Combo {
    Combo { nf, wdth, srej, outdoor }
}

/// A record trusted enough for the fall-back rule to act on.
fn trusted(c: Combo) -> Option<Golden> {
    Some(Golden { combo: c, strikes: TRUSTED_STRIKES })
}

fn main() {
    println!("== golden ==");

    println!("\n  packing survives a round trip");
    for (name, c) in [
        ("all zero", combo(0, 0, 0, false)),
        ("all at maximum", combo(7, 15, 15, true)),
        ("a real one", combo(2, 2, 0, false)),
        ("outdoor", combo(5, 3, 1, true)),
    ] {
        check(name, Combo::unpack(c.pack()) == c);
    }
    // The fields must not bleed into each other -- a packing bug here would
    // silently rewrite the watchdog when the noise floor moved.
    check(
        "nf does not disturb the others",
        Combo::unpack(combo(7, 0, 0, false).pack()) == combo(7, 0, 0, false),
    );
    check(
        "srej does not disturb the gain",
        Combo::unpack(combo(0, 0, 15, false).pack()) == combo(0, 0, 15, false),
    );
    check(
        "gain does not disturb srej",
        Combo::unpack(combo(0, 0, 0, true).pack()) == combo(0, 0, 0, true),
    );

    println!("\n  the fall-back rule");
    let good = combo(1, 2, 0, false);
    let deaf = combo(6, 2, 0, false);

    check(
        "no record, nothing to say",
        fall_back_to(deaf, None, 60).is_none(),
    );
    check(
        "one strike is not yet trusted",
        fall_back_to(deaf, Some(Golden { combo: good, strikes: 1 }), 60).is_none(),
    );
    check(
        "trusted, deaf and quiet a long time -- go back",
        fall_back_to(deaf, trusted(good), PATIENCE_MINUTES) == Some(good.nf),
    );
    check(
        "...but not before the patience runs out",
        fall_back_to(deaf, trusted(good), PATIENCE_MINUTES - 1).is_none(),
    );
    check(
        "already as open -- nothing to go back to",
        fall_back_to(good, trusted(good), 600).is_none(),
    );
    // The rule must never *reduce* sensitivity: a tuner that has found
    // something more open than the record keeps it.
    check(
        "more open than the record -- left alone",
        fall_back_to(combo(0, 0, 0, false), trusted(good), 600).is_none(),
    );
    check(
        "a record from the other gain does not apply",
        fall_back_to(deaf, trusted(combo(1, 2, 0, true)), 600).is_none(),
    );

    // **The noise floor is all the rescue moves, so it is all it compares.**
    // Comparing the other two made a rescue that no move of `nf` could finish:
    // with the chip at sr 2 against a record at sr 1 -- `srej 2` typed after a
    // trusted record -- it fired every window and froze the walk.
    let record = combo(3, 0, 1, false);
    check(
        "at the record's nf with a deafer srej, there is nothing to do",
        fall_back_to(combo(3, 0, 2, false), trusted(record), PATIENCE_MINUTES + 5).is_none(),
    );
    check(
        "...nor with a stored watchdog above the record's",
        fall_back_to(combo(3, 2, 1, false), trusted(record), 600).is_none(),
    );
    // And the other way it went wrong: a noise floor more open than the
    // record's, with a higher srej, was "returned" up to the record's rung.
    check(
        "a noise floor more open than the record's is never pulled up to it",
        fall_back_to(combo(1, 0, 2, false), trusted(record), 600).is_none(),
    );
    check(
        "deafer in nf and srej both: the noise floor comes back, and only it",
        fall_back_to(combo(5, 0, 2, false), trusted(record), PATIENCE_MINUTES) == Some(3),
    );

    println!("\n  building the record");
    let first = observe(None, good);
    check("the first strike starts the count", first == Golden { combo: good, strikes: 1 });
    let second = observe(Some(first), good);
    check("the same combination counts up", second.strikes == 2);
    check("...and is then trusted", second.strikes >= TRUSTED_STRIKES);
    let moved = observe(Some(second), deaf);
    check(
        "a different combination replaces it and starts again",
        moved == Golden { combo: deaf, strikes: 1 },
    );
    // Newest evidence wins rather than most-counted: a combination that worked
    // last month in different weather is not a better answer than one that
    // worked a minute ago.
    check(
        "a long-standing record does not outvote fresh evidence",
        observe(Some(Golden { combo: good, strikes: 500 }), deaf).strikes == 1,
    );

    println!("\n  the storm this was written for");
    // Deaf at nf 6, a trusted record at nf 1, nothing heard for half an hour:
    // the sky is not the likeliest explanation.
    check(
        "a deaf tuner in a storm is pulled back",
        fall_back_to(combo(6, 2, 0, false), trusted(combo(1, 2, 0, false)), 30) == Some(1),
    );
    // And the reverse: an honestly quiet night at the known-good setting is
    // left entirely alone, however long it lasts.
    check(
        "a quiet night at the known-good setting is not disturbed",
        fall_back_to(combo(1, 2, 0, false), trusted(combo(1, 2, 0, false)), 6000).is_none(),
    );

    println!("\n  the floor: rungs measured to drown");
    let none = Floor::default();
    check("nothing learned means no restriction", none.lowest() == 0);
    check("...and relaxing is allowed from anywhere", none.may_relax_from(1));

    // The observed case: a sweep measured nf 0 at 595/min against a 60/min
    // threshold, settled at nf 1, and the walk relaxed into nf 0 anyway.
    let learned = none.swamped(0);
    check("swamped at 0 puts the floor at 1", learned.lowest() == 1);
    check("so nf 1 may not relax further", !learned.may_relax_from(1));
    check("but nf 2 still may", learned.may_relax_from(2));

    check("only the highest swamped rung is kept", none.swamped(0).swamped(2).lowest() == 3);
    check("...in either order", none.swamped(2).swamped(0).lowest() == 3);
    check(
        "the floor cannot exceed the ladder",
        none.swamped(MAX_NF).lowest() == MAX_NF,
    );
    check("forgetting clears it", Floor::forget().lowest() == 0);

    // -- a rung that heard lightning is a floor too -------------------------
    check(
        "a heard rung floors the walk there",
        Floor::default().heard(1).lowest() == 1,
    );
    check(
        "and relaxing from one above it is allowed",
        Floor::default().heard(1).may_relax_from(2),
    );
    check(
        "but relaxing off it is not",
        !Floor::default().heard(1).may_relax_from(1),
    );
    check(
        "the most open rung that worked is the one kept",
        Floor::default().heard(3).heard(1).lowest() == 1,
    );
    check(
        "a later strike at a deafer rung does not ratchet it up",
        Floor::default().heard(1).heard(4).lowest() == 1,
    );
    check(
        "drowning and hearing take the stricter of the two",
        Floor::default().swamped(2).heard(1).lowest() == 3,
    );
    check(
        "and hearing wins when it is the stricter",
        Floor::default().swamped(0).heard(3).lowest() == 3,
    );
    check(
        "forgetting clears the heard rung as well",
        Floor::forget().heard_at.is_none(),
    );

    println!("\n  what counts as drowning");
    // Ordinary noise is the walk's business; only drowning is a fact about the
    // room worth remembering, or the floor ratchets up until the device is deaf.
    check("the measured swamp qualifies", is_swamped(595, 60));
    check("a merely noisy window does not", !is_swamped(120, 60));
    check("nor does exactly twice the threshold", !is_swamped(120, 60));
    check("five times does", is_swamped(300, 60));
    check("a zero threshold does not divide by zero", is_swamped(5, 0));
    // **The regression this test exists for.** The working rung on this device
    // measures 0 noise and 5-8 disturbers a second. A swamp test fed the sum
    // would grade it as drowning and ratchet the floor up under it, one rung a
    // window, until the device was deaf -- so the argument is noise alone.
    check("the working rung is not drowning", !is_swamped(0, 60));
    check("...however many disturbers accompany it", !is_swamped(0, 60));

    println!("\n  what a detection may teach");
    // **The regression this section exists for.** `overhead` has been man-made
    // -- the hammer at srej 0, September's 9,542 round the clock, November's on
    // a USB plug-in -- as well as lightning, and nothing in the reading says
    // which; man-made, it fed the record.
    check("overhead is not evidence", !is_evidence(Distance::Overhead));
    check("a kilometre reading is", is_evidence(Distance::Km(6)));
    // The nearest kilometre bin is 5, one above the sentinel: the line is drawn
    // at the sentinel, not at "close".
    check("so is the nearest kilometre bin", is_evidence(Distance::Km(5)));
    check("out of range is an answer about distance, so it is", is_evidence(Distance::OutOfRange));
    check("a real kilometre flash may be recorded", may_record(false, Distance::Km(10)));
    check("a real overhead flash may not", !may_record(false, Distance::Overhead));
    check("a simulated flash never may", !may_record(true, Distance::Km(10)));
    check("...overhead or not", !may_record(true, Distance::Overhead));

    // Strokes through the real merger, at its default one-second window: the
    // flash is judged on the distance it is recorded with.
    let stroke = |distance| Strike { distance, energy_raw: 100_000 };
    let flash = |strokes: &[Distance]| {
        let mut merging = Merger::default();
        for (index, distance) in strokes.iter().enumerate() {
            let _ = merging.observe(&stroke(*distance), Some(1), 1, false, index as u32 * 100);
        }
        merging.take_due(10_000).expect("the window closed")
    };
    let mixed = flash(&[Distance::Overhead, Distance::Km(8)]);
    check(
        "overhead then kilometres in one flash is recorded at kilometres",
        mixed.strike.distance == Distance::Km(8) && mixed.strokes == 2,
    );
    check("...and counts, as its kilometre stroke would alone", may_record(mixed.simulated, mixed.strike.distance));
    let reversed = flash(&[Distance::Km(8), Distance::Overhead]);
    check("in either order", may_record(reversed.simulated, reversed.strike.distance));
    let contradictory = flash(&[Distance::Overhead, Distance::OutOfRange]);
    check(
        "overhead with far is recorded as overhead",
        contradictory.strike.distance == Distance::Overhead,
    );
    check(
        "...and does not count, so the log's distance column says what taught the device",
        !may_record(contradictory.simulated, contradictory.strike.distance),
    );
    let only = flash(&[Distance::Overhead, Distance::Overhead, Distance::Overhead]);
    check("three overhead strokes are one flash that teaches nothing", !may_record(only.simulated, only.strike.distance));

    // The record as `session::remember_working_point` builds it: each strike a
    // flash out of the real merger, through the real `fold_flash`, and the
    // record kept as it was when the flash teaches nothing -- which is all the
    // caller adds. Until 0.22.1 this was a closure that re-implemented the
    // gate, so it tested a copy of the rule rather than the one the device
    // runs, and reverting the device's passed it.
    let fold = |record: Option<Golden>, distance: Distance, at: Combo| {
        match fold_flash(record, &flash(&[distance]), at) {
            Some(after) => Some(after),
            None => record,
        }
    };
    check(
        "fold_flash learns from a located flash",
        fold_flash(None, &flash(&[Distance::Km(9)]), combo(3, 0, 1, false))
            == Some(Golden { combo: combo(3, 0, 1, false), strikes: 1 }),
    );
    check(
        "...and says it learned nothing from an overhead one",
        fold_flash(None, &flash(&[Distance::Overhead]), combo(3, 0, 1, false)).is_none(),
    );
    let mut simulated = flash(&[Distance::Km(9)]);
    simulated.simulated = true;
    check(
        "...or from a simulated one",
        fold_flash(trusted(combo(3, 0, 1, false)), &simulated, combo(3, 0, 1, false)).is_none(),
    );
    let room = combo(3, 0, 1, false);
    let mut september = None;
    for _ in 0..840 {
        september = fold(september, Distance::Overhead, room);
    }
    check("840 overhead strikes write no record at all", september.is_none());
    check("...so nothing can fall back to one", fall_back_to(combo(7, 0, 1, false), september, 600).is_none());

    // 2026-11-01 outside that morning, in the order the log has them: seven
    // with a distance, six overhead among them.
    let november = [
        Distance::Km(14), Distance::Km(6), Distance::Km(6), Distance::Overhead,
        Distance::Overhead, Distance::Overhead, Distance::Km(10), Distance::Km(6),
        Distance::Overhead, Distance::Overhead, Distance::Overhead, Distance::Km(12),
        Distance::Km(12),
    ];
    let mut record = None;
    for distance in november {
        record = fold(record, distance, room);
    }
    check(
        "that morning's thirteen make a record of seven",
        record == Some(Golden { combo: room, strikes: 7 }),
    );
    check("...which is still trusted", record.map(|r| r.strikes).unwrap_or(0) >= TRUSTED_STRIKES);
    // A stream of overhead must not be able to buy trust on its own either:
    // one strike with a distance, then nothing but overhead.
    let mut thin = fold(None, Distance::Km(9), room);
    for _ in 0..50 {
        thin = fold(thin, Distance::Overhead, room);
    }
    check("one located strike and fifty overhead is still one strike", thin.map(|r| r.strikes) == Some(1));
    check("...and not trusted", fall_back_to(combo(7, 0, 1, false), thin, 600).is_none());

    let passed = PASS.load(Ordering::Relaxed);
    let failed = FAIL.load(Ordering::Relaxed);
    println!("\n{passed} passed, {failed} failed");
    if failed > 0 {
        std::process::exit(1);
    }
}
