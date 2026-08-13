//! What a wake loop iteration accumulates, and what the screen is redrawn for.
//!
//! Split out of `main` because these grew there rather than belonging there:
//! three small state types and the four functions that move them. `listen` now
//! reads as a loop skeleton calling named steps instead of defining them
//! inline.

use esp_idf_hal::delay::FreeRtos;
use esp_idf_hal::i2c::I2cDriver;

use crate::as3935::{As3935, Distance, Interrupt, Location, Strike};

use crate::{as3935, clock, defence, history, log, settings};

/// What one batch window heard.
#[derive(Default)]
pub struct Batch {
    pub strikes: u32,
    pub disturbers: u32,
    pub noise: u32,
    pub unknown: u32,
}

impl Batch {
    pub fn heard_interference(&self) -> bool {
        self.disturbers > 0 || self.noise > 0
    }

    /// **Uncalled since the ladder moved to a once-a-minute decision.** It
    /// answered "did this one-second batch hear nothing", which was the trigger
    /// for the old per-batch decay; the minute window asks the same question of
    /// a span long enough for the answer to mean something. Kept as the natural
    /// complement to `heard_interference`.
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.strikes == 0 && !self.heard_interference() && self.unknown == 0
    }
}


/// Everything counted since boot.
#[derive(Default)]
pub struct Totals {
    /// **Flashes, not strokes** — one per merge window (§4.3). `Batch::strikes`
    /// still counts every stroke, deliberately: it is the tuner's evidence that
    /// lightning is present, and more of it is better, where this is the number
    /// a person reads.
    pub strikes: u32,
    pub disturbers: u32,
    pub last_strike: Option<(Distance, u32, Option<u64>)>,
    /// Folds return strokes into flashes.
    ///
    /// **Housed here because `Totals` is already threaded to both callers of
    /// `record_strike`** — the real path and the simulator. Anywhere else and
    /// the merge would need a new parameter through four signatures, or would
    /// apply to only one of the two paths that must stay identical.
    pub merger: Merger,
    /// **Interference** per minute — noise *and* disturbers — from the last
    /// probe. The number the screen shows.
    ///
    /// Both, because the ladder responds to both ([`Batch::heard_interference`])
    /// and a rate that disagreed with the bar beside it is worse than no rate.
    /// Counting only `NoiseTooHigh` produced exactly that: once the floor
    /// climbed to 7 the noise stopped and disturbers took over at 9 a second, so
    /// the screen read a full bar next to `0/min` — "defending at maximum"
    /// beside "nothing to defend against", each true of a different thing.
    ///
    /// **A measurement, where the defence level beside it is a setting.** The
    /// ladder's level is a bad proxy for a jammed band in three ways: capped at
    /// 7, frozen entirely under `sensitive on`, and reading 0 in exactly the
    /// case that matters.
    ///
    /// ## Why it has to be probed rather than simply counted
    ///
    /// Counting `NoiseTooHigh` as it arrives does not work at the normal
    /// operating point, and the whole measurement turns on this. `WDTH 2`
    /// suppresses the noise *interrupts* without suppressing the noise, so a
    /// jammed band produces **zero** events. Measured on this board, every
    /// combination agreeing:
    ///
    /// | bus | supply | `WDTH` | events |
    /// |---|---|---|---|
    /// | 100 kHz | USB | **2** | 0, over 31 minutes |
    /// | 100 kHz | USB | 0 | 8/s |
    /// | 200 kHz | USB | 0 | 5–8/s |
    /// | 200 kHz | USB | **2** | **0** |
    /// | 200 kHz | battery | 0 | 0 |
    ///
    /// So the gate is opened deliberately for a moment, the events are counted,
    /// and it is closed again — see `NOISE_PROBE_MS` in `main`. That is the only
    /// way to ask "would this band be noisy if I were listening" without
    /// listening all the time.
    pub noise_per_min: u32,
    /// Disturbers in the same window. Same period as everything else on screen,
    /// so the two numbers can be read against each other.
    pub disturbers_per_min: u32,
    /// Events counted during the probe now running. Zeroed when one starts.
    pub probe_noise: u32,
}

pub const NOISE_JAMMED_PER_MIN: u32 = 60;


