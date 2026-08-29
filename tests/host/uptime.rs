//! Host checks for interval arithmetic across the 49.7-day wrap.
//!
//! **This compiles the real `src/uptime.rs`**, included by path. The bug it
//! exists to prevent is the worst shape a failure can have: after the wrap,
//! `saturating_sub` returns zero for ever, so every interval in the firmware
//! reads "no time has passed" — the batch never closes, the tuner never steps,
//! the screen never redraws, the log never syncs — while the interrupt keeps
//! firing and the console keeps answering. The device looks alive and does
//! nothing, for another 49.7 days.
//!
//! ```sh
//! cd tests/host && rustc --edition 2024 -A dead_code -o /tmp/uptime uptime.rs && /tmp/uptime
//! ```

use std::sync::atomic::{AtomicU32, Ordering};

#[path = "../../src/uptime.rs"]
mod uptime;
use uptime::*;

static PASS: AtomicU32 = AtomicU32::new(0);
static FAIL: AtomicU32 = AtomicU32::new(0);

fn check(name: &str, ok: bool) {
    let counter = if ok { &PASS } else { &FAIL };
    counter.fetch_add(1, Ordering::Relaxed);
    println!("  {:<4} {name}", if ok { "ok" } else { "FAIL" });
}

fn main() {
    println!("== uptime ==");

    // --- the ordinary case --------------------------------------------------
    check("no time at all", since(1_000, 1_000) == 0);
    check("one second", since(2_000, 1_000) == 1_000);
    check("a minute is due after a minute", due(61_000, 1_000, 60_000));
    check("and not a millisecond before", !due(60_999, 1_000, 60_000));
    check("exactly on the boundary is due", due(61_000, 1_000, 60_000));

    // --- the wrap, which is the whole point ---------------------------------
    //
    // A sample taken 1 s before the counter wrapped, read 1 s after it did:
    // two seconds have really passed.
    let before = u32::MAX - 1_000;
    let after = 1_000u32;
    check("two seconds across the wrap", since(after, before) == 2_001);
    check("and a one-second interval is due", due(after, before, 1_000));

    // **The failure this replaces.** `saturating_sub` gives zero here, and a
    // caller reads it as "no time has passed".
    check("saturating_sub would have said zero", after.saturating_sub(before) == 0);
    check("and would never have become due", !(after.saturating_sub(before) >= 1_000));

    // Exactly at the wrap.
    check("one tick over the top", since(0, u32::MAX) == 1);
    check("the whole range", since(u32::MAX, 0) == u32::MAX);

    // A long interval across the wrap: the six-hourly sweep is the longest here.
    let six_hours = 6 * 60 * 60 * 1_000;
    let then = u32::MAX - six_hours / 2;
    let now = six_hours / 2;
    // The gap here is six hours and one millisecond: `MAX - x` to `y` is
    // `y + x + 1`, because the wrap passes through zero. Worth spelling out,
    // because the off-by-one is exactly what a reader would get wrong.
    check("six hours spanning the wrap is due", due(now, then, six_hours));
    check("the elapsed time is six hours and one ms", since(now, then) == six_hours + 1);
    check("a seven-hour interval is not yet due", !due(now, then, 7 * 60 * 60 * 1_000));

    // --- the limit, stated rather than hidden -------------------------------
    //
    // Modular arithmetic cannot tell a very old sample from a future one. The
    // boundary is half the range, about 24.85 days.
    let half = u32::MAX / 2;
    check("just under half the range still reads forward", since(half, 0) == half);
    check(
        "and past it the answer is the wrap-around, not a negative",
        since(0, half + 2) == u32::MAX - half - 1,
    );

    // Every interval in the firmware is far inside that limit. Six hours is
    // 21,600,000 ms against a safe range of 2,147,483,647 — almost exactly one
    // per cent, which is the honest figure. (An earlier version of this check
    // asserted "under 1%" and failed by half a per cent, which is the test
    // doing its job on the person writing it.)
    let longest_used = 6 * 60 * 60 * 1_000u64;
    let safe = (u32::MAX / 2) as u64;
    check("the longest interval used is about 1% of the safe range",
        longest_used * 1_000 / safe == 10);
    check("and two orders of magnitude inside it", longest_used * 50 < safe * 100);

    let (pass, fail) = (PASS.load(Ordering::Relaxed), FAIL.load(Ordering::Relaxed));
    println!("\n{pass} passed, {fail} failed");
    if fail > 0 {
        std::process::exit(1);
    }
}
