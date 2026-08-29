//! Which clock-and-sleep policy to run, as pure arithmetic.
//!
//! **Free of ESP-IDF so it can be host-tested.** `power` owns the registers —
//! `esp_pm_configure`, the frequency ceilings, reading the live configuration
//! back — and importing `esp_idf_hal::sys` puts the whole module out of reach
//! of bare `rustc`. The *decision* is a pure function of two numbers, and it is
//! the half that can be wrong quietly.
//!
//! `power` re-exports everything here, so callers still say `power::decide`.

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
/// No supply detection: see `power`'s module comment for the three schemes that
/// were tried and why each failed. What is left is about what a person is doing.
pub fn decide(uptime_s: u32, last_console_s: Option<u32>) -> Policy {
    if cfg!(feature = "no-light-sleep") {
        return Policy::Awake;
    }
    if uptime_s < GRACE_S {
        return Policy::Awake;
    }
    // **`uptime::since`, not `saturating_sub`.** The uptime counter wraps every
    // 49.7 days, and `saturating_sub` fails in the direction that costs the
    // most: after the wrap `uptime_s` is small and `seen` is near the top of
    // the range, so the subtraction saturates to 0 — which reads as "the
    // console was touched this instant" and pins the device Awake for the whole
    // next cycle. 0.170 W against 0.0127 W, and nothing reports it, because a
    // device that never sleeps looks perfectly healthy from the console.
    //
    // This site was missed when every other interval in the firmware moved to
    // wrapping arithmetic. `tests/host/policy.rs` fails without it.
    match last_console_s {
        Some(seen) if crate::uptime::since(uptime_s, seen) < CONSOLE_AWAKE_S => Policy::Awake,
        _ => Policy::Frugal,
    }
}
