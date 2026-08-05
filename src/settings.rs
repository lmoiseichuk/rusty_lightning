//! What survives a power cut, and nothing else.
//!
//! **Storage only** — no interpretation and no policy. Right now that is one
//! value, the indoor/outdoor location, but the boundary is worth drawing at the
//! first value rather than the fifth: §5's persisted epoch and §4.3's tuned
//! thresholds land here too, and a storage layer that also owns rules becomes
//! the module everything reaches into.

use esp_idf_hal::sys::EspError;

use crate::as3935::Location;
use crate::storage::Namespace;

const NAMESPACE: &[u8] = b"settings\0";
const KEY_LOCATION: &[u8] = b"location\0";
const KEY_BATTERY_MIN: &[u8] = b"bat_min\0";
const KEY_BATTERY_MAX: &[u8] = b"bat_max\0";
const KEY_DRAIN_SUM: &[u8] = b"bat_sum\0";
const KEY_DRAIN_SECONDS: &[u8] = b"bat_cnt\0";

/// Encoded so the stored byte is not a bare 0/1 whose meaning is invisible in a
/// hex dump.
const STORED_INDOOR: u8 = 1;
const STORED_OUTDOOR: u8 = 2;

/// The stored location, or `None` if this unit has never been told.
///
/// `None` is not an error — it is what a virgin device reports, and the caller
/// supplies the default. Returning a default from here would hide the
/// difference between "never configured" and "configured, and it happens to be
/// indoor", which matters the first time someone wonders whether their button
/// press actually stuck.
pub fn location() -> Option<Location> {
    let nvs = Namespace::open(NAMESPACE).ok()?;
    match nvs.get_u8(KEY_LOCATION)? {
        STORED_INDOOR => Some(Location::Indoor),
        STORED_OUTDOOR => Some(Location::Outdoor),
        // A byte we did not write. Treat as unset rather than guessing, so a
        // future encoding change degrades to "run the default" instead of to
        // "silently outdoors".
        _ => None,
    }
}

/// Persist the location. Written only when it actually changes — a button press
/// is rare, but so is a flash write budget.
pub fn store_location(location: Location) -> Result<(), EspError> {
    let value = match location {
        Location::Indoor => STORED_INDOOR,
        Location::Outdoor => STORED_OUTDOOR,
    };
    let nvs = Namespace::open(NAMESPACE)?;
    nvs.set_u8(KEY_LOCATION, value)?;
    nvs.commit()
}


/// The lowest and highest cell voltage this unit has actually seen, in mV.
///
/// `None` until the device has observed anything — the caller seeds from
/// [`crate::battery::SEED_RANGE`], and the difference between "never observed"
/// and "observed, and it happens to equal the seed" is worth keeping.
pub fn battery_range() -> Option<(u16, u16)> {
    let nvs = Namespace::open(NAMESPACE).ok()?;
    let low = nvs.get_u16(KEY_BATTERY_MIN)?;
    let high = nvs.get_u16(KEY_BATTERY_MAX)?;
    // A pair where the low is not below the high is not a range; treat it as
    // unset rather than reasoning from it.
    (low < high).then_some((low, high))
}

/// Persist an observed range. Written only when an endpoint actually moves.
pub fn store_battery_range(low: u16, high: u16) -> Result<(), EspError> {
    let nvs = Namespace::open(NAMESPACE)?;
    nvs.set_u16(KEY_BATTERY_MIN, low)?;
    nvs.set_u16(KEY_BATTERY_MAX, high)?;
    nvs.commit()
}


/// The discharge accumulator: millivolts shed, and over how many seconds.
///
/// **Four battery values in NVS, and these are the last two** — the borders
/// (`bat_min`/`bat_max`) say how far the cell can fall, and this pair says how
/// fast it is falling. Together they are a runtime estimate; neither half is
/// one alone.
///
/// `None` when the device has never accumulated anything, which is a first boot
/// or the first poll after a charge reset it.
pub fn battery_drain() -> Option<crate::battery::Drain> {
    let nvs = Namespace::open(NAMESPACE).ok()?;
    let sum_mv = nvs.get_i32(KEY_DRAIN_SUM)?;
    let seconds = nvs.get_i32(KEY_DRAIN_SECONDS)?;
    // A negative on either side is a value this never writes; treat it as
    // absent rather than reasoning from it.
    if sum_mv < 0 || seconds < 0 {
        return None;
    }
    Some(crate::battery::Drain {
        sum_mv: sum_mv as u32,
        seconds: seconds as u32,
    })
}

/// Persist the accumulator.
///
/// Written on a reset and then only every [`crate::battery::DRAIN_SAVE_S`], not
/// on every poll: the gauge is read every ten seconds, and a flash write at
/// that cadence to protect a number whose whole purpose is to average over days
/// would be the wrong trade. A power cut costs the last interval, which is a
/// rounding error against a multi-day baseline.
pub fn store_battery_drain(drain: crate::battery::Drain) -> Result<(), EspError> {
    let nvs = Namespace::open(NAMESPACE)?;
    nvs.set_i32(KEY_DRAIN_SUM, drain.sum_mv as i32)?;
    nvs.set_i32(KEY_DRAIN_SECONDS, drain.seconds as i32)?;
    nvs.commit()
}
