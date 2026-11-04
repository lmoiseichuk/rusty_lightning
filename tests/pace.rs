//! lightning project, Copyright (c) 2026 Leonid Moiseichuk under MIT License
//!
//! Host checks for the pacing of walks over the whole strike log: that a walk
//! blocks often enough, and for long enough, that the task watchdog never fires.
//!
//! **This compiles the real `src/pace.rs`** (and the `src/uptime.rs` it leans
//! on), included by path, and reads the watchdog's timeout and the light-sleep
//! threshold out of the real `sdkconfig.defaults` -- so the cadence is checked
//! against the numbers the build uses, not against copies of them.
//!
//! The defect it exists for: on 0.21.1 a `dump` of 18,938 records reset the
//! board after 17,855 rows (1,319,116 bytes), because nothing in the loop
//! blocked and the idle task, which is the one thing the watchdog watches, did
//! not run for 5 s. The last section walks that log again, at the rate it was
//! measured at, against the real pacer.
//!
//! What this cannot see is whether each walk calls the pacer. They are listed
//! in `src/pace.rs`; that is for a reader, not a test.
//!
//! ```sh
//! cd tests && rustc --edition 2024 -A dead_code -o /tmp/pace pace.rs && /tmp/pace
//! ```

use std::sync::atomic::{AtomicU32, Ordering};

// `pace` reaches for `crate::uptime` for its wrap-correct interval check; at the
// crate root of this test binary that is exactly where it is.
#[path = "../src/uptime.rs"]
mod uptime;
#[path = "../src/pace.rs"]
mod pace;
use pace::*;

static PASS: AtomicU32 = AtomicU32::new(0);
static FAIL: AtomicU32 = AtomicU32::new(0);

fn check(name: &str, ok: bool) {
    let counter = if ok { &PASS } else { &FAIL };
    counter.fetch_add(1, Ordering::Relaxed);
    println!("  {:<4} {name}", if ok { "ok" } else { "FAIL" });
}

/// The build's own configuration, compiled into this binary.
///
/// `include_str!` resolves against this file, so this is the very file
/// esp-idf-sys reads, not a copy.
const SDKCONFIG: &str = include_str!("../sdkconfig.defaults");

/// The value of `name=` in `sdkconfig.defaults`, as a number.
fn setting(name: &str) -> Option<u32> {
    for line in SDKCONFIG.lines() {
        let Some((key, value)) = line.trim().split_once('=') else {
            continue;
        };
        if key == name {
            return value.trim().parse().ok();
        }
    }
    None
}

