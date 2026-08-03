//! Clock and sleep policy (§7).
//!
//! §7 calls power saving a nice-to-have because the device is usually
//! USB-powered. That is true of the *usual* case and useless in the one that
//! matters: on the 2000 mAh cell, the difference between the default policy and
//! a considered one is roughly four days against eighty.
//!
//! ## One policy after the grace period
//!
//! This started out switching between a fast USB policy and a frugal battery
//! one, on whatever signal could distinguish them. Three signals were tried and
//! all three were wrong in some case that actually happens:
//!
//! | Signal | How it failed |
//! |---|---|
//! | Evaluated on the redraw path | up to fifteen minutes late |
//! | `CRATE < 0` | reads exactly `0.00 %/hr` for minutes after unplugging |
//! | `usb_serial_jtag_is_connected()` | never reports a disconnect at all |
//!
//! The tempting conclusion was to stop switching entirely and run the frugal
//! policy always. That was tried, and **it makes the board unreachable**: light
//! sleep powers down the USB PHY, so within a minute the port stops opening
//! (`Errno 71, Protocol error`) and the device can be neither observed nor
//! flashed without the BOOT-button dance. A device that cannot be debugged over
//! USB is a poor trade for current it only needs to save when nobody is
//! watching.
//!
//! **Supply detection is gone entirely.** Three schemes were tried and each was
//! wrong in a case that happens: evaluated on the redraw path it was fifteen
//! minutes late; `CRATE` reads exactly zero for minutes after unplugging; and
//! `usb_serial_jtag_is_connected()` never reports a disconnect at all. None of
//! them earned their complexity.
//!
//! What replaces them is a **grace period plus console activity**. The device
//! runs fast and awake for five minutes after boot — long enough to flash it —
//! and drops to the frugal policy after, whatever it is plugged into. Anyone who
//! needs the console back types something, which extends the awake window; see
//! `console`. Both signals are about what a *person* is doing, which is the
//! thing that actually decides whether the console matters.
//!
//! ## The three settings, in order of what they are worth
//!
//! **1. Light sleep is worth an order of magnitude more than the clock.** This
//! device spends its life blocked on a notification waiting for an interrupt,
//! which is exactly the state tickless idle exists for — roughly 22 mA awake
//! against ~130 µA asleep. Everything else here is rounding error beside it.
//!
//! **2. 80 MHz, chosen over 40.** 40 MHz is the crystal frequency, so capping
//! there would keep the BBPLL from ever starting — worth about a milliamp while
//! awake. 80 keeps the PLL running and gives that back.
//!
//! It buys two things that are worth more than a milliamp on a device that is
//! asleep most of the time:
//!
//! * **WiFi works.** 80 MHz APB is the radio's floor — below it the radio
//!   cannot operate at all. At 80 the AP and any sync need no separate policy,
//!   no switching, and no window to schedule. A whole mechanism disappears.
//! * **So does USB Serial/JTAG**, which needs 48 MHz from the PLL.
//!
//! And the milliamp is small beside what light sleep saves: at a 5 % awake
//! fraction the difference between 40 and 80 is roughly 124 days against 95,
//! while the difference between sleeping and not is 95 days against four.
//!
//! **3. A 10 MHz floor.** XTAL/4, which is where `esp_pm` parks the core when
//! no driver is holding a frequency lock. Waiting is what this device does
//! most of, and a lower clock while waiting is a proportional saving.

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


/// Which policy the gauge's discharge rate calls for.
///
/// See the module comment for the table and for why every case except "actively
/// discharging" resolves to [`Policy::Usb`].
/// How long after boot the awake policy is held, whatever else is true.
///
/// Light sleep powers down the USB PHY, so a board running it is reachable only
/// in windows of a few seconds — recovering one cost an entire evening. Five
/// minutes of guaranteed awake after every boot is all a flash needs, and it
/// makes the escape hatch always one power cycle away.
///
/// It costs nothing a deployed device notices: five minutes of 160 MHz once per
/// boot, against months of sleeping.
pub const GRACE_S: u32 = 5 * 60;

/// How long console activity keeps the device awake.
///
/// Somebody typing at the console is the clearest possible signal that the
/// console matters right now — better than any supply detection, because it is
/// about a person rather than about a cable. Ten minutes is long enough to read
/// a CSV dump or set the clock without the port dying mid-sentence.
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
