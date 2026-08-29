//! Host checks for the button gesture bands.
//!
//! **This compiles the real `src/press.rs`**, included by path. The bands are
//! the whole design, and every one of them is a claim about a boundary that no
//! amount of pressing a button on a bench would explore reliably.
//!
//! The claim worth defending is the ceiling. GPIO9 carries the host's CDC DTR
//! line as well as the only button, so a serial monitor that asserts DTR for a
//! whole session drives it low for minutes — and under the old floor-only rule
//! that toggled the AFE gain. This device was found in outdoor mode, at about a
//! quarter of the indoor gain, having heard nothing through a storm.
//!
//! ```sh
//! cd tests/host && rustc --edition 2024 -A dead_code -o /tmp/press press.rs && /tmp/press
//! ```

use std::sync::atomic::{AtomicU32, Ordering};

#[path = "../../src/press.rs"]
mod press;
use press::*;

static PASS: AtomicU32 = AtomicU32::new(0);
static FAIL: AtomicU32 = AtomicU32::new(0);

fn check(name: &str, ok: bool) {
    let counter = if ok { &PASS } else { &FAIL };
    counter.fetch_add(1, Ordering::Relaxed);
    println!("  {:<4} {name}", if ok { "ok" } else { "FAIL" });
}

fn main() {
    println!("== press ==");

    // --- the bands, at their boundaries ------------------------------------
    check("0 ms is too short", classify(0) == Gesture::TooShort);
    check("a 300 ms espflash DTR pulse is too short", classify(300) == Gesture::TooShort);
    check("one tick under the floor is too short", classify(ACCEPT_MS - 1) == Gesture::TooShort);
    check("the floor itself is a gain press", classify(ACCEPT_MS) == Gesture::Gain);
    check("2 s is a gain press", classify(2_000) == Gesture::Gain);
    check("1.5 s no longer is -- the floor moved to 2 s", classify(1_500) == Gesture::TooShort);
    check("one tick under the long band is still gain", classify(LONG_MS - 1) == Gesture::Gain);
    check("ten seconds is the portal", classify(LONG_MS) == Gesture::Portal);
    check("one tick under the ceiling is still the portal", classify(STUCK_MS - 1) == Gesture::Portal);

    // **The ceiling is the half that was missing.** A held DTR sails past any
    // floor; only a ceiling refuses it.
    check("the ceiling is stuck, not a portal press", classify(STUCK_MS) == Gesture::Stuck);
    check("a five-minute serial session is stuck", classify(300_000) == Gesture::Stuck);
    check("an hour is stuck", classify(3_600_000) == Gesture::Stuck);

    // Exhaustive: every duration lands in exactly one band, in order.
    let mut last = Gesture::TooShort;
    let mut changes = 0;
    for ms in 0..40_000u32 {
        let g = classify(ms);
        if g != last {
            changes += 1;
            last = g;
        }
    }
    check("the bands are contiguous and ordered (3 transitions)", changes == 3);

    // --- the state machine --------------------------------------------------
    let mut p = Press::new();
    check("a press in progress reports nothing", p.sample(true, 1_000).is_none());
    check("still held reports nothing", p.sample(true, 3_000).is_none());
    check("release classifies the run", p.sample(false, 3_000) == Some(Gesture::Gain));
    check("and then nothing again", p.sample(false, 4_000).is_none());

    // **Dispatch on release, not on crossing.** Acting at 1.5 s would make the
    // long gesture unreachable: the gain would already have toggled.
    let mut p = Press::new();
    p.sample(true, 0);
    for t in [2_000, 5_000, 9_000, 11_000] {
        check(&format!("no gesture at {t} ms while still held"), p.sample(true, t).is_none());
    }
    check("released at 12 s is the portal", p.sample(false, 12_000) == Some(Gesture::Portal));

    // The device can show a person how long they have held it.
    let mut p = Press::new();
    check("nothing held, no duration", p.held_ms(500).is_none());
    p.sample(true, 1_000);
    check("held for 4 s reads as 4 s", p.held_ms(5_000) == Some(4_000));

    // A stuck cable never releases, so the complaint cannot wait for one.
    let mut p = Press::new();
    p.sample(true, 0);
    check("not stuck at 10 s", !p.newly_stuck(10_000));
    check("stuck at 30 s", p.newly_stuck(30_000));
    check("and says so only once", !p.newly_stuck(31_000));
    check("and not again at five minutes", !p.newly_stuck(300_000));
    p.sample(false, 400_000);
    p.sample(true, 400_100);
    check("a fresh press can complain again", p.newly_stuck(431_000));

    // The millisecond counter wraps every 49.7 days and this device runs on.
    let mut p = Press::new();
    p.sample(true, u32::MAX - 1_000);
    check("a press across the wrap is timed correctly",
        p.sample(false, 1_000) == Some(Gesture::Gain));

    let (pass, fail) = (PASS.load(Ordering::Relaxed), FAIL.load(Ordering::Relaxed));
    println!("\n{pass} passed, {fail} failed");
    if fail > 0 {
        std::process::exit(1);
    }
}
