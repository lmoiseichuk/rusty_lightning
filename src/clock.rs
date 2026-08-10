//! Wall-clock time without a network (§5).
//!
//! ## "Fake NTP": a persisted epoch plus uptime
//!
//! There is no RTC battery and, for now, no WiFi — so the device cannot know
//! what time it is when it powers on. What it *can* do is remember what time it
//! was when it was last told, and count forward.
//!
//! So the clock is set once over the console (`time <unix-epoch>`), written to
//! NVS, and re-loaded on every boot. Between boots the system clock runs from
//! there. It drifts, and the drift does not matter at the resolution this
//! device works in: strikes are bucketed at fifteen minutes, and the RC
//! oscillator is good to a few seconds a day.
//!
//! ## Why it is saved periodically rather than only when set
//!
//! A device told the time on Monday and power-cycled on Friday would otherwise
//! come back believing it was Monday — the stored epoch would be four days
//! stale. Re-saving as it runs keeps the stored value close to the truth, so a
//! power cut costs only the time the device spent *off*, which nothing can
//! recover anyway.
//!
//! The write is cheap and rare (see [`SAVE_INTERVAL_S`]), and NVS wear-levels.

use core::fmt::Write as _;

use esp_idf_hal::sys::{self, EspError};

use crate::storage::Namespace;

const NAMESPACE: &[u8] = b"settings\0";
const KEY_EPOCH: &[u8] = b"epoch\0";
const KEY_TZ_MINUTES: &[u8] = b"tz_min\0";

/// How often the current time is written back to NVS.
///
/// Fifteen minutes bounds how far the stored epoch can lag reality, so an
/// unexpected power cut costs at most that much apparent time. Against NVS's
/// endurance that is roughly 35 000 writes a year spread by wear levelling —
/// negligible, and the value is one entry.
pub const SAVE_INTERVAL_S: u32 = 15 * 60;

/// A time we are confident enough in to stamp records with.
///
/// Anything before this is a device that has never been told the time — the
/// system clock starts near zero at power-on, and stamping strikes with 1970
/// would produce a log that sorts and buckets wrongly forever.
const PLAUSIBLE_EPOCH: u64 = 1_700_000_000; // 2023-11-14

/// Seconds since the Unix epoch, or `None` if the clock has never been set.
///
/// `None` is deliberately not "zero" or "uptime". A caller writing a strike
/// record has to decide what to do about an unknown time, and hiding that
/// behind a plausible-looking number is how a database ends up with a decade of
/// events in 1970.
pub fn now() -> Option<u64> {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs();
    (seconds >= PLAUSIBLE_EPOCH).then_some(seconds)
}

/// Set the system clock and remember it.
pub fn set(epoch: u64) -> Result<(), EspError> {
    let time = sys::timeval {
        tv_sec: epoch as sys::time_t,
        tv_usec: 0,
    };
    // SAFETY: a plain libc call with a fully initialised struct.
    unsafe {
        sys::settimeofday(&time, core::ptr::null());
    }
    save(epoch)
}

/// Restore the clock from NVS, if there is anything to restore.
///
/// Returns what was restored, for the boot banner — a device that comes up
/// without a time should say so, because every record it writes until it is
/// told will be unstamped.
pub fn restore() -> Option<u64> {
    // **A clock that is already running wins.**
    //
    // The ESP32's RTC keeps counting across a reset -- a reflash, a watchdog, a
    // button reboot -- so the system clock is often still correct when this
    // runs. Restoring on top of it does not recover lost time; it *destroys*
    // good time, replacing a live clock with whatever NVS last saved, which is
    // up to `SAVE_INTERVAL_S` -- fifteen minutes -- stale.
    //
    // Observed: the clock was set, the board reflashed twice within four
    // minutes, and each boot rewound it to the moment of the set. Every strike
    // stamped in between would have carried a timestamp minutes early, which is
    // the one field a strike log cannot be wrong about.
    //
    // So restore is now what its name implies: a fallback for a clock that has
    // nothing, not an overwrite of one that has something. A true power cut
    // still lands here, because the RTC stops without power.
    if let Some(running) = now() {
        return Some(running);
    }

    let nvs = Namespace::open(NAMESPACE).ok()?;
    let epoch = nvs.get_u64(KEY_EPOCH)?;
    if epoch < PLAUSIBLE_EPOCH {
        return None;
    }
    set(epoch).ok()?;
    Some(epoch)
}

