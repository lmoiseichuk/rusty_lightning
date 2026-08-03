//! Clock and sleep policy (§7).
//!
//! §7 calls power saving a nice-to-have because the device is usually
//! USB-powered. That is true of the *usual* case and useless in the one that
//! matters: on the 2000 mAh cell, the difference between the default policy and
//! a considered one is roughly four days against eighty.
//!
//! ## Why the policy switches at runtime rather than at build time
//!
//! The moisture project gated this behind a cargo feature, because that device
//! is either on a bench with a console or sealed in a box on batteries, and
//! never both. This one is the same device in both roles — plugged in at a desk
//! and carried outside on the same firmware — so the policy follows the
//! **charging state** the fuel gauge already reports. Nothing to remember at
//! flash time, and no build that is fast but flat.
//!
//! ## The three settings, in order of what they are worth
//!
//! **1. Light sleep is worth an order of magnitude more than the clock.** This
//! device spends its life blocked on a notification waiting for an interrupt,
//! which is exactly the state tickless idle exists for — roughly 22 mA awake
//! against ~130 µA asleep. Everything else here is rounding error beside it.
//!
//! **2. 40 MHz, not 80 — but only when the radio is off.** 40 MHz is the
//! crystal frequency, so capping there means the **BBPLL never has to start**.
//! The PLL costs on the order of a milliamp whenever it runs, regardless of
//! what the core does with it — so capping at 80 does not halve anything, it
//! keeps the PLL available and therefore keeps it running.
//!
//! **⚠ That cap forecloses WiFi.** The radio needs the PLL: 80 MHz APB is its
//! floor, and below that it cannot operate at all — this is not a question of
//! being slower. §5's SNTP sync and the configuration AP both need a brief
//! radio window, so that window gets its **own** policy rather than either
//! pinning the whole design at 80 or quietly failing to associate. It is short
//! and scheduled — radio up, sync, radio down — which is what makes paying
//! 80 MHz for it acceptable.
//!
//! Otherwise safe for what is attached: SPI to the panel is 4 MHz and divides
//! cleanly from a 40 MHz APB, and I2C is 100 kHz. **USB Serial/JTAG is the
//! other exception** — it needs 48 MHz from the PLL — which is why the on-USB
//! policy leaves the clock alone. The console is never lost on the power source
//! that has one.
//!
//! **3. A 10 MHz floor.** XTAL/4, which is where `esp_pm` parks the core when
//! no driver is holding a frequency lock. Waiting is what this device does
//! most of, and a lower clock while waiting is a proportional saving.

use esp_idf_hal::sys::{self, EspError};

/// What the device is running from, and what it is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Policy {
    /// Charging, or at least not discharging — treat as mains. Console alive.
    Usb,
    /// On the cell, radio off. The frugal one.
    Battery,
    /// A radio window, on either supply. **80 MHz is the floor for WiFi**, not
    /// a preference — see the module comment.
    ///
    /// Not constructed yet: §5's WiFi is deferred. It exists now rather than
    /// later because it is the constraint that decided the *other* two — the
    /// battery policy is 40 MHz precisely because this one can raise it to 80
    /// for the window that needs it, instead of the whole design paying 80 all
    /// the time for a radio that is off.
    #[allow(dead_code)]
    Wifi,
}

impl Policy {
    pub fn label(self) -> &'static str {
        match self {
            Policy::Usb => "USB",
            Policy::Battery => "battery",
            Policy::Wifi => "wifi window",
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
        // Leave everything alone. The console needs the PLL for its 48 MHz, and
        // on USB there is no reason to trade it away.
        Policy::Usb => sys::esp_pm_config_t {
            max_freq_mhz: 160,
            min_freq_mhz: 160,
            light_sleep_enable: false,
        },
        Policy::Battery => sys::esp_pm_config_t {
            max_freq_mhz: 40,
            min_freq_mhz: 10,
            light_sleep_enable: true,
        },
        // The radio's floor. Light sleep stays off: WiFi's own power saving is
        // modem sleep, which it manages itself, and layering tickless idle over
        // an association is how connections get dropped rather than saved.
        Policy::Wifi => sys::esp_pm_config_t {
            max_freq_mhz: 80,
            min_freq_mhz: 80,
            light_sleep_enable: false,
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


/// Decide the policy from the fuel gauge's discharge rate.
///
/// ## Why the USB peripheral is not consulted
///
/// An earlier version asked `usb_serial_jtag_is_connected()` first, reasoning
/// that it is exact and instant where the gauge is slow. **Measured on
/// hardware, it does not report a disconnect**: cutting USB left the device
/// claiming a host and sitting at 160 MHz indefinitely. Whatever that function
/// answers on this chip, it is not "is a cable attached" — so it is gone rather
/// than left in place looking authoritative.
///
/// ## What is left is enough
///
/// The gauge is the one signal proven to work all session: it reported
/// `0.00 %/hr` on a static cell and tracked voltage to the millivolt.
///
/// ```text
/// CRATE < 0   discharging       -> Battery
/// CRATE > 0   charging          -> Usb
/// CRATE = 0   neither, or full  -> Usb
/// ```
///
/// **`CRATE = 0` means Usb deliberately.** A fully charged cell on a desk
/// reports exactly zero, and reading that as "on battery" would drop the clock
/// and take the console with it — a board going silent because its battery
/// finished charging.
///
/// ## The cost: minutes, not milliseconds
///
/// `CRATE` is averaged over minutes, so after unplugging the device holds
/// 160 MHz until that average turns negative. Roughly 22 mA for a few minutes
/// out of 2000 mAh — under a tenth of a percent of the cell, once per unplug.
/// Correct and late beats instant and wrong.
///
/// The reverse transition needs no detection at all: **plugging USB in reboots
/// the board** — the C3 maps the host's DTR/RTS onto its reset straps — and a
/// fresh boot starts on the Usb policy.
///
/// `None` — no gauge, or it did not answer — is battery: with no evidence
/// either way, the frugal assumption is the safe one.
pub fn decide(centi_per_hour: Option<i32>) -> Policy {
    match centi_per_hour {
        Some(rate) if rate < 0 => Policy::Battery,
        Some(_) => Policy::Usb,
        None => Policy::Battery,
    }
}
