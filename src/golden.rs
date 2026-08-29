//! The settings that were in force when the device last heard lightning.
//!
//! **The one unambiguous signal this device ever gets.** Everything else it
//! measures is quiet, and quiet is two different things wearing the same face:
//! a working receiver under a still sky, and a deaf one under a storm. The
//! tuner cannot tell them apart, which is why "minimise events" has deafness as
//! its global optimum — a receiver that hears nothing has scored perfectly.
//!
//! A detected strike breaks that symmetry. It is proof, not inference, that
//! this exact combination of registers was able to hear lightning from this
//! room. So it is written down, and it becomes the thing the device falls back
//! to and boots from, instead of a mid-range guess nobody measured.
//!
//! **Free of ESP-IDF so it can be host-tested**: this module holds the record
//! and the fall-back rule, `settings` stores it, and `tuning` acts on it.

/// A complete sensitivity configuration.
///
/// **All four together, because none of them means anything alone.** A noise
/// floor learned at indoor gain describes a front end seeing roughly four times
/// what the outdoor one sees, so carrying it across a gain change is how a good
/// point becomes a deaf one — which this project has already done once and
/// written down.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Combo {
    /// `NF_LEV`, 0..=7. The only knob the tuner walks.
    pub nf: u8,
    /// `WDTH`, 0..=15. Higher discards slow-rising distant arrivals.
    pub wdth: u8,
    /// `SREJ`, 0..=15. Zero admits man-made impulses as lightning.
    pub srej: u8,
    /// The AFE gain. `false` is indoor, which is the sensitive one.
    pub outdoor: bool,
}

impl Combo {
    /// Pack into the `u32` NVS stores.
    ///
    /// Widths are the registers' own, not the smallest that fit today: `nf` is
    /// three bits because the register is three bits, so a build that puts more
    /// of the space back under the tuner does not silently reinterpret a stored
    /// value.
    pub fn pack(self) -> u32 {
        (self.nf as u32 & 0x7)
            | ((self.wdth as u32 & 0xF) << 3)
            | ((self.srej as u32 & 0xF) << 7)
            | ((self.outdoor as u32) << 11)
    }

    pub fn unpack(packed: u32) -> Combo {
        Combo {
            nf: (packed & 0x7) as u8,
            wdth: ((packed >> 3) & 0xF) as u8,
            srej: ((packed >> 7) & 0xF) as u8,
            outdoor: (packed >> 11) & 1 == 1,
        }
    }

    /// Whether `self` is at least as able to hear a weak strike as `other`.
    ///
    /// **Only comparable at the same gain.** Across a gain change the question
    /// is meaningless and the honest answer is "no", not a guess.
    ///
    /// Deafness here is ordered by the three knobs that can lose a strike, each
    /// of which loses it a different way: `nf` by refusing to look at a signal
    /// that quiet, `wdth` by discarding a waveform that rises that slowly,
    /// `srej` by discarding one that short. Lower is more receptive in all
    /// three, so this is a plain per-field comparison and not a score — a score
    /// would let a gain in one knob pay for a loss in another, and they do not
    /// trade: a strike rejected by the watchdog is not recovered by a lower
    /// noise floor.
    pub fn at_least_as_open_as(self, other: Combo) -> bool {
        self.outdoor == other.outdoor
            && self.nf <= other.nf
            && self.wdth <= other.wdth
            && self.srej <= other.srej
    }
}

/// A combination that has demonstrably heard lightning, and how often.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Golden {
    pub combo: Combo,
    /// Strikes heard at this exact combination.
    ///
    /// Kept because one strike is an anecdote — at `srej 0` it might be an
    /// electric hammer — and forty is a record. The fall-back rule asks for
    /// more than one before it will overrule a live tuner.
    pub strikes: u32,
}

/// How many strikes at a combination before it is trusted enough to return to.
///
/// **Two, not one.** `SREJ 0` admits man-made impulses, and this project has
/// logged 503 of them in a morning from an electric hammer; a single detection
/// is not evidence a setting can hear *lightning*. Two is still cheap during
/// any real storm, which produces them in minutes, and it discards the isolated
/// false positive that would otherwise pin the device to whatever it was set to
/// when a drill started up next door.
pub const TRUSTED_STRIKES: u32 = 2;

/// How long the tuner may sit deafer than a trusted combination before it is
/// pulled back, in minutes.
///
/// Long enough that an ordinary quiet spell does not fight the tuner, short
/// enough that a storm is not slept through. A storm that produces a strike
/// every three to five minutes gives this several chances.
pub const PATIENCE_MINUTES: u32 = 20;

/// Whether the tuner should be pulled back to a combination known to work.
///
/// The rule, in one sentence: **if the device is deafer than something that
/// provably heard lightning here, and it has heard nothing for a long time,
/// the deafness is the more likely explanation than the sky.**
///
/// That is the whole point of recording the combination. Without it, "heard
/// nothing for twenty minutes" is evidence of a quiet sky; with it, and with
/// the knowledge that this room *has* produced strikes at a more open setting,
/// it becomes evidence about the receiver instead.
///
/// Returns `None` when there is nothing to say — no record, not yet trusted, a
/// different gain, or the tuner already at least as open as the record.
pub fn fall_back_to(
    current: Combo,
    golden: Option<Golden>,
    quiet_minutes: u32,
) -> Option<Combo> {
    let golden = golden?;
    if golden.strikes < TRUSTED_STRIKES {
        return None;
    }
    // A record from the other gain describes a different front end.
    if golden.combo.outdoor != current.outdoor {
        return None;
    }
    // Already as open, or more so: nothing to go back to, and forcing the point
    // would *reduce* sensitivity.
    if current.at_least_as_open_as(golden.combo) {
        return None;
    }
    if quiet_minutes < PATIENCE_MINUTES {
        return None;
    }
    Some(golden.combo)
}

/// Fold a strike heard at `combo` into the record.
///
/// A strike at the same combination increments its count. A strike at a
/// different one **replaces** the record rather than competing with it: the
/// newest evidence describes the room as it is now, and a combination that
/// worked last month in different weather is not a better answer than one that
/// worked a minute ago. The count starting again at one is the honest reading —
/// the new combination has been proved once.
pub fn observe(golden: Option<Golden>, combo: Combo) -> Golden {
    match golden {
        Some(golden) if golden.combo == combo => Golden {
            combo,
            strikes: golden.strikes.saturating_add(1),
        },
        _ => Golden { combo, strikes: 1 },
    }
}
