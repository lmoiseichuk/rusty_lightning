//! What a wake loop iteration accumulates, and what the screen is redrawn for.
//!
//! Split out of `main` because these grew there rather than belonging there:
//! three small state types and the four functions that move them. `listen` now
//! reads as a loop skeleton calling named steps instead of defining them
//! inline.

use esp_idf_hal::delay::FreeRtos;
use esp_idf_hal::i2c::I2cDriver;

use crate::merger::{Merged, Merger};
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
    /// Consecutive strikes reported at the nearest bin.
    ///
    /// A long run means one of two things and they are indistinguishable from
    /// the reading alone: the storm is overhead, or the estimator is stuck.
    /// See [`reset_if_stuck_overhead`].
    pub overhead_run: u32,
    /// Whether this run of nearest-bin readings has already been acted on.
    ///
    /// `false` by default, so a device that has only ever reported the nearest
    /// bin still gets one attempt — which is precisely the case that produced
    /// 917 consecutive `Overhead` readings and no way to tell why.
    pub overhead_reset_done: bool,
    /// When the last reset ran, in seconds since boot. Zero means never.
    pub overhead_reset_at_s: u32,
    /// Resets attempted since the last real distance reading.
    pub overhead_attempts: u32,
    /// Whether this device has stopped claiming a distance -- see
    /// [`OVERHEAD_ATTEMPTS_BEFORE_GIVING_UP`].
    pub overhead_gave_up: bool,
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
            // **And into the log, which is the point of this whole change.**
            // The device's central unanswered question is whether real flashes
            // are arriving here rather than as lightning. Counted, they say
            // nothing; timestamped, they can be laid against an independent
            // record of when flashes actually happened.
            log_event(strike_log.as_deref_mut(), log::Kind::Disturber);
        }
        Interrupt::NoiseTooHigh => {
            batch.noise += 1;
            // Into the probe accumulator too. Between probes this collects
            // events nobody asked for, which is harmless: it is zeroed at the
            // start of the next one.
            totals.probe_noise += 1;
            log_event(strike_log.as_deref_mut(), log::Kind::Noise);
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

impl StormWatch {
    /// Whether the weather has been quiet long enough to disturb the sensor.
    ///
    /// Named for the weather rather than `settled`, because `Sweep::settled`
    /// already means "the point the search finished on" and the two would read
    /// identically at a call site while meaning nothing alike.
    ///
    /// **The same test that decides a storm has ended**, deliberately reused
    /// rather than given its own threshold. Anything that wants to know "is it
    /// safe to stop listening properly for a while" is asking the question this
    /// already answers, and two definitions of "storm over" in one firmware
    /// would eventually disagree -- which is the defect this codebase keeps
    /// finding in its own history.
    pub fn weather_quiet(&self) -> bool {
        self.quiet_windows >= STORM_END_QUIET_WINDOWS
    }
}

/// Consecutive nearest-bin readings before the estimate is suspected of being
/// stuck rather than correct.
///
/// **Three, and a false trigger is free.** If the storm really is overhead the
/// estimator rebuilds from the next strikes and reports overhead again, which
/// costs nothing; if it was stuck, this is the only thing that unsticks it. The
/// asymmetry is what makes a small number safe.
pub const OVERHEAD_RUN_BEFORE_RESET: u32 = 3;

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



/// Longest merge window `merge <ms>` accepts.
///
/// Ten seconds. Beyond that the window stops folding one flash together and
/// starts swallowing separate ones: the busiest minute of 2026-08-12 averaged a
/// strike every four seconds, so a window near this ceiling would already be
/// merging across genuinely distinct events during a storm overhead.
pub const MERGE_WINDOW_MAX_MS: u32 = 10_000;


/// Log one non-lightning event, with the time it arrived.
///
/// **Volume is the objection, and it is real.** On 2026-08-13 this would have
/// written 103,000 rows in one storm. That is the cost of answering the
/// question, and it is bounded by the same retention the strike log already
/// has — where a strike log that cannot be cross-referenced is a file nobody
/// can draw a conclusion from at all.
fn log_event(strike_log: Option<&mut log::Log>, kind: log::Kind) {
    let Some(log) = strike_log else { return };
    log.append_event(
        clock::now().unwrap_or(0),
        crate::now_ms(),
        CURRENT_NF.load(core::sync::atomic::Ordering::Relaxed),
        kind,
    );
}

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

    // **Write down what was set when it worked.**
    //
    // This is the only moment the device gets unambiguous positive evidence
    // about its own settings. Quiet is two things wearing one face -- a working
    // receiver under a still sky, and a deaf one under a storm -- and the tuner
    // cannot tell them apart, which is why "minimise events" has deafness as
    // its optimum. A strike proves this combination could hear lightning from
    // this room, and that proof is what `golden::fall_back_to` later uses to
    // decide that silence is the receiver's fault rather than the sky's.
    //
    // **Simulated strikes are excluded.** A `strike` console command proves
    // nothing about the front end -- it never went through it -- and letting it
    // write the record would let a bench test pin the device to whatever it
    // happened to be set to.
    if !merged.simulated {
        remember_working_point();
    }

    // **A kilometre reading re-arms; the nearest bin counts toward a reset.**
    // `OutOfRange` re-arms too: it is a real answer about distance, so an
    // estimator producing it is demonstrably not stuck.
    match strike.distance {
        Distance::Overhead => totals.overhead_run += 1,
        Distance::Km(_) | Distance::OutOfRange => {
            // A real answer about distance: the estimator is demonstrably not
            // stuck, so every part of the stuck state goes, not just the latch.
            totals.overhead_run = 0;
            totals.overhead_reset_done = false;
            totals.overhead_attempts = 0;
            totals.overhead_reset_at_s = 0;
            totals.overhead_gave_up = false;
        }
    }
    totals.last_strike = Some((strike.distance, strike.intensity_milli(), merged.epoch));
    history.record(merged.minute, strike);
    // The table's row. Kept whole, where `record` above folds it into buckets.
    history.recent.push(history::Recent {
        epoch: merged.epoch,
        distance: strike.distance,
        energy_raw: strike.energy_raw,
        score_milli: history::score_milli(strike).unwrap_or(0),
        strokes: merged.strokes,
    });

    // An unset clock still logs the strike, with 0 for the time. It happened;
    // what is unknown is when.
    if let Some(log) = strike_log {
        // **The free-running millisecond counter, not a subdivision of the
        // epoch.** The epoch has one-second resolution and the anomaly being
        // measured is *inside* a second — deriving the sub-second part from a
        // second-resolution clock would produce exactly the flat zero that
        // hides it. This counter is monotonic since boot and independent of
        // whether the wall clock is set at all.
        log.append(
            merged.epoch.unwrap_or(0),
            crate::now_ms(),
            CURRENT_NF.load(core::sync::atomic::Ordering::Relaxed),
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
/// **Strikes are counted here as the interrupt lands**, before the merge window
/// and before anything can drop them, so this line is the only place that says
/// "the chip classified something as lightning" independently of whether a
/// record was ever written.
///
/// It was missing until 0.7.4, and its absence cost an overhead storm. The
/// symptom was 103,000 disturbers, one strike, and no way to tell from the
/// console whether the chip was rejecting the strikes at validation or whether
/// something downstream was losing them — two very different faults that looked
/// identical from outside. A count that only appears once a record is committed
/// cannot answer that; this one can.
pub fn report(batch: &Batch) {
    if batch.strikes == 0 && !batch.heard_interference() && batch.unknown == 0 {
        return;
    }
    println!(
        "batch: {} strike(s), {} disturber(s), {} noise, {} unknown",
        batch.strikes, batch.disturbers, batch.noise, batch.unknown
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
    // The two settings are printed beside the point deliberately. They are no
    // longer part of it, but they are still what the chip is holding -- and a
    // line that showed only the tunable third of the configuration would be the
    // "two different things to read at 3am" this function exists to prevent.
    let watchdog = settings::watchdog().unwrap_or(defence::WATCHDOG_DEFAULT);
    let spike = settings::spike_rejection().unwrap_or(defence::SPIKE_REJECTION_DEFAULT);
    out.push_str(&format!(
        "[wd {watchdog} sr {spike} fixed] (wait {} strike(s))",
        point.min_strikes_count()
    ));
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

    // Indoor gain is ~4x outdoor, so every energy figure already accumulated
    // was measured on a different scale and the distance estimate built from
    // them is now describing a receiver that no longer exists.
    restart_statistics(sensor, i2c, "gain changed");

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
/// which is what lets `tests/` compile the real packing instead of a copy of
/// it. The register writes are the one part that cannot be host-tested anyway.
///
/// **All four fields, every time.** The tuner and the search both move the
/// packed number as a whole, and a carry changes fields that were not the
/// obvious target of the step.
/// Discard the sensor's distance statistics, saying why.
///
/// **The statistics describe the instrument that gathered them.** The AS3935
/// estimates distance from the energies of recent strikes, so anything that
/// changes what those energies *mean* invalidates the entire accumulation. A
/// gain change rescales every one of them; a jump in the rejection point
/// changes which population of events is being averaged at all; and a
/// calibration sweep deliberately mis-sets the receiver eleven times in a row.
/// Carrying figures across any of those is averaging two different instruments.
///
/// **Deliberately not called from the ±1 walk.** One notch a minute barely
/// moves the receiver and does not move the storm, and clearing on every step
/// would mean the estimator never accumulated enough to say anything — which
/// is the failure this whole mechanism exists to prevent, arrived at from the
/// other direction.
/// Reset the distance statistics when the nearest bin keeps repeating.
///
/// The AS3935's nearest bin means "closer than 5 km", and a run of them is
/// ambiguous in a way no single reading can resolve: either the storm is
/// overhead, or the estimator has stopped tracking. Observed as 917 in a row
/// across a storm that approached, sat overhead and departed — a field that had
/// become a constant, which is the one thing a measurement must never be.
///
/// **Rate-limited, not latched — and that distinction was costing every retry.**
///
/// The fear this guard was built around is real: a plain counter would clear
/// every third strike of a genuinely overhead cell, the estimator would never
/// accumulate anything, and a freshly cleared estimator whose first readings
/// fall back to the nearest bin causes the very condition that triggers
/// clearing. That argument is sound. The implementation of it was not.
///
/// `overhead_reset_done` was cleared in exactly one place — the `Km |
/// OutOfRange` arm of the match above — so re-arming required a kilometre
/// reading. On a device whose complaint *is* that no kilometre reading ever
/// comes, that made this a **one-shot per boot**. Confirmed in
/// `storm-2026-08-13.csv`: it fired at 16:11:30, no kilometre reading followed,
/// and it never fired again across the remaining 21 strikes of that storm.
///
/// A minimum interval answers the runaway fear directly and without that
/// dependency: five minutes is far longer than the estimator needs to
/// accumulate, and far shorter than a storm. The kilometre reading still
/// re-arms immediately, because a working estimator should not be made to wait.
const OVERHEAD_RESET_MIN_INTERVAL_S: u32 = 300;

/// How many attempts one storm gets before the device stops claiming a distance.
///
/// **Four tries and then an honest "unknown".** A confidently wrong "overhead"
/// about a cell twenty kilometres away is worse than no distance at all, and
/// the log cannot tell the two apart afterwards.
const OVERHEAD_ATTEMPTS_BEFORE_GIVING_UP: u32 = 4;

pub fn reset_if_stuck_overhead(sensor: &As3935, i2c: &mut I2cDriver<'_>, totals: &mut Totals) {
    if totals.overhead_run < OVERHEAD_RUN_BEFORE_RESET {
        return;
    }
    let now_s = crate::now_ms() / 1000;
    // Wrap-correct, like every other interval here -- see `crate::uptime`.
    let since = crate::uptime::since(now_s, totals.overhead_reset_at_s);
    if totals.overhead_reset_at_s != 0 && since < OVERHEAD_RESET_MIN_INTERVAL_S {
        return;
    }
    totals.overhead_reset_at_s = now_s.max(1);
    totals.overhead_attempts = totals.overhead_attempts.saturating_add(1);
    if totals.overhead_attempts > OVERHEAD_ATTEMPTS_BEFORE_GIVING_UP {
        if !totals.overhead_gave_up {
            totals.overhead_gave_up = true;
            println!(
                "as:   {} resets and still nothing but the nearest bin -- reporting distance as UNKNOWN",
                OVERHEAD_ATTEMPTS_BEFORE_GIVING_UP
            );
            println!("as:   a confident \"overhead\" about a distant cell is worse than no distance");
        }
        return;
    }
    totals.overhead_reset_done = true;
    restart_statistics(
        sensor,
        i2c,
        "nearest bin repeated -- the estimate may be stuck rather than close",
    );
}

pub fn restart_statistics(sensor: &As3935, i2c: &mut I2cDriver<'_>, why: &str) {
    match sensor.clear_statistics(i2c) {
        Ok(()) => println!("as:   distance statistics cleared -- {why}"),
        Err(e) => println!("as:   could not clear distance statistics ({why}) -- {e}"),
    }
}

/// The noise floor the chip is holding right now.
///
/// **Written here because this is the only path that touches the register**, so
/// it cannot disagree with the hardware. Read by the log, which stamps every
/// event with the rung that was in force when it arrived.
///
/// That column is the whole point of the exercise: the rung is currently a
/// deterministic function of the noise, and the noise correlates with the
/// weather, so "strikes per storm-minute by rung" measures a confound rather
/// than the knob. Recording the rung with each event is what makes the curves
/// -- noise, disturbers and lost edges against NF_LEV -- computable at all.
/// Nobody has ever had them.
pub static CURRENT_NF: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);

pub fn apply(
    sensor: &As3935,
    i2c: &mut I2cDriver<'_>,
    point: defence::Point,
) -> Result<(), esp_idf_hal::sys::EspError> {
    sensor.set_noise_floor(i2c, point.noise_floor())?;
    CURRENT_NF.store(point.noise_floor(), core::sync::atomic::Ordering::Relaxed);
    // The watchdog comes from the *setting*, not the point -- see
    // `defence::WATCHDOG_DEFAULT`. Written here on every apply rather than once
    // at boot for the same reason as the strike count below: this is the one
    // path that touches the rejection registers, so it is the one place that
    // can guarantee what they hold.
    //
    // Reading NVS per apply is affordable: the tuner applies once a minute, and
    // a sweep once a probe.
    let watchdog = settings::watchdog().unwrap_or(defence::WATCHDOG_DEFAULT);
    sensor.set_watchdog_threshold(i2c, watchdog)?;
    // Takes a strike *count*, not the field, and reports what it actually
    // programmed -- the chip quantises to 1/5/9/16 and the return value is how
    // that is confirmed.
    // Always one strike -- see `defence::MIN_STRIKES_COUNT`. Written every
    // time rather than once at boot, so a chip that glitched or was reset
    // out from under us cannot quietly start waiting for sixteen.
    sensor.set_min_strikes(i2c, defence::MIN_STRIKES_COUNT)?;
    Ok(())
}

/// The noise floor at zero — the most receptive the *tuner's* space can be.
///
/// **Not every rejection knob**, which is what this said while the point held
/// four registers. `Point::OPEN` is `NF_LEV = 0` and nothing else; `WDTH`,
/// `SREJ` and `MIN_NUM_LIGH` left the packed point and are settings now, so
/// they keep whatever `srej`, `wdth` and NVS last put there. To open those too,
/// set them individually.
///
/// Expect disturbers. That is the point — this trades noise rejection for the
/// chance of hearing a strike the auto-tune was filtering out, and the caller
/// must freeze the tuner while it is on or the first disturber will immediately
/// climb back off it.
pub fn force_max_sensitivity(
    sensor: &As3935,
    i2c: &mut I2cDriver<'_>,
) -> Result<(), esp_idf_hal::sys::EspError> {
    restart_statistics(sensor, i2c, "sensitivity override");
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
/// **Sixty seconds**, which is affordable for one reason: the space is three
/// bits, so a sweep is **three probes**, where the state machine this replaced
/// took hundreds. A full sweep is three minutes, once, and the point then goes
/// to NVS — so the cost is paid at most once per room rather than continuously.
/// (This read "13 probes, thirteen minutes" while the point was 13 bits over
/// four registers; the argument is unchanged and the arithmetic is eight times
/// cheaper.)
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
///
/// **360, raised from 240 on 2026-08-19, because a storm is not a jammed band
/// and the old ceiling could not tell them apart.** A nearby strike throws
/// harmonics that arrive as disturbers -- the reason `Tuning::hold` exists -- and
/// that storm drove the rate to **264/min**, above the ceiling itself. So no
/// legal threshold tolerated real weather: the operator could only choose a
/// number that called a storm noisy, which is precisely when the tuner must not
/// climb. 360 sits above what the weather produced and still well below the 480
/// a swamped band gives.
pub const QUIET_PER_MIN_MAX: u32 = 360;

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


/// Fold the current settings into the known-good record.
///
/// Reads the registers' *stored* values rather than the chip's, because the
/// stored ones are what a reboot will restore -- a record of something the
/// device cannot get back to would be worse than none.
///
/// Written to NVS only when the record actually changes. A close storm makes
/// strikes faster than once a minute, and this is flash.
fn remember_working_point() {
    let combo = crate::golden::Combo {
        nf: CURRENT_NF.load(core::sync::atomic::Ordering::Relaxed),
        wdth: crate::settings::watchdog().unwrap_or(0),
        srej: crate::settings::spike_rejection().unwrap_or(0),
        outdoor: matches!(crate::settings::location(), Some(Location::Outdoor)),
    };

    let before = crate::settings::golden();
    let after = crate::golden::observe(before, combo);
    if before == Some(after) {
        return;
    }
    match crate::settings::store_golden(after) {
        Ok(()) if before.map(|b| b.combo) != Some(combo) => println!(
            "gold: heard lightning at nf {} wd {} sr {} ({}) -- remembered as the setting that works",
            combo.nf,
            combo.wdth,
            combo.srej,
            if combo.outdoor { "outdoor" } else { "indoor" },
        ),
        Ok(()) => println!("gold: {} strike(s) now heard at this setting", after.strikes),
        Err(e) => println!("gold: could not store the working point -- {e}"),
    }
}
