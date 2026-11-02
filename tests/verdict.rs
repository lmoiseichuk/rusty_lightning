//! Host checks for the tuning window's verdict.
//!
//! **This compiles the real `src/verdict.rs`**, included by path — not a copy.
//! Keeping that module free of ESP-IDF imports is what makes this possible, and
//! it is why the module exists at all: `tuning` drives an I²C sensor and cannot
//! be built on a workstation, so the one decision worth checking was unreachable
//! until it was moved out.
//!
//! The claim being defended is a negative one, which is exactly the kind that
//! rots quietly: **a disturber must never reach the quiet verdict.** Nothing in
//! the type system says so. Folding `noise + disturbers` compiles, runs, and
//! looks reasonable — it was what shipped, and it made the tuner climb `NF_LEV`
//! against events that knob cannot touch.
//!
//! ```sh
//! cd tests && rustc --edition 2024 -A dead_code -o /tmp/verdict verdict.rs && /tmp/verdict
//! ```

use std::sync::atomic::{AtomicU32, Ordering};

// `verdict` splits a lightning interrupt by its distance with
// `golden::is_evidence`, so both of those come in at the root, where its
// `crate::golden` and `crate::strike` resolve.
#[path = "../src/strike.rs"]
mod strike;
#[path = "../src/golden.rs"]
mod golden;
#[path = "../src/verdict.rs"]
mod verdict;
use strike::Distance;
use verdict::*;

static PASS: AtomicU32 = AtomicU32::new(0);
static FAIL: AtomicU32 = AtomicU32::new(0);

fn check(name: &str, ok: bool) {
    let counter = if ok { &PASS } else { &FAIL };
    counter.fetch_add(1, Ordering::Relaxed);
    println!("  {:<4} {name}", if ok { "ok" } else { "FAIL" });
}

fn main() {
    println!("== verdict ==");

    // **The regression this file exists for.** Storm of 2026-08-26: a window of
    // pure disturbers read as a noisy band, and the tuner answered with the one
    // knob that cannot remove them.
    let mut storm = Window::default();
    storm.fold(0, 8, 0);
    check(
        "a window of disturbers alone is quiet",
        storm.quiet(1, 60),
    );
    check(
        "and its disturbers are still counted",
        storm.disturbers_per_min(60) == 8,
    );
    check(
        "and contribute nothing to the noise rate",
        storm.noise_per_min(60) == 0,
    );

    let mut noisy = Window::default();
    noisy.fold(11, 0, 0);
    check("noise alone still says noisy", !noisy.quiet(1, 60));

    // A rate, not a count: the same evidence in half the window is twice the
    // rate, and the verdict has to follow.
    let mut half = Window::default();
    half.fold(2, 0, 0);
    check("2 in 30 s is 4/min", half.noise_per_min(30) == 4);
    check("2 in 60 s is 2/min", half.noise_per_min(60) == 2);
    check("and 4/min fails a 3/min bar", !half.quiet(3, 30));
    check("where 2/min passes it", half.quiet(3, 60));

    // Multiply before dividing, or a window that is not a whole divisor of 60
    // collapses.
    check("7 in 45 s is 9/min, not 0", Window::per_min(7, 45) == 9);
    check("a zero window does not divide by zero", Window::per_min(5, 0) == 300);

    // Strikes veto a climb elsewhere; the window only has to carry them.
    let mut weather = Window::default();
    weather.fold(0, 6, 2);
    check("strikes are kept separately", weather.strikes == 2);
    check("and do not make the band noisy", weather.quiet(1, 60));

    let mut used = Window::default();
    used.fold(3, 4, 5);
    used.unconfirmed = 6;
    used.clear();
    check("clear empties every counter", used == Window::default());

    println!("\n  a lightning interrupt is not always a strike the tuner may act on");
    let mut batch = Batch::default();
    batch.lightning(Some(Distance::Overhead));
    check("overhead is still a strike to the console", batch.strikes == 1);
    check("...but not a confirmed one", batch.confirmed() == 0 && batch.unconfirmed == 1);
    batch.lightning(Some(Distance::Km(9)));
    check("a kilometre reading is confirmed", batch.confirmed() == 1);
    batch.lightning(Some(Distance::OutOfRange));
    check("so is out of range", batch.confirmed() == 2);
    // The detail read failed: nothing says what the chip measured, so it may
    // not vouch for anything.
    batch.lightning(None);
    check("an unreadable one is unconfirmed", batch.unconfirmed == 2 && batch.confirmed() == 2);
    check("and every one of them is still counted", batch.strikes == 4);

    // **The hold, the dip and the rescue all read `Window::strikes`**, so this
    // is the line between an overhead and a decision.
    let mut overheads = Batch::default();
    for _ in 0..3 {
        overheads.lightning(Some(Distance::Overhead));
    }
    overheads.noise = 70;
    let mut window = Window::default();
    window.fold_batch(&overheads);
    check("a window of overhead alone heard no lightning", !window.heard_lightning());
    check("...carries no strike a decision could read", window.strikes == 0);
    check("...still says how many it had", window.unconfirmed == 3);
    check("...and is judged on its noise like any other", !window.quiet(60, 60));
    let mut located = Batch::default();
    located.lightning(Some(Distance::Km(12)));
    window.fold_batch(&located);
    check("one strike with a distance and it has heard lightning", window.heard_lightning());
    check("and holds exactly that one", window.strikes == 1 && window.unconfirmed == 3);

    // September's stream, one overhead every 8 s for a minute, sixty minutes.
    let mut september_heard = false;
    for _ in 0..60 {
        let mut minute = Window::default();
        for second in 0..60 {
            let mut each = Batch::default();
            if second % 8 == 0 {
                each.lightning(Some(Distance::Overhead));
            }
            minute.fold_batch(&each);
        }
        september_heard |= minute.heard_lightning();
    }
    check("an hour of September's overhead never once reads as lightning", !september_heard);

    let (pass, fail) = (PASS.load(Ordering::Relaxed), FAIL.load(Ordering::Relaxed));
    println!("\n{pass} passed, {fail} failed");
    if fail > 0 {
        std::process::exit(1);
    }
}
