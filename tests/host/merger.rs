//! Host checks for the merge window (§4.3).
//!
//! **This compiles the real `src/merger.rs`**, included by path. It had no
//! coverage at all until it was moved out of `session`, which drives an I²C
//! sensor and cannot be built on a workstation.
//!
//! That absence mattered more than it sounds. The claim "930 of those records
//! were return strokes" rests entirely on this window — and of 1040 records in
//! `storm-2026-08-12.csv`, **not one shares a second with another**, so with a
//! one-second window there was never a second stroke inside one. The argument
//! was circular and nothing could have shown it.
//!
//! ```sh
//! cd tests/host && rustc --edition 2024 -A dead_code -o /tmp/merger merger.rs && /tmp/merger
//! ```

use std::sync::atomic::{AtomicU32, Ordering};

#[path = "../../src/strike.rs"]
mod strike;
// `merger` reaches for `crate::uptime` for its wrap-correct interval check, so
// the real one is compiled in here too rather than stubbed.
#[path = "../../src/uptime.rs"]
mod uptime;
#[path = "../../src/merger.rs"]
mod merger;

// `merger` reaches for `crate::strike`; at the crate root of this test binary
// that is exactly where `strike` is.
use crate::strike::{Distance, Strike};
use merger::*;

static PASS: AtomicU32 = AtomicU32::new(0);
static FAIL: AtomicU32 = AtomicU32::new(0);

fn check(name: &str, ok: bool) {
    let counter = if ok { &PASS } else { &FAIL };
    counter.fetch_add(1, Ordering::Relaxed);
    println!("  {:<4} {name}", if ok { "ok" } else { "FAIL" });
}

/// A merger with a chosen window. `Merger` is `Default` plus `set_window_ms`,
/// which is the shipped path -- the console changes the window on a live one.
fn with_window(window_ms: u32) -> Merger {
    let mut m = Merger::default();
    m.set_window_ms(window_ms);
    m
}

fn s(distance: Distance, energy_raw: u32) -> Strike {
    Strike { distance, energy_raw }
}

fn main() {
    println!("== merger ==");

    // --- one stroke, no merging --------------------------------------------
    let mut m = with_window(1_000);
    check("the first stroke does not complete a flash",
        m.observe(&s(Distance::Km(5), 100), Some(10), 1, false, 0).is_none());
    let done = m.take_due(1_001);
    check("and it completes once the window passes", done.is_some());
    let done = done.unwrap();
    check("one stroke means strokes == 1", done.strokes == 1);
    check("with its own energy", done.strike.energy_raw == 100);
    check("and its own distance", done.strike.distance == Distance::Km(5));

    // --- two strokes inside the window --------------------------------------
    let mut m = with_window(1_000);
    m.observe(&s(Distance::Km(5), 100), Some(10), 1, false, 0);
    m.observe(&s(Distance::Km(7), 200), Some(11), 1, false, 500);
    let done = m.take_due(1_001).expect("window closed");
    check("two strokes fold into one flash", done.strokes == 2);
    check("energy is summed", done.strike.energy_raw == 300);
    // **The first stroke's clock, not the last.** A merge must not move an
    // event later than it happened.
    check("the flash keeps the FIRST stroke's epoch", done.epoch == Some(10));

    // --- the window runs from the first stroke, not the last -----------------
    //
    // A sliding window would let a long enough train merge without limit, and a
    // storm overhead would collapse into one record hours long.
    let mut m = with_window(1_000);
    m.observe(&s(Distance::Km(5), 10), Some(0), 1, false, 0);
    m.observe(&s(Distance::Km(5), 10), Some(0), 1, false, 900);
    let spilled = m.observe(&s(Distance::Km(5), 10), Some(0), 1, false, 1_100);
    check("a stroke past the window closes the flash rather than extending it",
        spilled.is_some());
    check("and the closed flash holds only the two inside it",
        spilled.map(|f| f.strokes) == Some(2));

    // --- a zero window means no merging at all -------------------------------
    //
    // `strokes` must never be zero: the console and the CSV both treat it as a
    // count of real strokes.
    let mut m = with_window(0);
    let first = m.observe(&s(Distance::Km(5), 10), Some(0), 1, false, 0);
    let second = m.observe(&s(Distance::Km(5), 10), Some(0), 1, false, 0);
    check("a zero window emits per stroke", first.is_some() || second.is_some());
    for f in [first, second].into_iter().flatten() {
        check("and never reports zero strokes", f.strokes >= 1);
    }

    // --- distances: the two sentinels are not distances ----------------------
    // **A kilometre reading wins over the nearest-bin sentinel.** `finish` uses
    // `overhead` only when there were no kilometre samples at all, so a flash
    // mixing the two reports the average of the real readings.
    //
    // Worth a note rather than a change: "overhead" means *nearer than 5 km*, so
    // a flash containing one is arguably nearer than a 9 km stroke suggests, and
    // this reports the further of the two. It is self-consistent and documented,
    // and altering it would move every mixed record -- so it is recorded here as
    // the contract and raised in findings rather than quietly rewritten.
    let mut m = with_window(1_000);
    m.observe(&s(Distance::Overhead, 10), Some(0), 1, false, 0);
    m.observe(&s(Distance::Km(9), 10), Some(0), 1, false, 100);
    let done = m.take_due(2_000).expect("closed");
    check("a kilometre reading outranks an overhead sentinel",
        done.strike.distance == Distance::Km(9));

    // With no kilometre reading at all, the sentinel is all there is.
    let mut m = with_window(1_000);
    m.observe(&s(Distance::Overhead, 10), Some(0), 1, false, 0);
    m.observe(&s(Distance::Overhead, 10), Some(0), 1, false, 100);
    let done = m.take_due(2_000).expect("closed");
    check("overhead alone stays overhead", done.strike.distance == Distance::Overhead);

    let mut m = with_window(1_000);
    m.observe(&s(Distance::Km(4), 10), Some(0), 1, false, 0);
    m.observe(&s(Distance::Km(8), 10), Some(0), 1, false, 100);
    let done = m.take_due(2_000).expect("closed");
    check("kilometre readings average", done.strike.distance == Distance::Km(6));

    let mut m = with_window(1_000);
    m.observe(&s(Distance::OutOfRange, 10), Some(0), 1, false, 0);
    let done = m.take_due(2_000).expect("closed");
    check("out of range survives as itself", done.strike.distance == Distance::OutOfRange);

    // --- changing the window flushes what is pending -------------------------
    let mut m = with_window(10_000);
    m.observe(&s(Distance::Km(5), 10), Some(0), 1, false, 0);
    check("shortening the window flushes the pending flash",
        m.set_window_ms(1_000).is_some());

    // --- nothing pending, nothing due ---------------------------------------
    let mut m = with_window(1_000);
    check("an idle merger has nothing due", m.take_due(999_999).is_none());

    // --- the wrap ------------------------------------------------------------
    let mut m = with_window(1_000);
    m.observe(&s(Distance::Km(5), 10), Some(0), 1, false, u32::MAX - 500);
    check("a window spanning the millisecond wrap still closes",
        m.take_due(600).is_some());

    let (pass, fail) = (PASS.load(Ordering::Relaxed), FAIL.load(Ordering::Relaxed));
    println!("\n{pass} passed, {fail} failed");
    if fail > 0 {
        std::process::exit(1);
    }
}
