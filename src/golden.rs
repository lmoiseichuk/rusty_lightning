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
//!
//! ## Not every detection is a strike worth learning from
//!
//! "A detected strike breaks that symmetry" was the premise, and the log
//! disproves it for one reading in particular: **`overhead`**, which has been
//! a hammer as often as it has been a storm. See [`is_evidence`] for the
//! measurements. Since 0.22.0 a detection the chip files in its nearest bin is
//! still logged, charted and shown, and it teaches the device nothing -- not
//! this record, not its trust, and none of the tuner's decisions that commit
//! it to something. It still holds the walk, which commits it to nothing: see
//! `verdict::Walk::Hold`.
//!
//! A record counted before that rule existed cannot be told apart from one
//! that was not, so it is set aside rather than trusted: see
//! `settings::legacy_golden`.

use crate::strike::Distance;

/// Whether a detection at `distance` is evidence that this receiver can hear
/// lightning -- enough to vouch for the setting it arrived at.
///
/// **The one test everything that learns from a strike goes through**: this
/// record and its trust (`session::commit_merged`), and the tuner's decisions
/// that commit it to something -- the dip that stays open, the dip a strike
/// makes unnecessary, the silence the rescue waits out, and, through this
/// record, the floor the walk will not relax below. The hold is the one
/// decision that does not ask: it only refuses to step, so it takes every
/// lightning interrupt (`verdict::Walk::Hold`).
///
/// **`Overhead` is not evidence, because the reading has been both things and
/// nothing in it says which.** It has been man-made:
///
/// * **2026-08-14, `srej 0`:** 503 "strikes" in three and a half hours from an
///   electric hammer on a neighbouring roof, every one graded `overhead`.
/// * **2026-09-16 to 09-24:** 9,542 logged strikes, 99 % of them `overhead`,
///   round the clock -- 3 a.m. included -- with 7-10 s between them. They fed
///   this record: "840 strikes heard here".
/// * **2026-11-01, afternoon:** three `overhead` matched electrical events --
///   a USB plug-in to the second, the end of a short run on the cell, one
///   mid-run -- and none matched thunder.
///
/// And it has been lightning:
///
/// * **2026-08-12:** 917 detections, every one in the nearest bin, while
///   thunder was audible every 10-30 s from 5-10 km (doc/specs.md §9).
/// * **2026-08-13:** 24 of 27 `overhead`, from a cell that was visibly
///   overhead.
/// * **2026-09-10:** a live storm, one audible strike every thirty seconds.
///   The two strikes that proved `wd 0` that day (2629a2d19aeb) are the log's
///   12:27:08 and 12:27:32 rows, 22 and 46 s after a boot, and both read
///   `overhead`; from 12:19 to 13:59 the log has 3 kilometre readings against
///   63 `overhead`.
///
/// **2026-11-01 outside on the cell settles neither way.** Thirteen detections
/// in eleven minutes, seven at 6-14 km and six `overhead`. Three of the six
/// came 6-9 s after another detection and three 44-66 s after, so they are not
/// one echo pattern; and the log cannot say which detections were the two
/// strikes LM heard. The six carried 98,724-463,994 raw against 12,842-150,818
/// for the seven, which is not a second observation: the chip estimates
/// distance from the stored energies of recent events (datasheet v1-04,
/// page 33), so a high energy and the nearest bin are one reading. Nor does a
/// detection with no storm statistics behind it read `overhead` by
/// construction -- the datasheet says nothing of the kind, and that morning's
/// first detection, 39.5 s after boot had cleared the statistics, read 14 km,
/// as 2026-08-13's first three did.
///
/// So a real cell overhead is recorded, charted, announced and held for, and
/// it cannot vouch for the setting it arrived at. What that costs is a storm
/// that reads only `overhead` writing no record, so no rescue can return to
/// it; what the other choice cost was September writing "840 strikes heard
/// here" and handing boot, the rescue and the floor to it.
///
/// **`OutOfRange` is evidence.** It is an answer about distance -- the chip
/// placed the waveform beyond its range -- and none of the false streams above
/// produced one: of the 9,650 strikes in the board's log on 2026-11-01, not
/// one reads `far`.
pub fn is_evidence(distance: Distance) -> bool {
    match distance {
        Distance::Overhead => false,
        Distance::Km(_) | Distance::OutOfRange => true,
    }
}

