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


/// Is a USB **host** talking to us right now?
///
/// `usb_serial_jtag_is_connected` reports whether the peripheral is seeing
/// start-of-frame packets — so it is exact and instant when the device is
/// plugged into a computer, which is the case that matters on a bench.
///
/// **It is not a VBUS sense**, and the difference shows up in two places:
///
/// * a **power-only charger** sends no SOF, so this reads *disconnected* on a
///   device that is very much plugged in. The gauge's discharge sign covers
///   that case — slowly, but correctly;
/// * once light sleep is running, the **USB PHY is powered down**, so plugging
///   a cable into a sleeping device will not be noticed here either. The gauge
///   covers that too, within a few minutes of `CRATE` turning positive.
///
/// So neither signal is sufficient alone, and the two fail in opposite
/// directions: this one is fast and misses chargers, the gauge is slow and
/// misses nothing.
pub fn usb_host_present() -> bool {
    // SAFETY: a read-only query with no preconditions.
    unsafe { sys::usb_serial_jtag_is_connected() }
}

/// Decide the policy from both signals.
///
/// **A USB host is the only reason to stay fast.** That is the whole rule, and
/// the first version got it backwards by asking "is it discharging?" and
/// defaulting to `Usb` when unsure.
///
/// The measurement that killed that version: a cell sitting at 3976 mV reports
/// `CRATE` of **exactly 0.00 %/hr**, not a negative number. The gauge averages
/// over minutes, so for the first few after unplugging it says nothing at all —
/// and "not discharging" was being read as "plugged in", leaving the device at
/// 160 MHz on battery precisely when it had just been unplugged.
///
/// Inverting the default costs nothing, which is what makes it obviously right:
/// **no USB host means no console**, so there is nothing left to protect by
/// keeping the clock up.
///
/// ## Host **or** charging, not both
///
/// This went round twice and the second answer is the right one. `AND` fails on
/// a case that happens constantly: **a full cell on USB reports `CRATE ≈ 0`**,
/// so "charging AND host" is false on a board that has been sitting on a desk
/// all afternoon — and the device would drop to 40 MHz with light sleep and
/// take the console down with it. A developer's board going silent because its
/// battery finished charging is not a trade worth making.
///
/// So either signal alone is enough to stay fast. The cost of the `OR` is one
/// case that is genuinely suboptimal and genuinely minor: a **power-only
/// charger** — charging, no host — keeps the clock at 160 with no console to
/// show for it, and the charger's current is shared between the MCU and the
/// pack, so ~21 mA that could have gone into the cell does not. It charges
/// slower. Nothing breaks.
///
/// `centi_per_hour` is `None` when there is no gauge or it did not answer; with
/// neither signal available, battery is both the safe assumption and the likely
/// one.
pub fn decide(centi_per_hour: Option<i32>) -> Policy {
    if usb_host_present() {
        return Policy::Usb;
    }
    match centi_per_hour {
        Some(rate) if rate > 0 => Policy::Usb,
        _ => Policy::Battery,
    }
}
