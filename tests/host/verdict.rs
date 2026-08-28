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
//! cd tests/host && rustc --edition 2024 -A dead_code -o /tmp/verdict verdict.rs && /tmp/verdict
//! ```

use std::sync::atomic::{AtomicU32, Ordering};

#[path = "../../src/verdict.rs"]
mod verdict;
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
    used.clear();
    check("clear empties every counter", used == Window::default());

    let (pass, fail) = (PASS.load(Ordering::Relaxed), FAIL.load(Ordering::Relaxed));
    println!("\n{pass} passed, {fail} failed");
    if fail > 0 {
        std::process::exit(1);
    }
}
