//! Clock and sleep policy (§7).
//!
//! Two policies — `Awake` at 160 MHz with no sleep, `Frugal` at 80/10 MHz with
//! light sleep — chosen by *how long since a person last did something*, not by
//! what the device is plugged into.
//!
//! Measured at the USB port: **0.170 W awake against 0.0127–0.0152 W frugal**,
//! or 33.5 mA against 2.5–3.0 mA — 13× on the 2000 mAh cell, ~2.5 days against
//! ~30.
//!
//! **See §7 for why**: those measurements in full, the three supply-detection
//! schemes that were tried and each failed, why the ceiling is 80 MHz and not
//! 40, and why the grace period below is the only way back into a board that
//! has started sleeping.

use esp_idf_hal::sys::{self, EspError};

/// What the device is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Policy {
    /// Awake and fast: the console and the flasher both work. Held for the
    /// grace period after boot, and whenever somebody is using the console.
    Awake,
    /// 80 MHz ceiling, 10 MHz floor, light sleep on. WiFi still works here —
    /// 80 is its floor — so a radio window needs no separate policy.
    Frugal,
}

impl Policy {
    pub fn label(self) -> &'static str {
        match self {
            Policy::Awake => "awake",
            Policy::Frugal => "frugal",
        }
    }
}

/// Apply the policy for a supply.
///
/// Idempotent and cheap enough to call whenever the supply is re-evaluated, but
/// the caller only does so on a *change* — `esp_pm_configure` re-plans the
/// frequency-lock bookkeeping, and doing that once a second to set the same
/// values would be work bought for nothing.
pub fn apply(policy: Policy) -> Result<(), EspError> {
    let config = match policy {
        // Full speed, no sleep. Light sleep powers down the USB PHY outright,
        // which is what made the always-frugal experiment unflashable -- so the
        // awake policy is what guarantees a way back in.
        Policy::Awake => sys::esp_pm_config_t {
            max_freq_mhz: 160,
            min_freq_mhz: 160,
            light_sleep_enable: false,
        },
        Policy::Frugal => sys::esp_pm_config_t {
            max_freq_mhz: 80,
            min_freq_mhz: 10,
            light_sleep_enable: true,
        },
    };

    // SAFETY: a plain IDF call with a fully initialised config.
    let err = unsafe { sys::esp_pm_configure(&config as *const _ as *const core::ffi::c_void) };
    crate::storage::check(err)
}

/// Ceilings `esp_pm_configure` will accept on this chip.
///
/// 160 and 80 come from the PLL; 40 is the crystal itself. Anything else is
/// rejected by IDF, and a rejected config leaves the *previous* policy running
/// while the caller believes otherwise — so this is validated before the call
/// rather than after.
pub const PINNABLE_MHZ: [u32; 3] = [40, 80, 160];

/// Pin the clock and hold light sleep off, ignoring the policy.
///
/// **A debugging override, and the reason it exists is the USB port.** Light
/// sleep powers down the USB PHY, so a board on the frugal policy is reachable
/// only in short windows — which is fine in the field and miserable when you
/// are trying to watch a console. Pinning gives a board that stays reachable
/// indefinitely without editing [`decide`] and reflashing, so the *default*
/// behaviour stays honest while it is being observed.
///
/// `min == max` deliberately: DFS between two frequencies is not the thing
/// being asked for here, a fixed clock is.
pub fn pin(mhz: u32) -> Result<(), EspError> {
    let config = sys::esp_pm_config_t {
        max_freq_mhz: mhz as i32,
        min_freq_mhz: mhz as i32,
        light_sleep_enable: false,
    };
    // SAFETY: a plain IDF call with a fully initialised config.
    let err = unsafe { sys::esp_pm_configure(&config as *const _ as *const core::ffi::c_void) };
    crate::storage::check(err)
}

/// Change light sleep alone, holding the clock where it already is.
///
/// `esp_pm_configure` takes all three together, so "just toggle sleep" means
/// reading the current pair back and writing them again unchanged — which is
/// why the caller passes them in rather than this guessing.
pub fn set_light_sleep(max_mhz: u32, min_mhz: u32, enabled: bool) -> Result<(), EspError> {
    let config = sys::esp_pm_config_t {
        max_freq_mhz: max_mhz as i32,
        min_freq_mhz: min_mhz as i32,
        light_sleep_enable: enabled,
    };
    // SAFETY: a plain IDF call with a fully initialised config.
    let err = unsafe { sys::esp_pm_configure(&config as *const _ as *const core::ffi::c_void) };
    crate::storage::check(err)
}

/// Read back what `esp_pm` is actually enforcing.
///
/// Worth having rather than trusting the write: `esp_pm_configure` rejects
/// combinations the build does not support — a `min_freq_mhz` the crystal
/// cannot divide to, or light sleep without tickless idle — and a rejected
/// config leaves the previous policy in force while the code carries on
/// believing otherwise.
pub fn config() -> Option<(u32, u32, bool)> {
    let mut config = sys::esp_pm_config_t {
        max_freq_mhz: 0,
        min_freq_mhz: 0,
        light_sleep_enable: false,
    };
    // SAFETY: as above.
    let err =
        unsafe { sys::esp_pm_get_configuration(&mut config as *mut _ as *mut core::ffi::c_void) };
    (err == sys::ESP_OK).then_some((
        config.max_freq_mhz as u32,
        config.min_freq_mhz as u32,
        config.light_sleep_enable,
    ))
}


/// How long after boot the awake policy is held, whatever else is true.
///
/// This is the escape hatch, not a nicety — see §7. Long enough to flash, and
/// always one power cycle away.
pub const GRACE_S: u32 = 5 * 60;

/// How long console activity keeps the device awake.
///
/// Somebody typing is the clearest signal that the console matters right now —
/// it is about a person rather than a cable. Long enough to read a CSV dump or
/// set the clock without the port dying mid-sentence.
pub const CONSOLE_AWAKE_S: u32 = 10 * 60;

/// Which policy to run.
///
/// No supply detection: see the module comment for the three schemes that were
/// tried and why each failed. What is left is about what a person is doing.
pub fn decide(uptime_s: u32, last_console_s: Option<u32>) -> Policy {
    if cfg!(feature = "no-light-sleep") {
        return Policy::Awake;
    }
    if uptime_s < GRACE_S {
        return Policy::Awake;
    }
    match last_console_s {
        Some(seen) if uptime_s.saturating_sub(seen) < CONSOLE_AWAKE_S => Policy::Awake,
        _ => Policy::Frugal,
    }
}
