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

#[path = "../src/golden.rs"]
mod golden;
use golden::*;

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

    println!("\n  which of two is more open");
    let base = combo(3, 2, 0, false);
    check("equal counts as at least as open", base.at_least_as_open_as(base));
    check(
        "a lower noise floor is more open",
        combo(1, 2, 0, false).at_least_as_open_as(base),
    );
    check(
        "a higher noise floor is not",
        !combo(5, 2, 0, false).at_least_as_open_as(base),
    );
    check(
        "a higher watchdog is not",
        !combo(3, 4, 0, false).at_least_as_open_as(base),
    );
    // **The knobs do not trade.** A strike the watchdog discarded is not
    // recovered by a lower noise floor, so a gain in one must not pay for a
    // loss in another.
    check(
        "a lower floor does not pay for a higher watchdog",
        !combo(0, 4, 0, false).at_least_as_open_as(base),
    );
    check(
        "across a gain change the answer is no",
        !combo(0, 0, 0, true).at_least_as_open_as(base),
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
        fall_back_to(deaf, trusted(good), PATIENCE_MINUTES) == Some(good),
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
        fall_back_to(combo(6, 2, 0, false), trusted(combo(1, 2, 0, false)), 30)
            == Some(combo(1, 2, 0, false)),
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

    println!("\n  what counts as drowning");
    // Ordinary noise is the walk's business; only drowning is a fact about the
    // room worth remembering, or the floor ratchets up until the device is deaf.
    check("the measured swamp qualifies", is_swamped(595, 60));
    check("a merely noisy window does not", !is_swamped(120, 60));
    check("nor does exactly twice the threshold", !is_swamped(120, 60));
    check("five times does", is_swamped(300, 60));
    check("a zero threshold does not divide by zero", is_swamped(5, 0));

    let passed = PASS.load(Ordering::Relaxed);
    let failed = FAIL.load(Ordering::Relaxed);
    println!("\n{passed} passed, {failed} failed");
    if failed > 0 {
        std::process::exit(1);
    }
}
