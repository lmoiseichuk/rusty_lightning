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
            Param { name: "noise floor", min: 0, cur: 0, max: 7, step: 1, base_step: 1, write: () },
            Param { name: "watchdog", min: 0, cur: 0, max: 15, step: 2, base_step: 2, write: () },
            Param { name: "spike rejection", min: 0, cur: 0, max: 11, step: 4, base_step: 4, write: () },
            Param { name: "min strikes", min: 0, cur: 0, max: 3, step: 8, base_step: 8, write: () },
        ],
        cursor: 0,
        last_up: None,
    }
}

fn main() {
    println!("defence:");

    let mut l = ladder();
    check("starts at every minimum", l.params.iter().all(|p| p.cur == p.min));
    check("the cursor starts on the cheapest register", l.cursor == 0);
    check("position 0", l.position() == 0);

    // --- one register at a time -------------------------------------------
    //
    // The cursor tunes the noise floor until it is exhausted, then FIXES it
    // there and moves on. Fixing rather than resetting is the point: the
    // odometer this replaced threw the floor back to 0 on every carry and
    // discarded the calibration it had just spent minutes finding.
    for _ in 0..7 { l.up(); }
    check("seven steps top out the noise floor", l.params[0].cur == 7);
    check("...with the cursor still on it", l.cursor == 0);
    check("...and nothing else touched",
        l.params[1..].iter().all(|p| p.cur == p.min));

    l.up();
    check("the next step advances the cursor", l.cursor == 1);
    check("...KEEPS the noise floor where it got to", l.params[0].cur == 7);
    check("...and starts the watchdog, which strides by 2", l.params[1].cur == 2);

    // --- the invariant ------------------------------------------------------
    let mut l = ladder();
    let mut held = true;
    while l.up() {
        if l.params.iter().skip(l.cursor + 1).any(|p| p.cur != p.min) {
            held = false;
        }
    }
    check("everything past the cursor stays at 0, always", held);
    check("the top maxes every register", l.params.iter().all(|p| p.cur == p.max));
    check("up at the top reports no movement", !l.up());

    // --- silence hands over ------------------------------------------------
    //
    // Silence means the register under the cursor has found its boundary, so it
    // steps back to the last setting that worked and the NEXT register refines
    // from there. Waiting to exhaust each register first was accurate and far
    // too slow.
    let mut l = ladder();
    l.up();
    l.up();
    check("the cursor is on the noise floor", l.cursor == 0 && l.params[0].cur == 2);
    l.down();
    check("silence steps the noise floor back one", l.params[0].cur == 1);
    check("...and hands over to the watchdog", l.cursor == 1);
    check("...which starts from 0", l.params[1].cur == 0);

    // **The cursor never goes backwards.** An earlier version retreated when the
    // current register had nothing left to give, which silently undid the
    // hand-over: the machine walked home to the noise floor, raised it again on
    // the next noisy window, and oscillated `nf 0/1` forever. Observed on
    // hardware as a regular 30-second loop.
    let mut l = ladder();
    for _ in 0..9 { l.up(); }
    check("the cursor reached the watchdog", l.cursor == 1 && l.params[1].cur > 0);
    l.params[1].cur = 0;
    let before = l.params[0].cur;
    check("a register with nothing left reports no movement", !l.down());
    check("...and does NOT retreat the cursor", l.cursor == 1);
    check("...leaving the register behind it fixed where it worked",
        l.params[0].cur == before);

    // Silence walks the cursor forward, shedding one notch per register on the
    // way -- that is how the machine relaxes without ever going backwards.
    let mut l = ladder();
    for _ in 0..3 { l.up(); }
    let floor_was = l.params[0].cur;
    l.down();
    check("silence sheds one notch and hands over", l.params[0].cur == floor_was - 1
        && l.cursor == 1);

    let mut l = ladder();
    check("down at the bottom reports no movement", !l.down());

    let mut l = ladder();
    for _ in 0..20 { l.up(); }
    l.reset();
    check("reset returns to full sensitivity", l.position() == 0 && l.cursor == 0);

    // --- a single register in isolation ------------------------------------
    let mut p = Param { name: "x", min: 0, cur: 0, max: 2, step: 1, base_step: 1, write: () };
    check("param up stops at max", p.up() && p.up() && !p.up() && p.cur == 2);
    check("param down stops at min", p.down() && p.down() && !p.down() && p.cur == 0);

    let mut p = Param { name: "wdth", min: 0, cur: 0, max: 15, step: 2, base_step: 2, write: () };
    let mut seen = vec![p.cur];
    while p.up() { seen.push(p.cur); }
    check("a short last stride still lands on max", *seen.last().unwrap() == 15);
    check("strided span counts the clamped landing", p.span() == seen.len() as u32);

    // --- slowing down near the sweet spot ---------------------------------
    let mut p = Param { name: "w", min: 0, cur: 0, max: 15, step: 8, base_step: 8, write: () };
    p.slow();
    check("a reversal halves the stride", p.step == 4);
    p.slow(); p.slow(); p.slow();
    check("the stride bottoms out at 1, never 0", p.step == 1);
    p.restore();
    check("a continuation doubles it back", p.step == 2);
    for _ in 0..5 { p.restore(); }
    check("...but never past the nominal stride", p.step == 8);

    let mut l = ladder();
    for _ in 0..3 { l.up(); }
    l.down();
    l.up();
    check("a turn slows the register being tuned", l.params[0].step == 1);

    // --- resuming a learned point ------------------------------------------
    //
    // Rounded DOWN onto the stride grid, never up: a room that has gone quiet
    // must not come back deaf.
    let mut l = ladder();
    l.restore_point([5, 9, 7, 1]);
    check("the noise floor strides by 1, so it is exact", l.params[0].cur == 5);
    check("watchdog rounds down to its stride grid", l.params[1].cur == 8);
    check("spike rejection rounds down to its stride grid", l.params[2].cur == 4);
    check("min strikes rounds down to its stride grid", l.params[3].cur == 0);
    check("the cursor lands on the dearest engaged register", l.cursor == 2);
    check("restoring clears the direction memory", l.last_up.is_none());
    check("restoring resets strides to nominal",
        l.params.iter().all(|p| p.step == p.base_step));

    let mut l = ladder();
    l.restore_point([200, 200, 200, 200]);
    check("out-of-range values clamp instead of refusing to load",
        l.params.iter().all(|p| p.cur <= p.max));

    let mut l = ladder();
    for _ in 0..12 { l.up(); }
    let saved = l.point();
    let mut fresh = ladder();
    fresh.restore_point(saved);
    check("a point saved on the grid round-trips exactly", fresh.point() == saved);

    println!(
        "\n{} passed, {} failed",
        PASS.load(Ordering::Relaxed),
        FAIL.load(Ordering::Relaxed)
    );
    std::process::exit(if FAIL.load(Ordering::Relaxed) == 0 { 0 } else { 1 });
}