/// Whether a committed flash may be folded into the record.
///
/// **Judged on the distance the flash is recorded with**, which is the merged
/// one (`merger`), and that settles the mixed flash:
///
/// * **An `overhead` stroke merged with a kilometre stroke counts.** The merger
///   reports the kilometre mean whenever any stroke carried kilometres, so the
///   row reads as a distance -- and the kilometre stroke would have counted on
///   its own with merging off (`merge 0`). A merge window is a way of reading
///   the strokes, and it must not throw evidence away.
/// * **A flash whose only readings are `overhead` does not**, nor one that
///   paired `overhead` with `far`, which the merger also records as `overhead`.
///   Nearer than 5 km and beyond range inside one window contradict each
///   other, so that flash is the weakest evidence there is -- and leaving it
///   out keeps the log's `distance_km` column an exact record of what taught
///   the device, which a reader can reconstruct from the CSV alone.
///
/// A simulated strike never counts: it did not go through the front end.
pub fn may_record(simulated: bool, distance: Distance) -> bool {
    !simulated && is_evidence(distance)
}

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

/// Whether the tuner should be pulled back toward a combination known to work,
/// and to which noise floor.
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
/// **The noise floor is the only knob the rescue moves, so it is the only one
/// it compares.** `wdth` and `srej` are a person's settings -- `wdth <n>` and
/// `srej <n>` write them, and boot reconciles them with a trusted record
/// (`defence::spike_rejection_at_boot`) -- and the rescue does not overrule a
/// person in the middle of a session. Until 0.22.1 it compared all three and
/// moved one, which failed two ways:
///
/// * **It froze the walk.** With the chip at `sr 2` and a record at `sr 1` --
///   one `srej 2` typed after a trusted record, which 0.22.0's boot warning
///   invites -- or a stored watchdog above the record's, no move of `nf` could
///   make the device "as open as the record". So once the patience ran out it
///   fired every window: re-applied the same rung, restarted the window and
///   returned before the walk, so the hold, the climb, the dip, stuck detection
///   and the save all stopped until a located strike or a restart, even if the
///   rung began to drown.
/// * **It could deafen.** With the noise floor already more open than the
///   record's and `srej` higher, it "returned" the point *up* to the record's
///   rung.
///
/// Returns `None` when there is nothing to say -- no record, not yet trusted, a
/// different gain, a noise floor already at the record's or more open, or not
/// yet quiet for long enough. Otherwise the record's `nf`.
pub fn fall_back_to(
    current: Combo,
    golden: Option<Golden>,
    quiet_minutes: u32,
) -> Option<u8> {
    let golden = golden?;
    if golden.strikes < TRUSTED_STRIKES {
        return None;
    }
    // A record from the other gain describes a different front end.
    if golden.combo.outdoor != current.outdoor {
        return None;
    }
    // At the record's rung, or more open: nothing to go back to, and forcing
    // the point would *reduce* sensitivity. Lower `nf` looks at more.
    if current.nf <= golden.combo.nf {
        return None;
    }
    if quiet_minutes < PATIENCE_MINUTES {
        return None;
    }
    Some(golden.combo.nf)
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


// --- the other end of the ladder --------------------------------------------

/// The lowest rung the tuner may relax to, learned from being swamped.
///
/// **Symmetric to [`Golden`], and for the same reason.** `Golden` records a rung
/// measured to *hear*; this records one measured to *drown*. Both exist because
/// the walk otherwise treats every window as fresh evidence and forgets what it
/// has already established about this room.
///
/// The failure it prevents was observed directly. A sweep probed `nf 0`,
/// measured **595 events/min**, correctly graded it noisy and settled at `nf 1`
/// — and then the ordinary walk relaxed into `nf 0` on the next quiet window,
/// drowned, and the proportional climb (`noise_per_min / quiet_per_min`) took
/// 595/60 ≈ 9 notches at once, saturating at `nf 7`: fully deaf. From there it
/// walks back one notch per window, so a single excursion into the swamp costs
/// seven windows of maximum deafness, and it repeats.
///
/// One rung of memory ends that cycle.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Floor {
    /// The highest rung measured to be swamped, if any.
    ///
    /// The ladder is monotonic — a lower `nf` looks at more — so "swamped at
    /// this rung" implies swamped at every rung below it, and only the highest
    /// needs keeping.
    pub swamped_at: Option<u8>,
    /// The most open rung this room has been measured to hear *lightning* at.
    ///
    /// The mirror of `swamped_at`, and it earns its place for the same reason:
    /// a quiet window is not evidence that a more open rung has become
    /// habitable. Drowning says a rung is too open; a strike says a rung
    /// works. Between them the walk is bracketed by measurement rather than by
    /// the absence of weather -- which is the whole trap this device is about,
    /// because an auto-tune that minimises noise has deafness as its optimum.
    ///
    /// Kept as the *most open* rung that worked, so later strikes at a deafer
    /// setting do not ratchet the floor upward.
    pub heard_at: Option<u8>,
}

