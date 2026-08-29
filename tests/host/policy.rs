//! Host checks for the clock-and-sleep policy.
//!
//! **This compiles the real `src/policy.rs`**, included by path.
//!
//! The failure this exists to catch is the mirror of the one `uptime` catches,
//! and it is the more expensive of the two. `uptime`'s wrap bug freezes a timer
//! so the device does nothing; this one freezes the *policy* so the device
//! never sleeps — 0.170 W against 0.0127 W, 13× on the cell, for the 49.7 days
//! until the counter comes round again. Nothing reports it. The device is
//! answering the console and logging strikes the whole time; it is simply
//! flattening its battery in about two and a half days instead of thirty.
//!
//! ```sh
//! cd tests/host && rustc --edition 2024 -A dead_code -o /tmp/policy policy.rs && /tmp/policy
//! ```

use std::sync::atomic::{AtomicU32, Ordering};

#[path = "../../src/uptime.rs"]
mod uptime;
#[path = "../../src/policy.rs"]
mod policy;
use policy::*;

static PASS: AtomicU32 = AtomicU32::new(0);
static FAIL: AtomicU32 = AtomicU32::new(0);

fn check(name: &str, ok: bool) {
    let counter = if ok { &PASS } else { &FAIL };
    counter.fetch_add(1, Ordering::Relaxed);
    println!("  {:<4} {name}", if ok { "ok" } else { "FAIL" });
}

/// The uptime counter is `esp_timer_get_time() / 1000` narrowed to `u32` and
/// then divided by 1000 again by the caller, so it wraps here.
const WRAP_S: u32 = u32::MAX / 1000;

fn main() {
    println!("== policy ==");

    println!("\n  the grace period");
    check("boot is awake", decide(0, None) == Policy::Awake);
    check(
        "still awake one second before the grace ends",
        decide(GRACE_S - 1, None) == Policy::Awake,
    );
    check(
        "frugal once the grace ends with nobody about",
        decide(GRACE_S, None) == Policy::Frugal,
    );

    println!("\n  console activity");
    check(
        "a console seen just now holds it awake",
        decide(GRACE_S, Some(GRACE_S)) == Policy::Awake,
    );
    check(
        "awake one second before the console window closes",
        decide(GRACE_S + CONSOLE_AWAKE_S - 1, Some(GRACE_S)) == Policy::Awake,
    );
    check(
        "frugal once the console window closes",
        decide(GRACE_S + CONSOLE_AWAKE_S, Some(GRACE_S)) == Policy::Frugal,
    );

    println!("\n  across the counter wrap");
    // A console touched shortly before the wrap, and the device left alone.
    // Every one of these must be Frugal: the console window closed long ago in
    // real time, and no arithmetic accident may reopen it.
    let seen = WRAP_S - 60; // console used a minute before the wrap
    for (label, now) in [
        ("one second after the wrap", 1u32),
        ("a minute after the wrap", 60),
        ("an hour after the wrap", 3_600),
        ("a day after the wrap", 86_400),
        ("a week after the wrap", 7 * 86_400),
    ] {
        // The grace period covers the first five minutes after the wrap the
        // same as after a boot, which is correct and not what is under test --
        // so the early samples are checked only past it.
        if now < GRACE_S {
            check(
                &format!("{label}: grace period, awake as designed"),
                decide(now, Some(seen)) == Policy::Awake,
            );
            continue;
        }
        check(
            &format!("{label}: frugal, the console window is long closed"),
            decide(now, Some(seen)) == Policy::Frugal,
        );
    }

    // And the console window must still *work* after the wrap: somebody typing
    // at second 100 past the wrap keeps it awake for ten minutes from then.
    check(
        "a console used after the wrap still holds it awake",
        decide(WRAP_S + 400, Some(WRAP_S + 100)) == Policy::Awake,
    );

    let passed = PASS.load(Ordering::Relaxed);
    let failed = FAIL.load(Ordering::Relaxed);
    println!("\n{passed} passed, {failed} failed");
    if failed > 0 {
        std::process::exit(1);
    }
}
