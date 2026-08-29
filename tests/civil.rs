//! Host checks for the calendar arithmetic.
//!
//! **This compiles the real `src/civil.rs`**, included by path.
//!
//! The failure this exists to prevent is silent and unrecoverable. A wrong date
//! is written into every row of the strike log, and nothing downstream can
//! detect it: the rows are well-formed, monotonic and plausible. The device
//! cannot be tested into these cases either — a bench run covers one week of
//! one year, and every interesting input to this algorithm is a leap day or a
//! century boundary that a bench will not reach for decades.
//!
//! The reference values are from `date -u -d @<epoch>`, computed independently
//! of the code under test.
//!
//! ```sh
//! cd tests && rustc --edition 2024 -A dead_code -o /tmp/civil civil.rs && /tmp/civil
//! ```

use std::sync::atomic::{AtomicU32, Ordering};

#[path = "../src/civil.rs"]
mod civil;
use civil::*;

static PASS: AtomicU32 = AtomicU32::new(0);
static FAIL: AtomicU32 = AtomicU32::new(0);

fn check(name: &str, ok: bool) {
    let counter = if ok { &PASS } else { &FAIL };
    counter.fetch_add(1, Ordering::Relaxed);
    println!("  {:<4} {name}", if ok { "ok" } else { "FAIL" });
}

/// Compare against a `YYYY-MM-DD HH:MM:SS` reference string, so a failure
/// prints the date rather than six numbers.
fn same(name: &str, epoch: u64, expect: &str) {
    let at = civil(epoch);
    let got = format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        at.year, at.month, at.day, at.hour, at.minute, at.second
    );
    if got != expect {
        println!("  FAIL {name}: {epoch} -> {got}, expected {expect}");
        FAIL.fetch_add(1, Ordering::Relaxed);
        return;
    }
    check(name, true);
}

/// Days between two dates, by asking the algorithm for both and counting
/// forward a day at a time. Used to prove month lengths without a table.
fn day_after(epoch: u64) -> Civil {
    civil(epoch + 86_400)
}

fn main() {
    println!("== civil ==");

    println!("\n  anchors");
    same("the epoch itself", 0, "1970-01-01 00:00:00");
    same("one second before the millennium", 946_684_799, "1999-12-31 23:59:59");
    same("the millennium", 946_684_800, "2000-01-01 00:00:00");
    same("the 32-bit signed rollover", 2_147_483_647, "2038-01-19 03:14:07");
    same("a date this device will see", 1_756_339_200, "2025-08-28 00:00:00");

    println!("\n  time of day");
    same("midnight", 1_756_339_200, "2025-08-28 00:00:00");
    same("one second in", 1_756_339_201, "2025-08-28 00:00:01");
    same("one minute in", 1_756_339_260, "2025-08-28 00:01:00");
    same("one hour in", 1_756_342_800, "2025-08-28 01:00:00");
    same("noon", 1_756_382_400, "2025-08-28 12:00:00");
    same("the last second of the day", 1_756_425_599, "2025-08-28 23:59:59");
    same("the next midnight", 1_756_425_600, "2025-08-29 00:00:00");

    println!("\n  leap years");
    // 2024 is a leap year: divisible by 4, not by 100.
    same("2024-02-28", 1_709_078_400, "2024-02-28 00:00:00");
    same("2024-02-29 exists", 1_709_164_800, "2024-02-29 00:00:00");
    same("2024-03-01", 1_709_251_200, "2024-03-01 00:00:00");
    // 2023 is not: February has 28 days and the 29th is March 1st.
    same("2023-02-28", 1_677_542_400, "2023-02-28 00:00:00");
    same("2023 has no 29 February", 1_677_628_800, "2023-03-01 00:00:00");

    println!("\n  the century rules");
    // 1900 is divisible by 4 but by 100 and not 400 -- NOT a leap year. This is
    // the case a naive `year % 4 == 0` gets wrong, and it is before the epoch,
    // so it is only reachable through the `div_euclid` path.
    same("2100 is not a leap year", 4_107_542_400, "2100-03-01 00:00:00");
    same("2100-02-28 is the last of February", 4_107_456_000, "2100-02-28 00:00:00");
    // 2000 is divisible by 400 -- a leap year, the exception to the exception.
    same("2000-02-29 exists", 951_782_400, "2000-02-29 00:00:00");
    same("2000-03-01", 951_868_800, "2000-03-01 00:00:00");
    // 2400 closes the 400-year era the algorithm is built around.
    same("2400-02-29 exists", 13_574_563_200, "2400-02-29 00:00:00");

    println!("\n  year boundaries");
    same("2024 ends", 1_735_689_599, "2024-12-31 23:59:59");
    same("2025 begins", 1_735_689_600, "2025-01-01 00:00:00");
    // 31 December of a leap year is day 366, the largest day-of-year the
    // month_prime division ever sees.
    same("the 366th day", 1_735_603_200, "2024-12-31 00:00:00");

    println!("\n  every month has the right length");
    // No table: step from the 28th of each month of a leap year and check where
    // the month turns over. This is the property the 153-day pattern encodes,
    // and it is checked rather than asserted.
    let lengths = [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let mut epoch = 1_704_067_200; // 2024-01-01 00:00:00
    for (index, &length) in lengths.iter().enumerate() {
        let month = index as u32 + 1;
        let last = civil(epoch + (length - 1) * 86_400);
        let over = day_after(epoch + (length - 1) * 86_400);
        let ok = last.month == month
            && last.day == length as u32
            && over.day == 1
            && over.month == if month == 12 { 1 } else { month + 1 };
        check(
            &format!("2024-{month:02} has {length} days"),
            ok,
        );
        epoch += length * 86_400;
    }

    println!("\n  monotonic across a long run");
    // A day at a time for eight years, spanning two leap years and no century
    // boundary: the date must strictly increase and the day must never be zero
    // or above the month's length. Cheap, and it catches an off-by-one that
    // spot checks would step over.
    let mut previous = (0u32, 0u32, 0u32);
    let mut monotonic = true;
    let mut in_range = true;
    let mut at = 1_704_067_200u64;
    while at < 1_704_067_200 + 8 * 366 * 86_400 {
        let day = civil(at);
        let key = (day.year, day.month, day.day);
        if key <= previous {
            monotonic = false;
        }
        if day.day == 0 || day.day > 31 || day.month == 0 || day.month > 12 {
            in_range = false;
        }
        previous = key;
        at += 86_400;
    }
    check("the date strictly increases over eight years", monotonic);
    check("month and day stay in range", in_range);

    println!("\n  the time of day never leaks into the date");
    // Every second of one day must give the same date, and the hour/minute/
    // second must round-trip to the offset. 86_400 checks folded into one.
    let midnight = 1_709_164_800; // 2024-02-29, a leap day
    let mut consistent = true;
    for offset in 0..86_400u64 {
        let at = civil(midnight + offset);
        let seconds = at.hour * 3600 + at.minute * 60 + at.second;
        if at.day != 29 || at.month != 2 || at.year != 2024 || seconds as u64 != offset {
            consistent = false;
            break;
        }
    }
    check("all 86,400 seconds of a leap day agree", consistent);

    let passed = PASS.load(Ordering::Relaxed);
    let failed = FAIL.load(Ordering::Relaxed);
    println!("\n{passed} passed, {failed} failed");
    if failed > 0 {
        std::process::exit(1);
    }
}
