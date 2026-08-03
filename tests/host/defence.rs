//! Host checks for §4.2's noise-rejection ladder.
//!
//! A copy of `src/defence.rs`'s logic — see README.md.
//!
//! Worth testing because the ladder is three ranges welded end to end, and the
//! joins are where an off-by-one lives: a level that skips a rung, repeats one,
//! or writes a register value past its field width would all look plausible on
//! a console and would only misbehave under sustained noise.

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

fn main() {
    println!("defence:");

    check("level 0 is every knob at its base", settings(0) == Settings {
        noise_floor: NOISE_FLOOR_BASE, watchdog: WATCHDOG_BASE, spike_reject: SPIKE_REJECT_BASE });

    // --- the first rung: noise floor only ---------------------------------
    check("level 7 tops out the noise floor", settings(7).noise_floor == NOISE_FLOOR_MAX);
    check("...without touching the watchdog", settings(7).watchdog == WATCHDOG_BASE);
    check("...or spike rejection", settings(7).spike_reject == SPIKE_REJECT_BASE);

    // --- the joins, which is what this file is for ------------------------
    check("level 8 starts the watchdog, noise floor pinned", settings(8) == Settings {
        noise_floor: NOISE_FLOOR_MAX, watchdog: WATCHDOG_BASE + 1, spike_reject: SPIKE_REJECT_BASE });
    check("level 20 tops out the watchdog", settings(20).watchdog == WATCHDOG_MAX);
    check("...spike rejection still untouched", settings(20).spike_reject == SPIKE_REJECT_BASE);
    check("level 21 starts spike rejection", settings(21) == Settings {
        noise_floor: NOISE_FLOOR_MAX, watchdog: WATCHDOG_MAX, spike_reject: SPIKE_REJECT_BASE + 1 });

    // --- the top ----------------------------------------------------------
    check("MAX_LEVEL is 31", MAX_LEVEL == 31);
    check("MAX_LEVEL maxes every knob", settings(MAX_LEVEL) == Settings {
        noise_floor: NOISE_FLOOR_MAX, watchdog: WATCHDOG_MAX, spike_reject: SPIKE_REJECT_MAX });
    check("past the top saturates rather than wrapping", settings(255) == settings(MAX_LEVEL));

    // --- properties that must hold across the whole ladder ----------------
    let mut monotonic = true;
    let mut in_range = true;
    for level in 0..=MAX_LEVEL {
        let s = settings(level);
        if level > 0 {
            let previous = settings(level - 1);
            // Exactly one knob moves per rung, and only ever upward.
            let moved = (s.noise_floor != previous.noise_floor) as u8
                + (s.watchdog != previous.watchdog) as u8
                + (s.spike_reject != previous.spike_reject) as u8;
            if moved != 1
                || s.noise_floor < previous.noise_floor
                || s.watchdog < previous.watchdog
                || s.spike_reject < previous.spike_reject
            {
                monotonic = false;
            }
        }
        // Field widths: NF_LEV is 3 bits, WDTH and SREJ are 4.
        if s.noise_floor > 7 || s.watchdog > 15 || s.spike_reject > 15 {
            in_range = false;
        }
    }
    check("every rung moves exactly one knob, upward only", monotonic);
    check("no setting ever exceeds its register field", in_range);

    // --- the rung labels --------------------------------------------------
    check("rung names follow the ladder", rung(0) == "noise floor"
        && rung(7) == "noise floor" && rung(8) == "watchdog"
        && rung(20) == "watchdog" && rung(21) == "spike rejection"
        && rung(MAX_LEVEL) == "spike rejection");

    println!(
        "\n{} passed, {} failed",
        PASS.load(Ordering::Relaxed),
        FAIL.load(Ordering::Relaxed)
    );
    std::process::exit(if FAIL.load(Ordering::Relaxed) == 0 { 0 } else { 1 });
}