/// The subset of state the screen is redrawn for.
///
/// **Strikes and the mode button vote. Nothing else.** Everything else rides
/// the baseline
/// redraw, and that is a deliberate narrowing rather than an oversight:
///
/// * the **disturber count** moves every second in a noisy room;
/// * the **defence level** oscillates across a rung whenever the environment
///   sits near a threshold — measured here, 0→1→0 inside ninety seconds, which
///   at a 30 s floor is a 3.9 s refresh every half minute, forever;
/// * **battery, temperature and heap** move far slower than the baseline.
///
/// Each of those would pin the panel busy to report something nobody is waiting
/// on. A strike is the one event this device exists to announce, and it is the
/// one thing worth spending four seconds of panel time on the moment it
/// happens. Same conclusion the moisture project reached about its pack
/// voltage, arrived at from the opposite direction.
///
/// **The mode is here for the opposite reason.** It cannot churn — it changes
/// only when somebody deliberately presses the one button on the device — and
/// that press is exactly when a person is standing in front of the glass
/// waiting to see whether it worked. A setting that takes fifteen minutes to
/// appear reads as a button that does not work.
#[derive(PartialEq)]
pub struct Drawn {
    pub strikes: u32,
    pub last_strike: Option<(Distance, u32, Option<u64>)>,
    pub location: Location,
    /// §4.2's level. **A change test, so 11 -> 11 cannot repaint** — only a
    /// genuine move does, and the 30 s floor bounds how often that can happen.
    ///
    /// It was excluded for a while after it caused a refresh every half minute
    /// in a marginal room, oscillating 0 -> 1 -> 0. That was the ladder sitting
    /// exactly on a threshold, which is the one place it does not settle.
    /// Everywhere else it **converges**: it climbs until the noise is rejected
    /// and then stops, so the churn is a property of one environment rather
    /// than of the value.
    pub defence: u32,
}


/// Read one interrupt and fold it into the batch.
pub fn collect(
    sensor: &As3935,
    i2c: &mut I2cDriver<'_>,
    batch: &mut Batch,
    totals: &mut Totals,
    history: &mut history::History,
    minute: u32,
    strike_log: Option<&mut log::Log>,
) {
    // The datasheet's settle time, and the reason this is in the main task
    // rather than the ISR (§3).
    FreeRtos::delay_ms(as3935::IRQ_SETTLE_MS);
    read_and_handle(sensor, i2c, batch, totals, history, minute, strike_log, false);
}

/// Read the reason register without an edge having announced it (§3, plan B).
///
/// **A safety net for events whose interrupt never arrives.** Every measurement
/// on this board has shown the same asymmetry: hundreds of `NoiseTooHigh`, which
/// is effectively a continuous condition, and never once a disturber or a strike
/// — which are *impulsive*, and whose `INT` pulse is correspondingly brief. If
/// the edge is being missed rather than never generated, polling finds the event
/// sitting in `0x03` afterwards, because the register holds its reason until
/// something reads it.
///
/// So this is a diagnostic first and a fallback second. A poll that keeps
/// finding events proves the interrupt path is losing them, which is a different
/// defect from a sensor that hears nothing — and the two have been
/// indistinguishable from outside all week.
///
/// No settle delay: there is no edge to settle from, and whatever is in the
/// register has been there for however long it took this poll to come round.
pub fn poll(
    sensor: &As3935,
    i2c: &mut I2cDriver<'_>,
    batch: &mut Batch,
    totals: &mut Totals,
    history: &mut history::History,
    minute: u32,
    strike_log: Option<&mut log::Log>,
) {
    read_and_handle(sensor, i2c, batch, totals, history, minute, strike_log, true);
}

