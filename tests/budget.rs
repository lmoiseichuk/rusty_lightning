//! lightning project, Copyright (c) 2026 Leonid Moiseichuk under MIT License
//!
//! Host checks for the event-logging budget.
//!
//! **This compiles the real `src/budget.rs`**, included by path.
//!
//! The failure this guards is quiet and total. The strike log does not rotate,
//! so a budget larger than the free space fills the filesystem, and from then
//! on every write fails -- the strikes the device exists to record included --
//! while the console goes on answering as if nothing were wrong. Bare `events`
//! armed 20,000 rows against ~0.87 MB free and could do exactly that.
//!
//! ```sh
//! cd tests && rustc --edition 2024 -A dead_code -o /tmp/budget budget.rs && /tmp/budget
//! ```

use std::sync::atomic::{AtomicU32, Ordering};

#[path = "../src/budget.rs"]
mod budget;
use budget::*;

static PASS: AtomicU32 = AtomicU32::new(0);
static FAIL: AtomicU32 = AtomicU32::new(0);

fn check(name: &str, ok: bool) {
    let counter = if ok { &PASS } else { &FAIL };
    counter.fetch_add(1, Ordering::Relaxed);
    println!("  {:<4} {name}", if ok { "ok" } else { "FAIL" });
}

fn main() {
    println!("== budget ==");

    println!("\n  what one row costs");
    // The longest row the format can produce: a ten-digit epoch, the
    // millisecond counter at its ceiling, the longer of the two kinds and the
    // widest spike rejection. The newline is added by the writer, so it is
    // counted here.
    let longest = event_row(9_999_999_999, u32::MAX, "disturber", 7, 15, "2026-11-01 12:55:14");
    check(
        "the constant is the longest row the format writes",
        longest.len() as u32 + 1 == EVENT_ROW_BYTES,
    );
    let typical = event_row(1_793_555_714, 86_400_000, "noise", 3, 1, "2026-11-01 12:55:14");
    check("a typical row is shorter, so the cap errs safe", (typical.len() as u32 + 1) < EVENT_ROW_BYTES);
    // Twelve columns, matching the log's header: a row with a different count
    // is read at the wrong offsets after a reboot, which is how strikes once
    // went missing from the charts.
    check("a row has the header's twelve columns", longest.split(',').count() == 12);
    check(
        "and its strike-only columns are empty, not zero",
        longest.split(',').skip(6).take(4).all(|column| column.is_empty()),
    );

    println!("\n  what fits");
    check("nothing free, nothing armed", rows_that_fit(0) == 0);
    check("less than the reserve, nothing armed", rows_that_fit(RESERVED_BYTES - 1) == 0);
    check("exactly the reserve, nothing armed", rows_that_fit(RESERVED_BYTES) == 0);
    check(
        "one row past the reserve buys one row",
        rows_that_fit(RESERVED_BYTES + EVENT_ROW_BYTES) == 1,
    );
    check(
        "a row short of two buys one",
        rows_that_fit(RESERVED_BYTES + 2 * EVENT_ROW_BYTES - 1) == 1,
    );

    // **The case that prompted this**: 20,000 rows asked, ~0.87 MB free.
    let free = 870_000;
    check("the old bare default no longer fits", capped(20_000, free) < 20_000);
    check(
        "it is cut to what fits",
        capped(20_000, free) == (free - RESERVED_BYTES) / EVENT_ROW_BYTES,
    );
    check("a small budget passes untouched", capped(500, free) == 500);
    check("zero stays zero -- off is off", capped(0, free) == 0);
    check("an enormous ask is cut, not wrapped", capped(u32::MAX, free) == rows_that_fit(free));

    // The property that matters, over the whole range rather than at a few
    // points: whatever is armed, writing every row of it leaves the reserve
    // standing.
    let mut never_spends_the_reserve = true;
    let mut free = 0u32;
    while free < 4_000_000 {
        let armed = capped(u32::MAX, free);
        let spent = armed as u64 * EVENT_ROW_BYTES as u64;
        if armed > 0 && spent + RESERVED_BYTES as u64 > free as u64 {
            never_spends_the_reserve = false;
        }
        free += 4_093; // a prime step, so no alignment hides an edge
    }
    check("no budget ever reaches into the reserve", never_spends_the_reserve);

    let passed = PASS.load(Ordering::Relaxed);
    let failed = FAIL.load(Ordering::Relaxed);
    println!("\n{passed} passed, {failed} failed");
    if failed > 0 {
        std::process::exit(1);
    }
}
