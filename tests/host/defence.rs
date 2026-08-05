//! Host checks for §4.2's noise-rejection ladder.
//!
//! Worth testing because the ladder decides how deaf the sensor is, and the
//! interesting cases are its two ends: that level 0 is the chip's own defaults,
//! and that no level can ever reach past the noise floor into the two knobs
//! that reject lightning rather than noise.
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

fn main() {
    println!("defence:");

    check("level 0 is every knob at its base", settings(0) == Settings {
        noise_floor: NOISE_FLOOR_BASE, watchdog: WATCHDOG_BASE, spike_reject: SPIKE_REJECT_BASE });

    // --- the whole ladder is the noise floor ------------------------------
    check("level 7 tops out the noise floor", settings(7).noise_floor == NOISE_FLOOR_MAX);
    check("...without touching the watchdog", settings(7).watchdog == WATCHDOG_BASE);
    check("...or spike rejection", settings(7).spike_reject == SPIKE_REJECT_BASE);

    // --- the cap, which is what this file now exists for ------------------
    //
    // WDTH and SREJ reject lightning, not noise -- see the module comment. A
    // ladder that can reach them is the defect these three checks exist to
    // catch, so they assert the *absence* of the old rungs 8..31.
    check("MAX_LEVEL is 7 -- the noise floor's own range", MAX_LEVEL == 7);
    check("no level ever moves the watchdog off its default",
        (0..=255u8).all(|l| settings(l).watchdog == WATCHDOG_BASE));
    check("no level ever moves spike rejection off its default",
        (0..=255u8).all(|l| settings(l).spike_reject == SPIKE_REJECT_BASE));
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
    check("every rung is a noise-floor rung",
        (0..=255u8).all(|l| rung(l) == "noise floor"));

    println!(
        "\n{} passed, {} failed",
        PASS.load(Ordering::Relaxed),
        FAIL.load(Ordering::Relaxed)
    );
    std::process::exit(if FAIL.load(Ordering::Relaxed) == 0 { 0 } else { 1 });
}