/// Read `0x03` and fold whatever it says into the batch.
///
/// Shared by the interrupt path and the poll so the two cannot drift: an event
/// found by polling must be counted, logged and rendered exactly as one
/// announced by an edge, or the fallback would quietly report a different
/// device from the one being debugged.
#[allow(clippy::too_many_arguments)]
fn read_and_handle(
    sensor: &As3935,
    i2c: &mut I2cDriver<'_>,
    batch: &mut Batch,
    totals: &mut Totals,
    history: &mut history::History,
    minute: u32,
    mut strike_log: Option<&mut log::Log>,
    polled: bool,
) {
    let reason = match sensor.interrupt_reason(i2c) {
        Ok(reason) => reason,
        Err(e) => {
            println!("irq:  reason read failed -- {e}");
            return;
        }
    };

    // Nothing pending is the overwhelmingly common answer to a poll, and saying
    // so once every thirty seconds would bury everything else.
    if polled && matches!(reason, Interrupt::Unknown(0)) {
        return;
    }
    if polled {
        println!("poll: found {reason:?} with no interrupt -- the edge was missed");
    }

    match reason {
        Interrupt::Lightning => {
            batch.strikes += 1;
            // Strikes are reported individually and immediately. They are the
            // point of the device, they are rare, and a summary line would hide
            // the distance and energy that §4.3 needs.
            match sensor.strike(i2c) {
                Ok(strike) => record_strike(
                    totals,
                    history,
                    strike_log.as_deref_mut(),
                    &strike,
                    clock::now(),
                    minute,
                    false,
                ),
                Err(e) => println!("STRIKE  -- but the detail read failed: {e}"),
            }
        }
        Interrupt::Disturber => {
            batch.disturbers += 1;
            totals.disturbers += 1;
            // Disturbers count toward the rate as well as noise -- see
            // `Totals::noise_per_min` for why the screen must not separate them.
            totals.probe_noise += 1;
        }
        Interrupt::NoiseTooHigh => {
            batch.noise += 1;
            // Into the probe accumulator too. Between probes this collects
            // events nobody asked for, which is harmless: it is zeroed at the
            // start of the next one.
            totals.probe_noise += 1;
        }
        Interrupt::Unknown(_) => batch.unknown += 1,
    }
}


/// §4.3's storm-end detection: the caller [`As3935::clear_statistics`] waited for.
///
/// The sensor's distance estimate is built from statistics over a storm, and it
/// has no idea when one ends. Left alone it carries the last storm's figures
/// into the next one, and this firmware never told it otherwise — so the
/// estimator has been accumulating since the board was first switched on.
///
/// **Counted in windows rather than milliseconds** so it shares the tuner's
/// clock. One window is `crate::tuning::MEASURE_INTERVAL_S`; the caller steps
/// this once per window, and the definition of "quiet" is simply that the
/// cumulative strike count did not move.
///
/// Thirty minutes because that is the rule everyone else already uses: the
/// public safety guidance is to wait thirty minutes after the last thunder
/// before calling a storm over. Borrowing it means the number is defensible
/// rather than invented, and a cell that goes quiet for half an hour and comes
/// back is a new storm by any reading.
pub struct StormWatch {
    /// Cumulative strikes as of the last window boundary.
    seen: u32,
    /// Consecutive windows in which `seen` did not move.
    quiet_windows: u32,
    /// Whether this lull has already been acted on.
    ///
    /// Without it a week of fine weather would re-clear every minute — harmless
    /// on the wire, but it would bury the console line that says the clear
    /// happened, and that line is the only evidence the mechanism ran.
    cleared: bool,
}

/// Quiet windows before the statistics are discarded. One window is a minute.
pub const STORM_END_QUIET_WINDOWS: u32 = 30;

impl Default for StormWatch {
    fn default() -> Self {
        Self {
            seen: 0,
            quiet_windows: 0,
            // Starts `true` so a board that boots into fine weather does not
            // announce a storm ending that never happened. A real strike clears
            // the flag, which is what arms the detector.
            cleared: true,
        }
    }
}

impl StormWatch {
    /// One window's worth of storm-end bookkeeping.
    ///
    /// Takes the cumulative total rather than a per-window count so it needs
    /// nothing from the tuner, whose own counters are reset inside its step.
    pub fn step(&mut self, sensor: &As3935, i2c: &mut I2cDriver<'_>, strikes_total: u32) {
        // A strike arrived: restart the lull and re-arm.
        if strikes_total > self.seen {
            self.seen = strikes_total;
            self.quiet_windows = 0;
            self.cleared = false;
            return;
        }

        // **The counter went backwards, which is not a strike.** Only `clear`
        // does that, and it means a log erase rather than weather. Resync and
        // let the current lull continue counting.
        //
        // Testing `!=` here instead treated an erase as strike activity and
        // restarted the thirty-minute countdown from it — observed once, and
        // harmless because clearing an estimator that holds nothing is a no-op.
        // Testing `>` alone would be the worse half of the same mistake: with
        // `seen` left at its pre-erase value, real strikes would fail the
        // comparison until they climbed back past it, and the detector would be
        // deaf for exactly as long as the storm it had already counted.
        if strikes_total < self.seen {
            self.seen = strikes_total;
        }

        if self.cleared {
            return;
        }

        self.quiet_windows += 1;
        if self.quiet_windows < STORM_END_QUIET_WINDOWS {
            return;
        }

        match sensor.clear_statistics(i2c) {
            Ok(()) => {
                self.cleared = true;
                println!(
                    "as:   {} min without a strike -- distance statistics cleared",
                    STORM_END_QUIET_WINDOWS
                );
            }
            // Deliberately not marking it cleared: a bus error here means the
            // chip still holds the old storm, so the next window should try
            // again rather than record a success that did not happen.
            Err(e) => println!("as:   could not clear distance statistics -- {e}"),
        }
    }
}

