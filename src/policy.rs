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

/// How long recent lightning keeps the device awake.
///
/// **This is about weather, not about a cable.** Supply detection was tried
/// three times and each scheme failed (see `power`); what the device *can*
/// measure reliably is whether it is hearing strikes. A storm is exactly when
/// an unreachable console costs the most -- it is when the tuning wants
/// watching -- and it is also the one time the extra current buys something.
/// Quiet weather is unaffected, so the battery case is unchanged.
///
/// Matched to `golden::PATIENCE_MINUTES` (20) and to a storm that produces a
/// strike every three to five minutes: long enough that an ordinary gap between
/// flashes does not drop the port, short enough that a passing cell does not
/// pin the device awake for an hour afterwards.
pub const STORM_AWAKE_S: u32 = 15 * 60;

/// What the fuel gauge can say about the supply, reduced to what is decidable.
///
/// **Only `Discharging` is a conclusion.** `battery::Flow::Idle` cannot tell a
/// full cell on USB from a resting cell on its own -- both are flat -- so it is
/// folded into `Unknown` here rather than being given a vote it has not earned.
/// That keeps the failure recorded in §7 out of reach: a scheme that guesses
/// "mains" from a flat reading is the one that flattens a cell in two days.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Supply {
    /// The cell is measurably falling. Running on battery.
    Discharging,
    /// The cell is measurably rising. Something is charging it, so there is
    /// external power and nothing to save by sleeping.
    Charging,
    /// Flat, or not enough history. Decide on what a person is doing instead.
    Unknown,
}

/// How long console activity keeps the device awake.
///
/// Somebody typing is the clearest signal that the console matters right now —
/// it is about a person rather than a cable. Long enough to read a CSV dump or
/// set the clock without the port dying mid-sentence.
pub const CONSOLE_AWAKE_S: u32 = 10 * 60;

/// Which policy to run.
///
/// Ordered so the two conclusions that are *measured* outrank the ones that are
/// inferred. See `power`'s module comment for the three supply-detection
/// schemes that failed; `Supply` here is deliberately narrower than any of them.
pub fn decide(
    uptime_s: u32,
    last_console_s: Option<u32>,
    last_strike_s: Option<u32>,
    supply: Supply,
) -> Policy {
    if cfg!(feature = "no-light-sleep") {
        return Policy::Awake;
    }
    if uptime_s < GRACE_S {
        return Policy::Awake;
    }

    // **`uptime::since`, not `saturating_sub`.** The uptime counter wraps every
    // 49.7 days, and `saturating_sub` fails in the direction that costs the
    // most: after the wrap `uptime_s` is small and `seen` is near the top of
    // the range, so the subtraction saturates to 0 -- which reads as "the
    // console was touched this instant" and pins the device Awake for the whole
    // next cycle. 0.170 W against 0.0127 W, and nothing reports it, because a
    // device that never sleeps looks perfectly healthy from the console.
    //
    // This site was missed when every other interval in the firmware moved to
    // wrapping arithmetic. `tests/policy.rs` fails without it.
    if let Some(seen) = last_console_s {
        if crate::uptime::since(uptime_s, seen) < CONSOLE_AWAKE_S {
            return Policy::Awake;
        }
    }

    // **On the cell, sleep -- even mid-storm.** Light sleep does not stop the
    // sensor being heard, it costs the USB port; and 13x current is the
    // difference between a week and half a day. A storm is not worth spending
    // the whole battery to watch over a cable that is not plugged in.
    if supply == Supply::Discharging {
        return Policy::Frugal;
    }

    if let Some(seen) = last_strike_s {
        if crate::uptime::since(uptime_s, seen) < STORM_AWAKE_S {
            return Policy::Awake;
        }
    }

    // Something is charging the cell, so there is external power: sleeping buys
    // nothing and costs the console.
    if supply == Supply::Charging {
        return Policy::Awake;
    }

    Policy::Frugal
}
