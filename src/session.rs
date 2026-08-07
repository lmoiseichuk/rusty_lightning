//! What a wake loop iteration accumulates, and what the screen is redrawn for.
//!
//! Split out of `main` because these grew there rather than belonging there:
//! three small state types and the four functions that move them. `listen` now
//! reads as a loop skeleton calling named steps instead of defining them
//! inline.

use esp_idf_hal::delay::FreeRtos;
use esp_idf_hal::i2c::I2cDriver;

use crate::as3935::{As3935, Distance, Interrupt, Location, Strike};

use crate::{as3935, clock, defence, display, history, log, settings, ui};

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
    pub strikes: u32,
    pub disturbers: u32,
    pub last_strike: Option<(Distance, u32, Option<u64>)>,
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


/// Record one strike: counters, rings, log, console — in that order.
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
pub fn record_strike(
    totals: &mut Totals,
    history: &mut history::History,
    strike_log: Option<&mut log::Log>,
    strike: &Strike,
    epoch: Option<u64>,
    minute: u32,
    simulated: bool,
) {
    totals.strikes += 1;
    totals.last_strike = Some((strike.distance, strike.intensity_milli(), epoch));
    history.record(minute, strike);

    // An unset clock still logs the strike, with 0 for the time. It happened;
    // what is unknown is when.
    if let Some(log) = strike_log {
        log.append(epoch.unwrap_or(0), strike, simulated);
    }

    let when = match epoch {
        Some(epoch) => clock::format_local(epoch),
        None => heapless::String::try_from("(no clock)").unwrap_or_default(),
    };
    println!(
        "STRIKE  {}  {:?}  energy {} (intensity {}.{:03}){}",
        when,
        strike.distance,
        strike.energy_raw,
        strike.intensity_milli() / 1000,
        strike.intensity_milli() % 1000,
        if simulated { "  [SIMULATED]" } else { "" }
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
    sensor.set_min_strikes(i2c, point.min_strikes_count())?;
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

/// Settling time after programming a probe's registers, before counting starts.
///
/// `WDTH` and `NF_LEV` gate a running estimate the chip keeps of the band, not
/// an instantaneous comparison, so a window that opens the moment the register
/// lands spends its first moments measuring the *previous* setting. The same
/// 500 ms the reference waits after its own configuration block.
pub const CALIBRATE_SETTLE_MS: u32 = 500;

/// Count interference over one probe window.
///
/// Waits on the same notification the main loop uses rather than polling, so an
/// event cannot be missed between reads — and applies the §3 settle before each
/// register read, exactly as `collect` does.
fn measure(
    sensor: &As3935,
    i2c: &mut I2cDriver<'_>,
    irq: &mut esp_idf_hal::gpio::PinDriver<'_, esp_idf_hal::gpio::Input>,
    notification: &esp_idf_hal::task::notification::Notification,
    window_ms: u32,
) -> u32 {
    // **Clear anything already pending before the window opens.**
    //
    // The AS3935 holds `INT` high until the reason register is read. If a probe
    // starts with an event unserviced the pin is *already* high — and a
    // rising-edge interrupt cannot fire without an edge. The window then times
    // out having heard nothing, never reads the register, and leaves the pin
    // high for the next probe too.
    //
    // That is not hypothetical: it made every probe of a sweep report 0 events
    // while the main loop was counting 7–8 a second at the very settings the
    // sweep called silent, so the search "found" full sensitivity every time.
    let _ = sensor.interrupt_reason(i2c);

    let deadline = crate::now_ms().saturating_add(window_ms);
    let mut events = 0u32;
    while crate::now_ms() < deadline {
        // Re-arm before every wait: esp-idf disables a GPIO interrupt when it
        // fires, so a loop that forgets goes deaf after one event. `listen` does
        // this once per iteration; this loop lives inside one iteration and has
        // to do it itself.
        if let Err(e) = irq.enable_interrupt() {
            println!("cal:  could not re-arm the IRQ -- {e}");
            return events;
        }

        // A short wait rather than the whole remaining window, so the level
        // check below runs regularly. **Both paths matter**: the notification
        // catches ordinary edges, and the level catches a pin that was already
        // high when we armed — which an edge-triggered interrupt cannot.
        let slice = 50u32.min(deadline.saturating_sub(crate::now_ms())).max(1);
        let ticks = esp_idf_hal::delay::TickType::new_millis(slice as u64).ticks();
        let notified = notification.wait(ticks).is_some();
        if !notified && !irq.is_high() {
            continue;
        }

        FreeRtos::delay_ms(as3935::IRQ_SETTLE_MS);
        match sensor.interrupt_reason(i2c) {
            Ok(Interrupt::NoiseTooHigh) | Ok(Interrupt::Disturber) => events += 1,
            // A strike during calibration is still a strike, but it is not
            // interference and must not push the search toward deafness.
            Ok(_) => {}
            Err(e) => println!("cal:  reason read failed -- {e}"),
        }
    }
    events
}
/// Search for the most receptive point that stays quiet.
///
/// **A plain binary search over the packed number, 0..=8191.** The whole
/// ordering argument lives in the bit layout (see `defence`): bisection resolves
/// the high bits first, so the first probe decides `NF_LEV` — the one knob that
/// cannot reject a strike — and the last probes decide `MIN_NUM_LIGH`, the one
/// that can silence a storm. Nothing here needs to know that; it just bisects.
///
/// The predicate is "did this point hear nothing", which is monotonic enough to
/// bisect but not perfectly so: a carry such as 1023 → 1024 raises the floor by
/// one while dropping the other three fields to zero. The search can straddle
/// such a boundary and settle one point to the deaf side of it. Re-running it,
/// or running it with a longer window, is the answer — not more machinery.
///
/// Returns the settled point, which the caller stores and adopts.
pub fn calibrate(
    sensor: &As3935,
    i2c: &mut I2cDriver<'_>,
    irq: &mut esp_idf_hal::gpio::PinDriver<'_, esp_idf_hal::gpio::Input>,
    notification: &esp_idf_hal::task::notification::Notification,
    window_s: u32,
    mut panel: Option<&mut display::Panel<'_>>,
) -> defence::Point {
    let window_s = window_s.clamp(CALIBRATE_PROBE_MIN_S, CALIBRATE_PROBE_MAX_S);
    let window_ms = window_s.saturating_mul(1000);
    // Ceiling on the probe count, for the estimate only -- the loop below is
    // what actually decides when it is done.
    let probes = defence::BITS + 1;
    println!(
        "cal:  starting -- 0..={}, {} s per probe, about {} probes ({} s)",
        defence::MAX,
        window_s,
        probes,
        probes * window_s
    );

    // The lowest point known to be quiet, and the highest known to be noisy.
    // `low` starts fully receptive because that is the answer if the band is
    // clean, and `high` at the ceiling because that is the fallback if it is
    // not.
    let mut low = 0u16;
    let mut high = defence::MAX;

    let mut probe = 0u32;
    let mut last_events = 0u32;

    while low < high {
        let mid = low + (high - low) / 2;
        let point = defence::Point::new(mid);
        probe += 1;

        if let Err(e) = apply(sensor, i2c, point) {
            println!("cal:  could not program the sensor -- {e}");
        }

        // **Draw, then probe.** The refresh is 3.8 s of high-current SPI beside
        // a sensor this project has already watched be jammed by the board's own
        // activity, so it happens between probes and never during one. It also
        // doubles as settling time: the registers are already programmed, so the
        // repaint is time the chip spends adjusting to them rather than time
        // taken from the measurement.
        if let Some(panel) = panel.as_deref_mut() {
            let fields = describe(point);
            let mut frame = display::Panel::frame();
            ui::calibrating(
                &mut frame,
                &ui::Calibration {
                    probe,
                    probes,
                    low,
                    high,
                    raw: point.raw(),
                    max: defence::MAX,
                    percent: point.percent(),
                    fields: &fields,
                    last_events,
                    window_s,
                },
            );
            if let Err(e) = panel.show(&frame) {
                println!("cal:  progress screen failed -- {e}");
            }
        }

        // Let the new thresholds take effect before counting against them.
        FreeRtos::delay_ms(CALIBRATE_SETTLE_MS);

        let count = measure(sensor, i2c, irq, notification, window_ms);
        last_events = count;
        println!(
            "cal:  [{}..{}] {} -> {} -- {} event(s)",
            low,
            high,
            point.raw(),
            describe(point),
            count
        );

        // Quiet here, so nothing above this needs trying; noisy, so the answer
        // is strictly above. Excluding `mid` on the noisy side is what
        // guarantees progress when `low` and `high` are adjacent.
        if count == 0 {
            high = mid;
        } else {
            low = mid + 1;
        }
    }

    let settled = defence::Point::new(low);
    if let Err(e) = apply(sensor, i2c, settled) {
        println!("cal:  could not program the settled point -- {e}");
    }
    println!(
        "cal:  settled at {}/{} ({}%) -- {}",
        settled.raw(),
        defence::MAX,
        settled.percent(),
        describe(settled)
    );
    settled
}