/// Default strike-merge window, in milliseconds.
///
/// One lightning *flash* is normally three to four *return strokes* down the
/// same channel, tens to hundreds of milliseconds apart, and the AS3935 reports
/// each one it validates. So a device counting interrupts counts strokes, and
/// the storm of 2026-08-12 read 930 where the sky produced perhaps a third of
/// that — the log and the audible flash rate disagreed by exactly that factor.
///
/// A second is comfortably longer than a flash and far shorter than the gap
/// between them: that storm's busiest minute held 15 strikes, so even at its
/// peak the mean spacing was four seconds.
pub const MERGE_WINDOW_MS: u32 = 1000;

/// Longest merge window `merge <ms>` accepts.
///
/// Ten seconds. Beyond that the window stops folding one flash together and
/// starts swallowing separate ones: the busiest minute of 2026-08-12 averaged a
/// strike every four seconds, so a window near this ceiling would already be
/// merging across genuinely distinct events during a storm overhead.
pub const MERGE_WINDOW_MAX_MS: u32 = 10_000;

/// One flash: the strokes that arrived inside a merge window, folded together.
pub struct Merged {
    pub strike: Strike,
    /// How many strokes went into it. Never zero.
    pub strokes: u32,
    /// The **first** stroke's clock, not the last. The flash began then, and a
    /// merge must not move an event later than it happened.
    pub epoch: Option<u64>,
    pub minute: u32,
    pub simulated: bool,
}

/// Folds return strokes into flashes (§4.3).
///
/// **The window runs from the first stroke, not the last.** A sliding window
/// would let a long enough train merge without limit — a storm directly
/// overhead could collapse into a single record hours long, which is the
/// opposite of what this is for.
pub struct Merger {
    window_ms: u32,
    pending: Option<Accumulator>,
}

struct Accumulator {
    started_ms: u32,
    energy_sum: u32,
    /// Kilometre readings only. Overhead and out-of-range are not distances and
    /// averaging them in as 1 and 63 is the bug `history::Bucket` documents.
    distance_km_sum: u32,
    distance_samples: u32,
    overhead: bool,
    strokes: u32,
    epoch: Option<u64>,
    minute: u32,
    simulated: bool,
}

impl Default for Merger {
    fn default() -> Self {
        Self {
            window_ms: MERGE_WINDOW_MS,
            pending: None,
        }
    }
}

impl Accumulator {
    fn fold(&mut self, strike: &Strike) {
        self.strokes += 1;
        self.energy_sum = self.energy_sum.saturating_add(strike.energy_raw);
        match strike.distance {
            Distance::Km(km) => {
                self.distance_km_sum = self.distance_km_sum.saturating_add(km as u32);
                self.distance_samples += 1;
            }
            Distance::Overhead => self.overhead = true,
            Distance::OutOfRange => {}
        }
    }

    /// The flash these strokes describe.
    ///
    /// Distance is the mean over *measured* kilometres. With none, overhead
    /// wins if any stroke reported it — it is the closest reading there is, and
    /// folding it into the mean as a number is what once made a storm read as
    /// permanently overhead.
    fn finish(self) -> Merged {
        let distance = match (self.distance_samples, self.overhead) {
            (0, true) => Distance::Overhead,
            (0, false) => Distance::OutOfRange,
            (samples, _) => Distance::Km((self.distance_km_sum / samples as u32) as u8),
        };
        Merged {
            strike: Strike {
                distance,
                energy_raw: self.energy_sum,
            },
            strokes: self.strokes,
            epoch: self.epoch,
            minute: self.minute,
            simulated: self.simulated,
        }
    }
}

impl Merger {
    pub fn window_ms(&self) -> u32 {
        self.window_ms
    }

