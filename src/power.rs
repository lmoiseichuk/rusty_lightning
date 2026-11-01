//! Clock and sleep policy (§7).
//!
//! Two policies — `Awake` at 160 MHz with no sleep, `Frugal` at 80/10 MHz with
//! light sleep — chosen by *how long since a person last did something*, not by
//! what the device is plugged into.
//!
//! Measured at the USB port: **0.170 W awake against 0.0127–0.0152 W frugal**,
//! or 33.5 mA against 2.5–3.0 mA — 13× on the 2000 mAh cell.
//!
//! **What that ratio does *not* give you is the runtime.** 2000 mAh at 2.75 mA
//! is about thirty days, and the observed figure is **up to a week**. The
//! arithmetic is idle current and nothing else: it charges nothing for the
//! 3.8 s at full draw that every e-paper refresh costs, nor the five-minute
//! awake grace after each boot, nor an access-point window, nor the cell's own
//! self-discharge. So the 13× is the number to reason about when choosing a
//! policy — it is a ratio between two measured currents — and a week is the
//! number to quote to somebody asking how long it lasts.
//!
//! **See §7 for why**: those measurements in full, the three supply-detection
//! schemes that were tried and each failed, why the ceiling is 80 MHz and not
//! 40, and why the grace period below is the only way back into a board that
//! has started sleeping.

use esp_idf_hal::sys::{self, EspError};

// The policy *decision* lives in `policy`, which is free of ESP-IDF so it can
// be host-tested; re-exported so callers still say `power::decide` and
// `power::Policy`. This module keeps what needs the registers.
pub use crate::policy::{decide, Policy, Supply};

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

/// Let the sensor's interrupt wake the chip from automatic light sleep.
///
/// ## What was happening without it
///
/// **Asleep, the IRQ could not be heard at all.** This build has
/// `CONFIG_PM_SLP_DISABLE_GPIO` -- the IDF selects it on the C3 as part of a
/// workaround for a chip that resets when a floating input catches a static
/// pulse in light sleep -- so every pad switches to a sleep configuration with
/// its input disabled. No GPIO was a wake source either, so an event that
/// arrived while the core slept waited for the loop's one-second timer. And the
/// AS3935 reports only the *latest* reason: a second event inside that second
/// overwrote the first, and the log's `millis` column recorded when the loop
/// got round to it rather than when it happened.
///
/// ## What this does
///
/// `gpio_wakeup_enable` sets the pin to wake on a high level, and -- with
/// `CONFIG_PM_SLP_DISABLE_GPIO` on, read in the IDF 5.3.4 source -- also calls
/// `gpio_sleep_sel_dis` for it, so this one pad keeps its awake configuration
/// (input, pull-down) through sleep while the rest stay isolated. Exempting
/// it is safe on the workaround's own terms: its hazard is a *floating* input,
/// and this one is driven by the sensor's push-pull INT. The IDF applies the
/// sleep switching once at start-up, not on each `esp_pm_configure`, so the
/// policy changes in [`apply`] do not undo it.
///
/// `esp_sleep_enable_gpio_wakeup` then adds the GPIO block to the light-sleep
/// wake sources. The level must be the pin's interrupt trigger too: the C3 has
/// one trigger field per pad for both, which is why `main` sets the interrupt
/// to `HighLevel` rather than the rising edge it used before.
///
/// ## What it costs
///
/// One wake per event the chip raises, each about one scheduler tick awake:
/// the 8 ms settle before the register read rounds up to the 10 ms tick.
/// **Masked** -- the default -- disturbers never reach the pin, so the extra
/// wakes are noise and strikes only: at most about one a second at the
/// default quiet threshold of 60/min, and in the quiet rooms that threshold is
/// for, far fewer. **Unmasked**, at the 6-10 disturbers a second measured
/// indoors on 2026-11-01, it is 6-10 wakes a second, 6-10 % of the time awake
/// -- bounded above by the 33.5 mA measured at 160 MHz, so at worst roughly
/// doubling the 2.5-3.0 mA frugal draw. Unmasked was already an experiment
/// rather than a resting state; this is part of what it costs on the cell.
pub fn wake_on_irq(gpio: i32) -> Result<(), EspError> {
    // SAFETY: plain IDF calls on a pad `main` owns and has configured as an
    // input; neither touches memory the caller holds.
    unsafe {
        crate::storage::check(sys::gpio_wakeup_enable(
            gpio,
            sys::gpio_int_type_t_GPIO_INTR_HIGH_LEVEL,
        ))?;
        crate::storage::check(sys::esp_sleep_enable_gpio_wakeup())
    }
}
