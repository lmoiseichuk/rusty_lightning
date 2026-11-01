//! How many disturber and noise rows the strike log can afford.
//!
//! **Free of ESP-IDF so the arithmetic can be host-tested.** `log` owns the
//! file, the filesystem and the free-space query; this owns the two questions
//! that decide whether arming `events` can fill the flash -- what one event row
//! costs, and how many of them fit -- because getting either wrong is silent
//! until the filesystem is full and every write fails, strikes included.
//!
//! The row itself is built here too, so the size the cap assumes is the size
//! of the row actually written rather than a figure somebody counted once.

/// The longest event row the log can write, newline included, in bytes.
///
/// **Derived from the format, worst case, not measured on a typical row.** A
/// typical row is ~58 B; the cap has to hold for every row, and the fields that
/// grow are the millisecond counter (a `u32`, up to ten digits) and spike
/// rejection (up to two). `tests/budget.rs` builds the longest row with
/// [`event_row`] and checks it against this number, so the two cannot drift.
///
/// | field | width |
/// |---|---|
/// | epoch seconds | 10 (until the year 2286) |
/// | millis | 10 (`u32::MAX`) |
/// | kind | 9 (`disturber`) |
/// | nf | 1 |
/// | srej | 2 |
/// | local ISO time | 19 |
/// | simulated, strokes | 1 + 1 |
/// | separators | 11 |
/// | newline | 1 |
pub const EVENT_ROW_BYTES: u32 = 65;

/// Free space a budget may never spend, in bytes: 256 KiB.
///
/// **Kept for strikes**, which are what the log exists to hold and which an
/// event window must not be able to crowd out. At the measured 74 B per strike
/// row that is ~3,500 records -- the largest real storm logged here was 917,
/// so nearly four of them -- and it leaves LittleFS the spare blocks its
/// copy-on-write needs to keep committing anything at all.
pub const RESERVED_BYTES: u32 = 256 * 1024;

/// How many event rows fit in `free_bytes` without touching the reserve.
pub fn rows_that_fit(free_bytes: u32) -> u32 {
    free_bytes.saturating_sub(RESERVED_BYTES) / EVENT_ROW_BYTES
}

/// The budget actually armed when `asked` rows are requested.
///
/// **Every budget goes through this**, typed or restored after a restart.
/// `events` with no argument used to arm 20,000 rows -- about 1.3 MB at the
/// worst-case row, against ~0.87 MB free on the board that prompted this --
/// so the bare command could fill the filesystem by itself.
pub fn capped(asked: u32, free_bytes: u32) -> u32 {
    asked.min(rows_that_fit(free_bytes))
}

/// One event row, without its newline: what the chip called a disturber or
/// noise, and when.
///
/// Every strike-only column is empty rather than zero -- the chip rejected the
/// waveform before measuring it, so there is no distance or energy to report,
/// and a zero would read as one. `simulated` and `strokes` are `0`.
pub fn event_row(epoch: u64, millis: u32, kind: &str, nf: u8, srej: u8, iso: &str) -> String {
    format!("{epoch},{millis},{kind},{nf},{srej},{iso},,,,,0,0")
}