    /// Change the window, flushing whatever is pending under the old one.
    ///
    /// Returned rather than dropped: those strokes were detected, and a setting
    /// change is no reason to lose them.
    pub fn set_window_ms(&mut self, window_ms: u32) -> Option<Merged> {
        let flushed = self.pending.take().map(Accumulator::finish);
        self.window_ms = window_ms;
        flushed
    }

    /// Fold one stroke in.
    ///
    /// Returns the *previous* flash when this stroke fell outside its window,
    /// because that flash is now complete and this stroke starts the next one.
    pub fn observe(
        &mut self,
        strike: &Strike,
        epoch: Option<u64>,
        minute: u32,
        simulated: bool,
        now_ms: u32,
    ) -> Option<Merged> {
        // A window of zero switches merging off entirely: every stroke is its
        // own flash, which is what the device did before 0.7.0 and what anyone
        // studying individual strokes wants back.
        if self.window_ms == 0 {
            return Some(
                Accumulator {
                    started_ms: now_ms,
                    energy_sum: 0,
                    distance_km_sum: 0,
                    distance_samples: 0,
                    overhead: false,
                    strokes: 0,
                    epoch,
                    minute,
                    simulated,
                }
                .tap_fold(strike),
            );
        }

        let expired = match self.pending.as_ref() {
            None => false,
            Some(pending) => now_ms.saturating_sub(pending.started_ms) >= self.window_ms,
        };

        let flushed = if expired {
            self.pending.take().map(Accumulator::finish)
        } else {
            None
        };

        match self.pending.as_mut() {
            Some(pending) => pending.fold(strike),
            None => {
                let mut fresh = Accumulator {
                    started_ms: now_ms,
                    energy_sum: 0,
                    distance_km_sum: 0,
                    distance_samples: 0,
                    overhead: false,
                    strokes: 0,
                    epoch,
                    minute,
                    simulated,
                };
                fresh.fold(strike);
                self.pending = Some(fresh);
            }
        }

        flushed
    }

    /// Release a flash whose window has closed.
    ///
    /// Called from the loop, because the last stroke of a storm has nothing
    /// after it to push it out — without this it would sit in memory until the
    /// next strike, which might be next week.
    pub fn take_due(&mut self, now_ms: u32) -> Option<Merged> {
        let expired = match self.pending.as_ref() {
            None => false,
            Some(pending) => now_ms.saturating_sub(pending.started_ms) >= self.window_ms,
        };
        match expired {
            true => self.pending.take().map(Accumulator::finish),
            false => None,
        }
    }
}

impl Accumulator {
    /// `fold` then `finish`, for the merging-disabled path.
    fn tap_fold(mut self, strike: &Strike) -> Merged {
        self.fold(strike);
        self.finish()
    }
}

/// Fold a stroke into the current flash, committing whichever flash completes.
///
/// **The merge sits here rather than at the call sites** because both the real
/// and the simulated paths already come through this function, and §4.3's whole
/// point is that they behave identically. A merge applied to only one of them
/// would make the simulator stop exercising the path it exists to exercise.
pub fn record_strike(
    totals: &mut Totals,
    history: &mut history::History,
    strike_log: Option<&mut log::Log>,
    strike: &Strike,
    epoch: Option<u64>,
    minute: u32,
    simulated: bool,
) {
    let flushed = totals
        .merger
        .observe(strike, epoch, minute, simulated, crate::now_ms());
    if let Some(merged) = flushed {
        commit_merged(totals, history, strike_log, &merged);
    }
}

/// Commit a flash whose merge window has closed, if there is one.
///
/// Driven from the loop: see [`Merger::take_due`] for why a stroke cannot be
/// left waiting for the next one to push it out.
pub fn flush_due(
    totals: &mut Totals,
    history: &mut history::History,
    strike_log: Option<&mut log::Log>,
    now_ms: u32,
) {
    let due = totals.merger.take_due(now_ms);
    if let Some(merged) = due {
        commit_merged(totals, history, strike_log, &merged);
    }
}

