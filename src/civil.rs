//! Unix timestamp to calendar date, with no hardware and no library.
//!
//! **Free of ESP-IDF _and_ of every crate dependency**, so it host-tests under
//! bare `rustc` like `defence`, `verdict`, `csv`, `press`, `uptime` and
//! `merger`. It lived inside `clock`, which opens NVS and reads the RTC, so the
//! one part of that module that is pure arithmetic had no coverage — and it is
//! the part whose failures are silent and permanent, because a wrong date is
//! written into every row of the strike log and cannot be recovered afterwards.
//!
//! The split is at the calendar, not at the string: [`civil`] does the hard
//! part and returns numbers, and `clock::format` does the trivial `write!`.
//! That keeps `heapless` — which bare `rustc` has no way to find — on the
//! firmware side of the line, and it puts the tests where the bugs are. Leap
//! years, the 400-year cycle and the century exceptions are what this algorithm
//! exists to get right; zero padding is not.

/// A broken-down UTC date and time.
///
/// Plain public fields rather than accessors: this is a bag of numbers with no
/// invariant to protect beyond the one [`civil`] establishes when it builds it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Civil {
    pub year: u32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
}

/// Break a Unix timestamp into its UTC calendar parts.
///
/// Written out rather than pulled from a date library: `chrono` and friends are
/// large, and this needs one conversion with no locale, no zones and no
/// parsing. The civil-from-days conversion is Howard Hinnant's, exact for every
/// date this device will ever see.
///
/// The trick it turns on is the shift to 0000-03-01. Move the start of the year
/// to March and February — the only month whose length varies — becomes the
/// *last* month, so a leap day lands at the end of the cycle where it perturbs
/// nothing after it. That is what lets the rest be division rather than a table
/// of month lengths and a run of `if` for the century rules.
pub fn civil(epoch: u64) -> Civil {
    let days = (epoch / 86_400) as i64;
    let time_of_day = epoch % 86_400;

    // Shift the epoch to 0000-03-01 so leap days land at the end of the cycle.
    let z = days + 719_468;

    // An *era* is one full 400-year Gregorian cycle: 146_097 days, which is a
    // whole number of weeks and the period over which the calendar repeats
    // exactly. `div_euclid` rather than `/` so that pre-1970 timestamps — which
    // this device will not produce, but which a corrupted NVS read could —
    // floor towards negative infinity instead of towards zero, keeping
    // `day_of_era` non-negative.
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);

    // Divide out the leap days: one per 4 years, minus one per 100, plus one
    // per 400. The `- day_of_era / 146_096` term is the correction for the last
    // day of the era, which would otherwise round into the next year.
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);

    // With March as month 0, month lengths follow a 5-month 153-day pattern
    // closely enough that this single division recovers the month exactly.
    let month_prime = (5 * day_of_year + 2) / 153;

    let day = (day_of_year - (153 * month_prime + 2) / 5 + 1) as u32;
    let month = if month_prime < 10 {
        month_prime + 3
    } else {
        month_prime - 9
    } as u32;
    // January and February belong to the *next* calendar year than the shifted
    // one, so they get the +1 back.
    let year = (year_of_era + era * 400 + if month <= 2 { 1 } else { 0 }) as u32;

    Civil {
        year,
        month,
        day,
        hour: (time_of_day / 3600) as u32,
        minute: ((time_of_day % 3600) / 60) as u32,
        second: (time_of_day % 60) as u32,
    }
}
