//! lightning project, Copyright (c) 2026 Leonid Moiseichuk under MIT License
//!
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
    /// grace period after boot, whenever somebody is using the console, and
    /// for good while `awake on` is saved.
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

/// Whether a board that has never been told `awake on|off` is held awake: **no**.
///
/// The default is the instrument -- light sleep, so the cell lasts. Holding it
/// awake is something a person asks for, for a board on USB that has to stay
/// reachable, and its price is the 13x in `power`'s module comment.
pub const AWAKE_LOCK_DEFAULT: bool = false;

/// What the lock costs on the cell, in the words every report uses.
///
/// One copy, because it is printed at boot, by `status`, `health`, a bare
/// `awake` and in the web page's warning, and a figure written out at five call
/// sites is five figures.
///
/// **The asleep half is what the board has lasted, not the arithmetic.** Specs
/// §7's table gives 2.5-3.0 mA frugal as ~28-33 days of the 2000 mAh cell, but
/// that is idle current alone, and `power`'s module comment says what it leaves
/// out and which number to quote instead: the frugal board has lasted **up to
/// a week**, and "a week is the number to quote to somebody asking how long it
/// lasts". This line is that question answered, beside the toggle that decides
/// it -- so quoting ~30 here promised a month from a cell that is flat in a
/// week.
///
/// The awake half, ~2.5 days, is still the arithmetic (33.5 mA at the port),
/// because it is the only awake figure on record. It is the nearer of the two:
/// what the arithmetic leaves out -- the panel's refreshes, an access-point
/// window, the cell's self-discharge -- is a far smaller share of 33.5 mA than
/// of 2.75.
pub const AWAKE_LOCK_COST: &str = "~2.5 days on the 2000 mAh cell, against up to a week asleep";

/// The awake lock in force, from what NVS holds.
///
/// `None` is a board that has never been told -- `settings` keeps that apart
/// from "told off", as it does for every switch -- and it gets
/// [`AWAKE_LOCK_DEFAULT`]. Written here rather than as an `unwrap_or` at the
/// call site so the default is host-tested beside the rule it feeds.
pub fn awake_lock(saved: Option<bool>) -> bool {
    match saved {
        Some(on) => on,
        None => AWAKE_LOCK_DEFAULT,
    }
}

/// Which policy to run.
///
/// Ordered so the two conclusions that are *measured* outrank the ones that are
/// inferred. See `power`'s module comment for the three supply-detection
/// schemes that failed; `Supply` here is deliberately narrower than any of them.
///
/// `awake_lock` is `awake on`, saved in NVS, and it outranks everything here --
/// the grace period, the console, the weather **and the cell**. The comment
/// inside says why it does not give way to a discharging cell.
pub fn decide(
    uptime_s: u32,
    last_console_s: Option<u32>,
    last_strike_s: Option<u32>,
    supply: Supply,
    awake_lock: bool,
) -> Policy {
    if cfg!(feature = "no-light-sleep") {
        return Policy::Awake;
    }

    // **Unconditional, and above the cell's rule, on purpose.** The lock is for
    // a board on USB whose port must still be there when a script comes to
    // read it, hours after anybody last typed. Letting go of it when the
    // supply *looks* like the cell would need a signal this board does not
    // have:
    //
    // * the gauge reads a full cell on USB and a resting cell alike -- `CRATE`
    //   exactly 0 and a flat voltage -- and nothing on record says what it
    //   reads as a charge ends and the cell settles back, so a lock that gave
    //   way to `Discharging` could drop the port on USB, the one place it is
    //   for;
    // * `usb_serial_jtag_is_connected()` was measured claiming a host after
    //   the cable was pulled, and what it watches -- the host's start-of-frame
    //   packets -- also stops whenever the host suspends the bus: asleep
    //   overnight, rebooting, or autosuspending an idle port. That is the
    //   night before the morning read.
    //
    // So on the cell the lock costs the battery for as long as it is on, and
    // everything that reports the device's state says so: boot, `status`,
    // `health`, a bare `awake` and the web page.
    if awake_lock {
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