/// Record one flash: counters, rings, log, console — in that order.
///
/// **Shared by the real and simulated paths, which must stay behaviourally
/// identical.** They were written twice and did the same four things in the
/// same order; two copies of a sequence that has to match is two places for it
/// to stop matching, and the simulated path exists precisely so that the real
/// one can be trusted without a storm.
///
/// `simulated` marks the console line and one column of the record. It is
/// deliberately **not** allowed to change anything else: same counters, same
/// rings, same buffered write, same sync — a synthetic strike that took a
/// different route would not be exercising the path it exists to exercise.
///
/// The column was added after four records of unknown provenance made "has this
/// device ever seen real lightning?" unanswerable from its own log. A flag that
/// only reaches the console answers it for whoever was watching at the time,
/// which is the one reader who did not need telling.
pub fn commit_merged(
    totals: &mut Totals,
    history: &mut history::History,
    strike_log: Option<&mut log::Log>,
    merged: &Merged,
) {
    let strike = &merged.strike;
    totals.strikes += 1;
    totals.last_strike = Some((strike.distance, strike.intensity_milli(), merged.epoch));
    history.record(merged.minute, strike);

    // An unset clock still logs the strike, with 0 for the time. It happened;
    // what is unknown is when.
    if let Some(log) = strike_log {
        log.append(
            merged.epoch.unwrap_or(0),
            strike,
            merged.simulated,
            merged.strokes,
        );
    }

    let when = match merged.epoch {
        Some(epoch) => clock::format_local(epoch),
        None => heapless::String::try_from("(no clock)").unwrap_or_default(),
    };
    // Only when it merged something. A "1 stroke" on every line would be noise
    // on the one console this device has.
    let strokes = match merged.strokes {
        1 => String::new(),
        n => format!("  [{n} strokes]"),
    };
    println!(
        "STRIKE  {}  {:?}  energy {} (intensity {}.{:03}){}{}",
        when,
        strike.distance,
        strike.energy_raw,
        strike.intensity_milli() / 1000,
        strike.intensity_milli() % 1000,
        strokes,
        if merged.simulated { "  [SIMULATED]" } else { "" }
    );
}

/// One line per batch that heard interference. Silent otherwise — a quiet
/// device should be quiet, and the earlier per-event printing made the console
/// unreadable in a noisy room.
pub fn report(batch: &Batch) {
    if !batch.heard_interference() && batch.unknown == 0 {
        return;
    }
    println!(
        "batch: {} disturber(s), {} noise, {} unknown",
        batch.disturbers, batch.noise, batch.unknown
    );
}


/// Say where the defence point moved, in register terms.
///
/// The four fields every time rather than "the one that changed": with the
/// tuner walking the packed number by one, *which* field moved is a property of
/// where in the number the carry landed, and a line naming a single register
/// would be describing the least interesting half of the move.
pub fn tune(
    sensor: &As3935,
    i2c: &mut I2cDriver<'_>,
    point: defence::Point,
    direction: &str,
) {
    match apply(sensor, i2c, point) {
        Ok(()) => println!(
            "tune: {direction} to {}/{} ({}%) -- {}",
            point.raw(),
            defence::MAX,
            point.percent(),
            describe(point)
        ),
        Err(e) => println!("tune: could not move -- {e}"),
    }
}

/// The four fields of a point, for the console.
///
/// `min strikes` is spelled as the count the chip actually waits for rather than
/// the raw two bits, because "ms 1" and "wait for five strikes" are very
/// different things to read at 3am and only one of them is the truth.
pub fn describe(point: defence::Point) -> String {
    // Built from `defence::FIELDS` rather than written out, so the line follows
    // the layout: reordering the table reorders the console output instead of
    // leaving the labels attached to the wrong values.
    let mut out = String::new();
    for (index, field) in defence::FIELDS.iter().enumerate() {
        out.push_str(&format!("{} {} ", field.name, point.field(index)));
    }
    out.push_str(&format!("(wait {} strike(s))", point.min_strikes_count()));
    out
}

/// Switch indoor/outdoor, apply it to the sensor, and remember it.
///
/// The order matters: the sensor is told first, and NVS is written only if that
/// succeeded. The reverse would leave a device that reports one mode on screen
/// and runs the other -- and the stored value would survive the reboot that
/// would otherwise have fixed it.
pub fn toggle_location(sensor: &As3935, i2c: &mut I2cDriver<'_>, location: &mut Location) {
    let next = match location {
        Location::Indoor => Location::Outdoor,
        Location::Outdoor => Location::Indoor,
    };

    if let Err(e) = sensor.set_location(i2c, next) {
        println!("btn:  could not switch to {} -- {e}", next.label());
        return;
    }
    *location = next;

    match settings::store_location(next) {
        Ok(()) => println!("btn:  switched to {} (saved)", next.label()),
        // Worth saying out loud rather than swallowing: the device is running
        // the new mode but will forget it, which is the sort of thing that
        // wastes an afternoon the next time it is power-cycled.
        Err(e) => println!("btn:  switched to {} but NOT saved -- {e}", next.label()),
    }
}



