//! Host checks for §4.2's noise-rejection ladder.
//!
//! Worth testing because the machine decides how deaf the sensor is, and its
//! *ordering* is the whole design: the cheapest register must move on every
//! step and the most destructive one only when everything else is exhausted.
//! An ordering mistake would be invisible by inspection and would suppress
//! detection in exactly the conditions the device exists for.
//!
//! **This compiles the real `src/defence.rs`**, included by path — not a copy
//! of it. Keeping that module free of ESP-IDF imports is what makes this
//! possible, and it means a change to the ladder that breaks these checks fails
//! here rather than silently drifting away from what ships.

use std::sync::atomic::{AtomicU32, Ordering};

#[path = "../../src/defence.rs"]
mod defence;
use defence::*;

// Atomics rather than `static mut`. Two counters in a single-threaded test
// binary are the textbook case where `static mut` looks harmless, but taking a
// reference to one — which `println!("{PASS}")` does implicitly — is undefined
// behaviour, and Rust 2024 makes it a hard error rather than a warning.
// `AtomicU32` costs nothing here and is the idiomatic replacement.
static PASS: AtomicU32 = AtomicU32::new(0);
static FAIL: AtomicU32 = AtomicU32::new(0);

fn check(name: &str, ok: bool) {
    let counter = if ok { &PASS } else { &FAIL };
    counter.fetch_add(1, Ordering::Relaxed);
    println!("  {:<4} {name}", if ok { "ok" } else { "FAIL" });
}

/// The real machine's shape, with a placeholder writer.
///
/// The writer is a type parameter precisely so this can be built without
/// ESP-IDF — `()` carries no behaviour and the checks below are about the carry
/// logic, not about programming a chip. The ranges are kept in step with
/// `session::new_ladder` by hand; a divergence would show up as a failing count.
fn ladder() -> Ladder<()> {
    Ladder {
        params: [
            Param { name: "min strikes", min: 0, cur: 0, max: 3, write: () },
            Param { name: "spike rejection", min: 0, cur: 0, max: 11, write: () },
            Param { name: "watchdog", min: 0, cur: 0, max: 15, write: () },
            Param { name: "noise floor", min: 0, cur: 0, max: 7, write: () },
        ],
    }
}

fn main() {
    println!("defence:");

    let mut l = ladder();
    check("starts at every minimum", l.params.iter().all(|p| p.cur == p.min));
    check("position 0", l.position() == 0);
    check("total is the product of the spans", l.total() == 4 * 12 * 16 * 8);

    // --- the ordering, which is the design --------------------------------
    //
    // The noise floor is the only register that cannot reject a strike, so it
    // must move on every step. `min strikes` is the most destructive and must
    // not move until everything else is exhausted.
    l.up();
    check("the first step moves the noise floor", l.params[3].cur == 1);
    check("...and nothing else", l.params[0].cur == 0 && l.params[1].cur == 0 && l.params[2].cur == 0);

    let mut l = ladder();
    for _ in 0..7 { l.up(); }
    check("seven steps top out the noise floor", l.params[3].cur == 7);
    l.up();
    check("the eighth carries into the watchdog", l.params[2].cur == 1);
    check("...and hands the noise floor back", l.params[3].cur == 0);

    let mut l = ladder();
    let mut steps = 0u32;
    while l.params[0].cur == 0 && l.up() { steps += 1; }
    check("min strikes waits for the other three to be exhausted", steps == 8 * 16 * 12);

    // --- exhaustion and relaxation ----------------------------------------
    let mut l = ladder();
    let total = l.total();
    let mut count = 0u32;
    while l.up() { count += 1; }
    check("up reaches every position exactly once", count == total - 1);
    check("the top maxes every register", l.params.iter().all(|p| p.cur == p.max));
    check("up at the top reports no movement", !l.up());

    // Down walks back the cheapest register first -- never a reverse carry,
    // which would relax one register by maxing every cheaper one.
    let mut l = ladder();
    l.up();
    l.up();
    l.down();
    check("down retreats the noise floor", l.params[3].cur == 1);
    check("down at the bottom reports no movement", !ladder().down());

    let mut l = ladder();
    for _ in 0..20 { l.up(); }
    l.reset();
    check("reset returns to full sensitivity", l.position() == 0);

    // --- a single register in isolation ------------------------------------
    let mut p = Param { name: "x", min: 0, cur: 0, max: 2, write: () };
    check("param up stops at max", p.up() && p.up() && !p.up() && p.cur == 2);
    check("param down stops at min", p.down() && p.down() && !p.down() && p.cur == 0);
    check("span counts both ends", p.span() == 3);

    println!(
        "\n{} passed, {} failed",
        PASS.load(Ordering::Relaxed),
        FAIL.load(Ordering::Relaxed)
    );
    std::process::exit(if FAIL.load(Ordering::Relaxed) == 0 { 0 } else { 1 });
}
