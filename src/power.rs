//! Clock and sleep policy (§7).
//!
//! §7 calls power saving a nice-to-have because the device is usually
//! USB-powered. That is true of the *usual* case and useless in the one that
//! matters: on the 2000 mAh cell, the difference between the default policy and
//! a considered one is roughly four days against eighty.
//!
//! ## Two policies, and why it is not one
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
//! So there are two policies after all, on the one signal that has never lied:
//! **the fuel gauge's discharge rate**. It is slow — averaged over minutes — and
//! slow is fine here, because the thing being decided is only interesting after
//! the cable has been out for a while anyway.
//!
//! ```text
//! CRATE < 0   discharging       -> Battery   40/10 MHz, light sleep
//! CRATE > 0   charging          -> Usb       console stays alive
//! CRATE = 0   neither, or full  -> Usb       a full cell on a desk reads zero
//! unknown     no gauge          -> Usb       do not strand a board with no gauge
//! ```
//!
//! `CRATE = 0` meaning Usb is the load-bearing case and it is LM's: a fully
//! charged cell on a desk reports exactly zero, and reading that as "on battery"
//! would drop the clock and take the console with it.
//!
//! The reverse transition needs no detection at all: **plugging USB in reboots
//! the board** — the C3 maps the host's DTR/RTS onto its reset straps — and a
//! fresh boot starts on the Usb policy.
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

/// What the device is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Policy {
    /// Plugged in, or at least not draining: full clock, no light sleep, so the
    /// console and the flasher both keep working.
    Usb,
    /// Discharging: 40 MHz ceiling, 10 MHz floor, light sleep on.
    Battery,
    /// A radio window. **80 MHz is the floor for WiFi**, not a preference —
    /// below it the radio cannot operate at all.
    ///
    /// Not constructed yet: §5's WiFi is deferred. It exists now because it is
    /// the reason the normal policy can afford to sit at 40 — the one workload
    /// that truly needs more asks for it explicitly, instead of the whole
    /// design paying 80 all the time for a radio that is off.
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
        // Full speed and awake. The console needs the PLL for its 48 MHz, and
        // light sleep powers down the USB PHY outright -- which is what made the
        // always-frugal experiment unflashable.
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


/// Which policy the gauge's discharge rate calls for.
///
/// See the module comment for the table and for why every case except "actively
/// discharging" resolves to [`Policy::Usb`].
pub fn decide(centi_per_hour: Option<i32>) -> Policy {
    // The recovery build never leaves Usb, so the USB PHY never powers down and
    // the board stays reachable no matter what it is running from.
    if cfg!(feature = "no-light-sleep") {
        return Policy::Usb;
    }
    match centi_per_hour {
        Some(rate) if rate < 0 => Policy::Battery,
        _ => Policy::Usb,
    }
}