/// Program the chip with a defence point.
///
/// Lives here rather than in `defence` so that module stays free of ESP-IDF —
/// which is what lets `tests/host` compile the real packing instead of a copy of
/// it. The register writes are the one part that cannot be host-tested anyway.
///
/// **All four fields, every time.** The tuner and the search both move the
/// packed number as a whole, and a carry changes fields that were not the
/// obvious target of the step.
pub fn apply(
    sensor: &As3935,
    i2c: &mut I2cDriver<'_>,
    point: defence::Point,
) -> Result<(), esp_idf_hal::sys::EspError> {
    sensor.set_noise_floor(i2c, point.noise_floor())?;
    sensor.set_watchdog_threshold(i2c, point.watchdog())?;
    sensor.set_spike_rejection(i2c, point.spike_rejection())?;
    // Takes a strike *count*, not the field, and reports what it actually
    // programmed -- the chip quantises to 1/5/9/16 and the return value is how
    // that is confirmed.
    // Always one strike -- see `defence::MIN_STRIKES_COUNT`. Written every
    // time rather than once at boot, so a chip that glitched or was reset
    // out from under us cannot quietly start waiting for sixteen.
    sensor.set_min_strikes(i2c, defence::MIN_STRIKES_COUNT)?;
    Ok(())
}

/// Every rejection knob at zero — the most receptive the part can be.
///
/// Expect disturbers. That is the point — this trades noise rejection for the
/// chance of hearing a strike the auto-tune was filtering out, and the caller
/// must freeze the tuner while it is on or the first disturber will immediately
/// climb back off it.
pub fn force_max_sensitivity(
    sensor: &As3935,
    i2c: &mut I2cDriver<'_>,
) -> Result<(), esp_idf_hal::sys::EspError> {
    apply(sensor, i2c, defence::Point::OPEN)
}


/// How long each calibration probe listens by default, in seconds.
///
/// The decision a probe makes is only "was that zero or not", and a jammed band
/// here produces 8 events a second — so even a short window carries a large
/// margin against *loud* interference. What it does not carry is a margin
/// against **sparse** interference: a source arriving once every twenty seconds
/// reads as silence in a five-second window, and the search then settles on a
/// setting that is quiet only because it did not listen long enough.
///
/// **Sixty seconds**, which is affordable for one reason: bisecting a 13-bit
/// number is **13 probes**, where the state machine this replaced took hundreds.
/// A full sweep is thirteen minutes, once, and the point then goes to NVS — so
/// the cost is paid at most once per room rather than continuously.
///
/// Measured on this board at 10 s, the sweep settled on `wd 7` after seeing 4
/// events at `wd 6`; a count that small is exactly where a short window is
/// deciding on the tail of a distribution rather than on a rate.
pub const CALIBRATE_PROBE_S: u32 = 60;

/// The range `calibrate <seconds>` accepts.
pub const CALIBRATE_PROBE_MIN_S: u32 = 5;
pub const CALIBRATE_PROBE_MAX_S: u32 = 60;

/// What counts as **quiet**, in events per minute, when the device has never
/// been told otherwise.
///
/// **Not zero**, first of all. A probe's question is "was that window quiet",
/// and testing it as `count == 0` makes the answer depend on how long you
/// listened: a 60 s window has six times a 10 s window's chances of catching one
/// stray event. Measured here, three sweeps of the same room settled at 448, 448
/// and 478 — differing only by probe length, with the long one buying spike
/// rejection *and* min strikes on one to two events a minute, while rejecting
/// other points at a hundred a minute by the same verdict.
///
/// A rate rather than a count, so the verdict means the same thing at 5 s and at
/// 60 s. That is the whole point: it makes a longer probe strictly better rather
/// than quietly deafer.
///
/// **Sixty, and it was twelve until 0.7.3.** Twelve a minute is one event every
/// five seconds, which no room with a refrigerator in it manages. Measured in
/// this one: 90–150 events/min with the air conditioning running, and 121–132
/// during the lulls of a live storm. Against a threshold of twelve, every one of
/// those windows reads as noisy, so the tuner climbs continuously and spends its
/// whole budget defending against the house rather than against the sky —
/// observed sitting at `nf 7 wd 15`, 53 % harm, hearing nothing.
///
/// Raising it to 120 in the same room let a sweep settle at `wd 6` where twelve
/// would have forced `wd 7` and high spike rejection on top; the walk then found
/// 20–23 % harm, and the strike-hold rule held it at 0 % right through the storm
/// of 2026-08-12. Sixty is the middle of that: one event a second, comfortably
/// above a quiet house and an order of magnitude below the ~480/min a genuinely
/// jammed band produces, which is what [`QUIET_PER_MIN_MAX`] guards.
///
/// This is only the value for a device that has never been calibrated. Anything
/// with a stored threshold keeps it — `calibrate <s> <per-min>` is the only way
/// to change one, deliberately, because it decides what the room is allowed to
/// sound like.
pub const QUIET_PER_MIN: u32 = 60;