fn main() {
    println!("== pace ==");

    // --- the cadence against the watchdog ----------------------------------
    //
    // **Read, not assumed.** The IDF's default is 5 s, and the interval was
    // derived from it; if somebody shortens the watchdog, this is where the
    // interval has to be looked at again.
    let timeout_s = setting("CONFIG_ESP_TASK_WDT_TIMEOUT_S");
    check("sdkconfig.defaults sets the task watchdog's timeout", timeout_s.is_some());
    let timeout_ms = timeout_s.unwrap_or(0) * 1_000;
    check(
        &format!("the interval, {YIELD_INTERVAL_MS} ms, is at most a tenth of the {timeout_ms} ms timeout"),
        YIELD_INTERVAL_MS * 10 <= timeout_ms,
    );
    check("and is not zero, which would block on every row", YIELD_INTERVAL_MS > 0);

    // --- the block -----------------------------------------------------------
    //
    // Zero ticks is `vTaskDelay(0)`, a yield, which never lets the idle task
    // in. Three or more would let the idle task light-sleep the chip, and the
    // USB port with it, in the middle of a dump.
    check("a block is at least one tick -- zero is a yield, not a block", BLOCK_TICKS >= 1);
    let sleep_after = setting("CONFIG_FREERTOS_IDLE_TIME_BEFORE_SLEEP");
    check("sdkconfig.defaults sets the light-sleep threshold", sleep_after.is_some());
    check(
        "and a block is shorter than it, so it can never become a light sleep",
        BLOCK_TICKS < sleep_after.unwrap_or(0),
    );

    // --- what `due` says -------------------------------------------------------
    let mut pace = Pace::new(1_000);
    check("not due as the walk starts", !pace.due(1_000));
    check("not due a millisecond short of the interval", !pace.due(1_000 + YIELD_INTERVAL_MS - 1));
    check("due on the interval", pace.due(1_000 + YIELD_INTERVAL_MS));
    check("and the interval restarts there", !pace.due(1_000 + YIELD_INTERVAL_MS + 1));
    check("due again one interval later", pace.due(1_000 + 2 * YIELD_INTERVAL_MS));

    // A walk held up for ten intervals -- by a full console ring, say -- has
    // already blocked for all of it. One tick is owed, not ten.
    let mut pace = Pace::new(0);
    check("a hold-up of ten intervals is due once", pace.due(10 * YIELD_INTERVAL_MS));
    check("not ten times over", !pace.due(10 * YIELD_INTERVAL_MS + 1));

    // `now_ms()` wraps every 49.7 days; a dump across the wrap must still pace.
    let before = u32::MAX - 100;
    let mut pace = Pace::new(before);
    check("due across the 49.7-day wrap", pace.due(before.wrapping_add(YIELD_INTERVAL_MS)));
    let mut pace = Pace::new(before);
    check("and not early across it", !pace.due(before.wrapping_add(YIELD_INTERVAL_MS - 1)));

    // --- the dump that reset the board, walked again ---------------------------
    //
    // 2026-11-01, 0.21.1 at 160 MHz: 17,855 rows in the 5 s before the watchdog
    // fired, so 280 microseconds a row. Walk the whole 18,938-record log at that
    // rate on a microsecond clock, and charge every block a whole tick -- the
    // most `vTaskDelay(1)` can take at 100 Hz.
    const ROWS: u64 = 18_938;
    const ROW_US: u64 = 5_000_000 / 17_855;
    const TICK_US: u64 = 10_000;
    let interval_us = YIELD_INTERVAL_MS as u64 * 1_000;

    let mut now_us: u64 = 0;
    let mut held_us: u64 = 0; // the CPU kept since the last block
    let mut longest_held_us: u64 = 0;
    let mut blocks: u64 = 0;
    let mut pace = Pace::new(0);
    for _ in 0..ROWS {
        now_us += ROW_US;
        held_us += ROW_US;
        longest_held_us = longest_held_us.max(held_us);
        if pace.due((now_us / 1_000) as u32) {
            now_us += TICK_US * BLOCK_TICKS as u64;
            held_us = 0;
            blocks += 1;
        }
    }
    let walk_us = ROWS * ROW_US;

    check(&format!("{ROW_US} us a row, as measured"), ROW_US == 280);
    // One interval, the row that crossed it, and a millisecond of `now_ms()`
    // rounding down.
    check(
        &format!("the idle task is kept out {:.1} ms at most -- one interval and a row", longest_held_us as f64 / 1_000.0),
        longest_held_us <= interval_us + ROW_US + 1_000,
    );
    check(
        "where the same walk unpaced kept it out for the whole dump",
        walk_us > timeout_ms as u64 * 1_000,
    );
    // At most one block per interval of walking, plus the one a remainder can
    // add -- a pacer that forgot to restart would block on every row.
    check(
        &format!("the 18,938-row dump blocks {blocks} times, once an interval"),
        blocks >= 1 && blocks <= walk_us / interval_us + 1,
    );
    check(
        &format!(
            "costing {} ms on a {:.2} s walk -- {:.2} s in all",
            blocks * TICK_US / 1_000,
            walk_us as f64 / 1_000_000.0,
            now_us as f64 / 1_000_000.0
        ),
        blocks * TICK_US * 50 <= walk_us + TICK_US * 50,
    );

    // A walk shorter than one interval -- the replay of a young log -- never
    // blocks at all, so pacing costs a short log nothing.
    let mut pace = Pace::new(0);
    let mut short_blocks = 0;
    let mut now_us: u64 = 0;
    for _ in 0..1_000u64 {
        now_us += ROW_US;
        if pace.due((now_us / 1_000) as u32) {
            short_blocks += 1;
        }
    }
    check("a walk of 1,000 rows, 0.28 s, never blocks", short_blocks == 0);

    let passed = PASS.load(Ordering::Relaxed);
    let failed = FAIL.load(Ordering::Relaxed);
    println!("\n{passed} passed, {failed} failed");
    if failed > 0 {
        std::process::exit(1);
    }
}