impl Floor {
    /// Fold in a window that drowned at `nf`.
    pub fn swamped(self, nf: u8) -> Floor {
        Floor {
            swamped_at: Some(match self.swamped_at {
                Some(previous) => previous.max(nf),
                None => nf,
            }),
            ..self
        }
    }

    /// Fold in a rung that produced lightning.
    pub fn heard(self, nf: u8) -> Floor {
        Floor {
            heard_at: Some(match self.heard_at {
                Some(previous) => previous.min(nf),
                None => nf,
            }),
            ..self
        }
    }

    /// The lowest rung the walk may relax to.
    ///
    /// One above the highest rung known to drown, and `0` when nothing is known
    /// — an unlearned device is not restricted, because a floor invented
    /// without evidence is just a deafer default.
    pub fn lowest(self) -> u8 {
        let drowned = match self.swamped_at {
            Some(nf) => nf.saturating_add(1).min(MAX_NF),
            None => 0,
        };
        match self.heard_at {
            Some(heard) => drowned.max(heard),
            None => drowned,
        }
    }

    /// Whether relaxing from `nf` is allowed.
    pub fn may_relax_from(self, nf: u8) -> bool {
        nf > self.lowest()
    }

    /// Forget it.
    ///
    /// **The room changes.** A floor learned beside a running fridge is wrong
    /// once it stops, and nothing in a noise measurement says which it was. So
    /// a gain change clears it — that is a different front end — and so does an
    /// explicit recalibration, which is the operator saying the room is not
    /// what it was.
    pub fn forget() -> Floor {
        Floor {
            swamped_at: None,
            heard_at: None,
        }
    }
}

/// The top of the `NF_LEV` ladder, mirrored here so this module needs nothing
/// from `defence` and stays host-testable on its own.
pub const MAX_NF: u8 = 7;

/// How far above the quiet threshold a window must be to count as *swamped*
/// rather than merely noisy.
///
/// **Not simply "not quiet".** An ordinary noisy window is what the ±1 walk is
/// for, and treating every one as a permanent fact about the room would ratchet
/// the floor upward until the device was deaf by accumulation — the exact
/// failure the tuner already has, with a longer memory. Drowning is a different
/// state: the measured figures are 595/min against a 60/min threshold, roughly
/// ten times, where an ordinary noisy window sits at one or two times.
pub const SWAMPED_MULTIPLE: u32 = 5;

/// Whether a window's **noise** rate means the front end is drowning.
///
/// **Noise, not noise plus disturbers, and the difference is the whole test.**
/// Drowning is the chip reporting `NoiseTooHigh` continuously; a disturber is
/// the chip having looked at something and rejected it, which is a receiver
/// doing its job. On this device the working rung sits at 0 noise and 5-8
/// disturbers a second -- around 360/min -- so a test that summed them would
/// grade the working rung as swamped and ratchet the floor up under it until
/// the device was deaf. The two signals point in opposite directions and must
/// not be added.
pub fn is_swamped(noise_per_min: u32, quiet_per_min: u32) -> bool {
    noise_per_min >= quiet_per_min.max(1).saturating_mul(SWAMPED_MULTIPLE)
}