/// The range `calibrate <seconds> <per-min>` accepts for the threshold.
///
/// The ceiling is a guard against being told that a continuously jammed band is
/// quiet: this board measures ~8 events/second when genuinely swamped, which is
/// 480 a minute.
pub const QUIET_PER_MIN_MAX: u32 = 240;

/// Settling time after programming a probe's registers, before its window opens.
///
/// `WDTH` and `NF_LEV` gate a running estimate the chip keeps of the band, not
/// an instantaneous comparison, so a window that opens the moment the register
/// lands spends its first moments measuring the *previous* setting. The same
/// 500 ms the reference waits after its own configuration block.
pub const CALIBRATE_SETTLE_MS: u32 = 500;

/// A calibration in progress: a bisection the main loop drives, one probe per
/// window.
///
/// **State, not a function.** An earlier version ran the whole sweep inside one
/// call, which meant it seized the main loop for fourteen minutes: the screen
/// held whatever was on it when the command arrived, the console stopped
/// answering, and the sweep needed its own event counter, its own settle and its
/// own progress screen — a second copy of machinery `listen` already had.
///
/// Driven from the loop instead, a probe is just an ordinary measurement window
/// whose verdict goes to the search rather than to the ±1 walk. The events are
/// counted by the same interrupt path, the rate lands in the same counters, and
/// the ordinary status screen shows the point under test on the ordinary gauge.
/// Nothing about a sweep needs its own anything.
pub struct Sweep {
    /// The lowest point not yet ruled out, and the highest still worth trying.
    /// They meet on the answer.
    pub low: u16,
    pub high: u16,
    /// How many probes have reported. For the console line only.
    pub probe: u32,
    /// Seconds per probe, which also sets the loop's window while a sweep runs.
    pub window_s: u32,
}

impl Sweep {
    pub fn new(window_s: u32) -> Sweep {
        Sweep {
            low: 0,
            high: defence::MAX,
            probe: 0,
            window_s: window_s.clamp(CALIBRATE_PROBE_MIN_S, CALIBRATE_PROBE_MAX_S),
        }
    }

    /// The point this probe is testing: the midpoint of what is left.
    pub fn point(&self) -> defence::Point {
        defence::Point::new(self.low + (self.high - self.low) / 2)
    }

    /// The answer, once `done`.
    pub fn settled(&self) -> defence::Point {
        defence::Point::new(self.low)
    }

    pub fn done(&self) -> bool {
        self.low >= self.high
    }

    /// Fold in the last window's verdict and narrow the scope.
    ///
    /// Quiet means nothing above this point needs trying; noisy means the answer
    /// is strictly above it. Excluding the midpoint on the noisy side is what
    /// guarantees progress when the two bounds are adjacent.
    ///
    /// Takes the **verdict**, not the count, so the rate threshold lives in one
    /// place and the search cannot disagree with the ±1 walk about what quiet
    /// means. See [`QUIET_PER_MIN`].
    pub fn record(&mut self, quiet: bool) {
        let mid = self.low + (self.high - self.low) / 2;
        self.probe += 1;
        if quiet {
            self.high = mid;
        } else {
            self.low = mid + 1;
        }
    }

    /// Roughly how many probes are left, for the console.
    pub fn remaining(&self) -> u32 {
        let mut span = (self.high - self.low) as u32;
        let mut probes = 0;
        while span > 0 {
            span /= 2;
            probes += 1;
        }
        probes
    }
}