/// Write the current time to NVS. Called on a timer; see the module comment.
pub fn save(epoch: u64) -> Result<(), EspError> {
    let nvs = Namespace::open(NAMESPACE)?;
    nvs.set_u64(KEY_EPOCH, epoch)?;
    nvs.commit()
}

/// `YYYY-MM-DD HH:MM:SS` for a Unix timestamp, UTC.
///
/// Written out rather than pulled from a date library: `chrono` and friends are
/// large, and this needs one format with no locale, no zones and no parsing.
/// The civil-from-days conversion is Howard Hinnant's, which is exact for every
/// date this device will ever see.
pub fn format(epoch: u64) -> heapless::String<20> {
    let days = (epoch / 86_400) as i64;
    let time_of_day = epoch % 86_400;

    // Shift the epoch to 0000-03-01 so leap days land at the end of the cycle.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;

    let day = (day_of_year - (153 * month_prime + 2) / 5 + 1) as u32;
    let month = if month_prime < 10 {
        month_prime + 3
    } else {
        month_prime - 9
    } as u32;
    let year = (year_of_era + era * 400 + if month <= 2 { 1 } else { 0 }) as u32;

    // `heapless::String` implements `core::fmt::Write`, so `write!` works on it
    // exactly as it does on a `std::string::String` — the only difference is
    // that it returns `Err` when the fixed capacity is reached instead of
    // growing. Nineteen characters into a 20-byte buffer never will, hence the
    // discarded result. This replaced a hand-rolled digit loop; `{:02}` is the
    // same zero padding, written once.
    let mut out = heapless::String::new();
    let _ = write!(
        out,
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02}",
        time_of_day / 3600,
        (time_of_day % 3600) / 60,
        time_of_day % 60
    );
    out
}



// === Local time ============================================================
//
// The stored epoch is, and stays, **UTC**. A Unix timestamp is defined that
// way, and a log that stores local time is a log that becomes ambiguous twice
// a year and unreadable if the device moves. So the offset is applied on the
// way *out*, for the things a person reads.

/// Minutes east of UTC. Negative for the Americas.
///
/// Stored in minutes rather than hours so that half-hour and quarter-hour zones
/// — India, Nepal, parts of Australia — need no format change later.
pub fn tz_minutes() -> i32 {
    Namespace::open(NAMESPACE)
        .ok()
        .and_then(|nvs| nvs.get_i32(KEY_TZ_MINUTES))
        .unwrap_or(0)
}

pub fn set_tz_minutes(minutes: i32) -> Result<(), EspError> {
    let nvs = Namespace::open(NAMESPACE)?;
    nvs.set_i32(KEY_TZ_MINUTES, minutes)?;
    nvs.commit()
}

/// `YYYY-MM-DD HH:MM:SS` in local time.
///
/// Everything a person reads goes through this; everything stored uses
/// [`format`] on the raw UTC epoch. Keeping the two apart is what stops a log
/// becoming ambiguous at a daylight-saving boundary.
pub fn format_local(epoch: u64) -> heapless::String<20> {
    let shifted = (epoch as i64 + tz_minutes() as i64 * 60).max(0) as u64;
    format(shifted)
}

/// The offset as `UTC-4` / `UTC+5:30`, for display.
pub fn tz_label() -> heapless::String<12> {
    let minutes = tz_minutes();
    let mut out = heapless::String::new();
    let _ = out.push_str("UTC");
    if minutes == 0 {
        return out;
    }
    let sign = if minutes < 0 { '-' } else { '+' };
    let magnitude = minutes.unsigned_abs();
    let _ = write!(out, "{sign}{}", magnitude / 60);
    // Only the odd zones — India, Nepal, parts of Australia — reach this.
    if magnitude % 60 != 0 {
        let _ = write!(out, ":{:02}", magnitude % 60);
    }
    out
}
