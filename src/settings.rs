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
/// **Deliberately renamed from `defence`.** The four bytes behind the old key
/// were stored in *sensitivity* units and in the old row order, so reading one
/// back under the current convention turns a learned quiet point into a nearly
/// deaf one — the highest values landing on `min strikes`, which is the most
/// destructive register of the four. A new key makes every device that has ever
/// stored a point fall through to "no stored point" once and re-learn, which
/// costs minutes; honouring the old bytes would cost a storm.
const KEY_DEFENCE: &[u8] = b"defence3\0";
const KEY_QUIET: &[u8] = b"quiet\0";
/// How many records the log held at the last sync.
///
/// **In NVS, which is a different partition from the log.** That is the whole
/// point: it survives a littlefs format, so after one the device can say how
/// much was lost instead of coming back looking like it had never seen a storm.
const KEY_LOGGED: &[u8] = b"log_recs\0";
const KEY_MERGE: &[u8] = b"merge_ms\0";
const KEY_SREJ: &[u8] = b"srej\0";
const KEY_WDTH: &[u8] = b"wdth\0";

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
/// How many records the log held when it was last synced.
pub fn logged_records() -> Option<u32> {
    Namespace::open(NAMESPACE).ok()?.get_u32(KEY_LOGGED)
}

/// Remember the count, so a lost filesystem can be reported rather than guessed.
pub fn store_logged_records(records: u32) -> Result<(), EspError> {
    let nvs = Namespace::open(NAMESPACE)?;
    nvs.set_u32(KEY_LOGGED, records)?;
    nvs.commit()
}

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



/// The learned tuning point — one byte per register, packed into a word.
///
/// **Persisted because it describes the room, not the run.** A device that has
/// spent ten minutes discovering how noisy its corner is should not rediscover
/// it from scratch on every reset, least of all during the storm that caused the
/// reset. Restoring lands it near the answer in one step instead of climbing
/// there a window at a time.
///
/// One key rather than four: the registers were only meaningful together, and a
/// power cut between two of four writes would leave a point that was never
/// actually in force. Since the point shrank to a single 3-bit field that is no
/// longer a design decision so much as a description of the value — but the key
/// stays one key, because the argument comes back the moment anything rejoins
/// the space.
///
/// `None` on a device that has never tuned, where the caller starts from full
/// sensitivity.
pub fn defence_point() -> Option<crate::defence::Point> {
    let nvs = Namespace::open(NAMESPACE).ok()?;
    let packed = nvs.get_u32(KEY_DEFENCE)?;
    // `new` clamps, so a value written by a build with a different layout
    // degrades to something in range rather than being trusted whole.
    let point = crate::defence::Point::new(packed as u16);
    // **Said out loud, because the clamp is otherwise invisible.** A getter
    // that prints is not the habit to get into, but this is the one place that
    // can see both numbers, and the alternative is a boot line reporting a
    // point nobody chose with nothing to explain it.
    //
    // 0.11.0 is exactly when this matters: the layout went from seven bits to
    // three, so a stored value above 7 is a leftover in which `nf` shared the
    // word with a watchdog that is no longer in it. Clamping is safe -- `nf`
    // cannot reject a lightning waveform, and the +/-1 walk relaxes it back
    // down on the first quiet window -- but it is not the value that was saved.
    if packed as u16 > crate::defence::MAX {
        println!(
            "as:   stored defence point {packed} is above this build's max {} -- \
             clamped to {}, from an older layout",
            crate::defence::MAX,
            point.raw()
        );
    }
    Some(point)
}

/// Persist the learned point.
///
/// Rate-limiting is the **caller's** job — the machine can move every window,
/// and writing flash at that cadence to protect a value that re-learns in
/// minutes would be the wrong trade.
pub fn store_defence_point(point: crate::defence::Point) -> Result<(), EspError> {
    let nvs = Namespace::open(NAMESPACE)?;
    nvs.set_u32(KEY_DEFENCE, point.raw() as u32)?;
    nvs.commit()
}


/// The rate at or below which a window counts as **quiet**, in events/minute.
///
/// **The zero that the tuner and the search both compare against.** Nothing else
/// in the system needs it, and it exists because a literal zero turned out to be
/// the wrong test: a probe asks "did this window hear anything", so a longer
/// probe has strictly more chances to catch one stray event, and a 60 s sweep
/// therefore settled deafer than a 10 s one in the same room — spending spike
/// rejection and then min strikes on one to two events a minute, having rejected
/// other points at a hundred a minute by the identical verdict.
///
/// Persisted because it describes the room's noise floor as a *policy*, not as a
/// measurement: a garage beside a compressor and a quiet study want different
/// answers, and neither wants to be re-entered after a power cut.
///
/// `None` on a device that has never been told, where the caller uses
/// [`crate::session::QUIET_PER_MIN`].
pub fn quiet_per_min() -> Option<u32> {
    let nvs = Namespace::open(NAMESPACE).ok()?;
    Some(nvs.get_u32(KEY_QUIET)?)
}

/// Persist the quiet threshold. Written only when it is given explicitly.
pub fn store_quiet_per_min(rate: u32) -> Result<(), EspError> {
    let nvs = Namespace::open(NAMESPACE)?;
    nvs.set_u32(KEY_QUIET, rate)?;
    nvs.commit()
}

/// Spike rejection, 0..=15.
///
/// A setting rather than part of the tuning point: see
/// [`crate::defence::SPIKE_REJECTION_DEFAULT`] for what it cost to have the
/// tuner able to spend it. `None` on a device never told.
pub fn spike_rejection() -> Option<u8> {
    let nvs = Namespace::open(NAMESPACE).ok()?;
    Some(nvs.get_u32(KEY_SREJ)? as u8)
}

/// Persist the spike rejection level.
pub fn store_spike_rejection(level: u8) -> Result<(), EspError> {
    let nvs = Namespace::open(NAMESPACE)?;
    nvs.set_u32(KEY_SREJ, level as u32)?;
    nvs.commit()
}

/// The watchdog threshold the operator has chosen, 0–15.
///
/// A setting rather than part of the tuning point, and for a sharper reason
/// than spike rejection: see [`crate::defence::WATCHDOG_DEFAULT`]. It gates on
/// amplitude, so the tuner spending it cost the distant strikes this device
/// exists to report before the thunder does. `None` on a device never told.
pub fn watchdog() -> Option<u8> {
    let nvs = Namespace::open(NAMESPACE).ok()?;
    Some(nvs.get_u32(KEY_WDTH)? as u8)
}

/// Persist the watchdog threshold.
pub fn store_watchdog(level: u8) -> Result<(), EspError> {
    let nvs = Namespace::open(NAMESPACE)?;
    nvs.set_u32(KEY_WDTH, level as u32)?;
    nvs.commit()
}

/// The strike-merge window in milliseconds (§4.3).
///
/// Stored like the quiet threshold and for the same reason: it describes the
/// weather a room sees rather than a debugging session, so it should survive a
/// power cut. `None` on a device never told, where the caller uses
/// [`crate::merger::MERGE_WINDOW_MS`].
pub fn merge_window_ms() -> Option<u32> {
    let nvs = Namespace::open(NAMESPACE).ok()?;
    Some(nvs.get_u32(KEY_MERGE)?)
}

/// Persist the merge window.
pub fn store_merge_window_ms(window_ms: u32) -> Result<(), EspError> {
    let nvs = Namespace::open(NAMESPACE)?;
    nvs.set_u32(KEY_MERGE, window_ms)?;
    nvs.commit()
}
