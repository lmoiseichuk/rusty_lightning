//! Telling a fingertip from a USB host, and one gesture from another.
//!
//! **Free of ESP-IDF so the timing bands are host-tested**, like `defence`,
//! `verdict` and `csv`. What is here is arithmetic over one level and one clock;
//! the pin and the actions live in `listen`.
//!
//! ## Why this is hard, and why it is not debounce
//!
//! GPIO9 is the BOOT strap, and the ESP32-C3 maps the host's CDC **DTR** line
//! onto it. When `espflash`, `esptool` or a serial monitor connects it asserts
//! DTR, GPIO9 goes low, and the firmware sees a falling edge that is
//! electrically indistinguishable from a fingertip — because it *is* one.
//! GPIO9 carries the only button on the board, so there is nowhere to move to.
//!
//! Duration is the only signal that separates them, and the existing 1.5 s floor
//! gets half of it right: a flashing tool pulses DTR for a few hundred
//! milliseconds, and 1.5 s sits clear of that.
//!
//! **It does not get the other half.** A serial monitor asserts DTR and holds it
//! for the entire session — minutes, hours — which sails past a floor and
//! toggles the gain. `pyserial` does this by default on open. That is not
//! hypothetical: this device was found in outdoor mode, at roughly a quarter of
//! the indoor AFE gain, having heard nothing at all through a storm.
//!
//! So the band is closed at both ends. A person holds a button for seconds, not
//! minutes; a host holds DTR for as long as the port is open. Anything past the
//! ceiling is a cable, not a finger, and is refused **and reported**, because a
//! silently ignored press is how somebody comes to believe the button is broken.
//!
//! ## Why the level is polled rather than the edge trusted
//!
//! An edge can be missed: the loop blocks for about 3.9 s during an e-paper
//! refresh, and whether a GPIO9 edge wakes this chip from automatic light sleep
//! is not established anywhere in this tree. Polling the level and timing the
//! run of lows needs no edge at all — it only needs to look often enough, and it
//! degrades to about one-second granularity in the worst case rather than
//! failing.

/// Below this, it is a flashing tool pulsing DTR, not a person.
///
/// The original `BUTTON_HOLD_MS` was 1.5 s, chosen to clear a flashing tool's
/// few-hundred-millisecond pulse. Two seconds keeps that clearance and buys a
/// rounder number for a person to aim at: with the long gesture at ten, "two
/// seconds for gain, ten for the network" is something somebody can remember at
/// the top of a ladder without reading anything.
pub const ACCEPT_MS: u32 = 2_000;

/// At or past this, the gesture is the long one.
pub const LONG_MS: u32 = 10_000;

/// Past this, nothing human is happening: a host is holding DTR.
///
/// Ten seconds of slack over [`LONG_MS`], which is generous for somebody
/// counting to ten and still far short of a session.
pub const STUCK_MS: u32 = 30_000;

/// What a completed press turned out to be.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gesture {
    /// Shorter than [`ACCEPT_MS`]. A tool, or a brush against the case.
    TooShort,
    /// A deliberate press: toggle the indoor/outdoor gain.
    Gain,
    /// A long, deliberate press: raise or drop the access point.
    Portal,
    /// Held past [`STUCK_MS`]. A cable, and worth saying so.
    Stuck,
}

/// Tracks the run of consecutive lows and reports what it was on release.
///
/// **Dispatched on release, not on reaching a threshold.** Acting the moment a
/// hold passes 1.5 s would make the long gesture unreachable — the gain would
/// already have toggled by the time the tenth second arrived.
#[derive(Clone, Copy, Debug, Default)]
pub struct Press {
    /// When the current run of lows began. `None` while the pin is high.
    down_at_ms: Option<u32>,
    /// Set once a run passes [`STUCK_MS`], so the complaint is made once rather
    /// than on every poll for as long as the cable is attached.
    complained: bool,
}

impl Press {
    pub const fn new() -> Press {
        Press { down_at_ms: None, complained: false }
    }

    /// Feed one observation of the pin. `low` is true while it is pressed.
    ///
    /// Returns a gesture only on the release that ends a run.
    pub fn sample(&mut self, low: bool, now_ms: u32) -> Option<Gesture> {
        match (low, self.down_at_ms) {
            // Still up: nothing to do.
            (false, None) => None,
            // Newly down: start the clock.
            (true, None) => {
                self.down_at_ms = Some(now_ms);
                self.complained = false;
                None
            }
            // Still down: only worth a word if it has gone on absurdly long.
            (true, Some(_)) => None,
            // Released: classify the run that just ended.
            (false, Some(down_at)) => {
                self.down_at_ms = None;
                let held = now_ms.wrapping_sub(down_at);
                self.complained = false;
                Some(classify(held))
            }
        }
    }

    /// How long the current press has been held, if one is in progress.
    ///
    /// For the console and the screen, so somebody counting to ten can see the
    /// device counting with them.
    pub fn held_ms(&self, now_ms: u32) -> Option<u32> {
        self.down_at_ms.map(|at| now_ms.wrapping_sub(at))
    }

    /// Whether this run has just crossed [`STUCK_MS`] and should be reported.
    ///
    /// Reported while still held rather than on release, because a stuck DTR is
    /// never released — the port stays open — so a release-only report would
    /// never fire for the one case most worth knowing about.
    pub fn newly_stuck(&mut self, now_ms: u32) -> bool {
        match self.down_at_ms {
            Some(at) if !self.complained && now_ms.wrapping_sub(at) >= STUCK_MS => {
                self.complained = true;
                true
            }
            _ => false,
        }
    }
}

/// Which gesture a completed hold of `held_ms` was.
///
/// **Wrapping arithmetic is the caller's job**; this takes an elapsed time. The
/// bands are half-open and exhaustive, so every duration has exactly one answer
/// and a host test can walk the boundaries.
pub fn classify(held_ms: u32) -> Gesture {
    match held_ms {
        ms if ms < ACCEPT_MS => Gesture::TooShort,
        ms if ms < LONG_MS => Gesture::Gain,
        ms if ms < STUCK_MS => Gesture::Portal,
        _ => Gesture::Stuck,
    }
}
