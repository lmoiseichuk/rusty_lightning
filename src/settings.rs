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
