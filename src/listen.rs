//! The event loop.
//!
//! **A loop skeleton calling named steps**, which is what the decomposition was
//! for. Each step owns its own state: `tuning` the noise decision, `screen` the
//! refresh policy, `fuel` the gauge, `effects` the console's hardware work.
//! What is left here is the sequencing and the batching -- the part that is
//! genuinely about *when* things happen rather than what they do.
//!
//! Nothing in here may panic. A lightning detector that dies at 3am and leaves
//! the last screen frozen on the glass is indistinguishable from one that is
//! working, which is the failure mode this whole project keeps circling.

use esp_idf_hal::delay::TickType;
use esp_idf_hal::gpio::PinDriver;
use esp_idf_hal::i2c::I2cDriver;
use esp_idf_hal::task::notification::Notification;

use crate::as3935::{As3935, Location};
use crate::session::{collect, report, toggle_location, Batch, Drawn, StormWatch, Totals};
use crate::{
    battery, boot, clock, console, defence, display, effects, history, log, power, screen,
    session, system, tuning,
};
use crate::{minute_now, now_ms, NOTIFY_BUTTON};



/// How long to collect events before summarising them (§4.2's "~1 s batch").
pub const BATCH_MS: u32 = 1000;




/// How often to read the reason register with no interrupt having asked for it.
///
/// **Plan B, and a diagnostic.** The register holds its reason until something
/// reads it, so an event whose `INT` edge was missed is still sitting there
/// afterwards. Thirty seconds is far slower than a storm but far faster than
/// never, and it costs one I2C transaction — against a device that already polls
/// a fuel gauge three times as often.
///
/// What makes it worth having is the asymmetry every measurement here has shown:
/// hundreds of `NoiseTooHigh`, which is a near-continuous condition, and never
/// once a disturber or a strike, which are impulsive and whose pulse is brief.
/// If polling starts finding events, the interrupt path is losing them — a
/// different defect from a sensor that hears nothing, and one that has been
/// indistinguishable from outside.
pub const IRQ_POLL_INTERVAL_S: u32 = 30;

/// How often to read the reason register when the interrupt path is failing.
///
/// **Two hundred milliseconds, because thirty seconds is a diagnostic and this
/// is a rescue.** Measured during the storm of 2026-08-19: one
/// `poll: found Disturber with no interrupt` at 18:06:04 and then no events at
/// all, while strikes were arriving every five to ten seconds a few kilometres
/// away. The interrupt is configured rising-edge, and the AS3935 holds `INT`
/// asserted until the reason register is read -- so a single missed edge means
/// there is never another edge, and the device goes permanently deaf while
/// every other sign of health stays perfect.
///
/// The thirty-second poll cannot rescue that: the register reports only the
/// *latest* reason, so at best one event in thirty seconds survives and the
/// rest are overwritten. At 200 ms the read is faster than the storm and the
/// line is released before the next event needs it.
///
/// It costs one I2C transaction per interval against a device that already
/// polls a fuel gauge, and it is the difference between a log full of strikes
/// and a log full of nothing.
pub const IRQ_RESCUE_INTERVAL_MS: u32 = 200;

/// How often the tuner re-runs a full sweep without being asked.
///
/// **Six hours.** A sweep takes about a quarter of an hour and deliberately
/// mis-sets the sensor while it searches, so it is roughly 4 % of the device's
/// time spent deaf -- affordable at this spacing and not at one hour, which was
/// the first proposal.
///
/// What it buys is that the tuning point stops being a thing somebody has to
/// remember to re-establish. The band this sits in changes: a new appliance
/// next door moves it, and a point learned last month is answering a question
/// about a room that no longer exists.
pub const AUTO_CALIBRATE_INTERVAL_S: u32 = 6 * 3600;

/// Probe length for a sweep nobody asked for.
///
/// Sixty seconds, the same as [`MEASURE_INTERVAL_S`] and for the same reason --
/// it is the span over which "did that window hear anything" is a fair question.
pub const AUTO_CALIBRATE_WINDOW_S: u32 = 60;


/// How long to ignore further button edges after one is accepted.
///
/// A tactile switch bounces for a few milliseconds; 300 ms also stops a
/// deliberate double-press from toggling twice and landing back where it
/// started, which would look like the button not working at all.
///
/// Measured against a real clock rather than decremented per batch, which was
/// the earlier version's mistake: the batch is 1000 ms, so a 300 ms blanking
/// counter was clear again by the next window and blanked nothing at all.
pub const BUTTON_DEBOUNCE_MS: u32 = 300;

pub fn listen(
    sensor: &As3935,
    i2c: &mut I2cDriver<'_>,
    irq: &mut PinDriver<'_, esp_idf_hal::gpio::Input>,
    button: &mut PinDriver<'_, esp_idf_hal::gpio::Input>,
    notification: &Notification,
    location: &mut Location,
    mut panel: Option<&mut display::Panel<'_>>,
    antenna_khz: u32,
    irq_confirmed: bool,
    gauge: Option<&battery::Max17048>,
    die_temperature: Option<&system::DieTemperature>,
    mut strike_log: Option<&mut log::Log>,
    start_point: defence::Point,
) {
    let mut last_irq_poll_ms: u32 = now_ms();
    // Starts now rather than at zero, so a board that reboots during a storm
    // does not sweep the moment the weather clears.
    let mut last_calibrate_ms: u32 = now_ms();
    let mut last_button_ms: u32 = 0;
    let mut batch = Batch::default();
    let mut batch_started = now_ms();

    // Running totals, and what the glass currently shows. The screen is redrawn
    // when the two disagree -- never on a timer alone.
    let mut totals = Totals::default();

    // §4.3's merge window, restored like the quiet threshold and for the same
    // reason: it describes the weather a room produces rather than a debugging
    // session, so it should survive a power cut. Nothing can be pending this
    // early, so the flush this returns is always empty.
    let merge_window_ms = crate::settings::merge_window_ms().unwrap_or(session::MERGE_WINDOW_MS);
    let _ = totals.merger.set_window_ms(merge_window_ms);
    match merge_window_ms {
        0 => println!("as:   strike merge off -- every return stroke is its own record"),
        ms => println!("as:   strike merge window {ms} ms"),
    }

    let mut history = history::History::new();

    // Rebuild the rings from the file (§5). The charts are RAM and die with a
    // power cut; the CSV does not -- so without this every reboot would show a
    // device that had never seen a storm.
    if strike_log.is_some() {
        let mut replayed = 0u32;
        log::for_each(|epoch, strike| {
            history.record((epoch / 60) as u32, &strike);
            replayed += 1;
        });
        if replayed > 0 {
            println!("log:  replayed {replayed} record(s) into the charts");
        }
    }
    let mut screen = screen::Screen::new();

    // **Show what there is, rather than an empty six hours.**
    //
    // The replay above works and always has, but the chart draws only the newest
    // buckets that fit -- 80 bars, which on the 5-minute ring is 6h40m. A device
    // rebooted a fortnight after its last storm came up with every ring full and
    // the visible window empty, which reads exactly like a device that had
    // forgotten everything. It was a display default, not lost data.
    //
    // `ui::CHART_BARS` is what `charts` actually draws, so this asks the same
    // question the screen does rather than a similar one.
    if let Some(scope) = history::period_with_data(&history, crate::ui::CHART_BARS) {
        let period = match scope {
            1 => crate::ui::ChartPeriod::Week,
            2 => crate::ui::ChartPeriod::Month,
            _ => crate::ui::ChartPeriod::Day,
        };
        if period != screen.period {
            println!(
                "log:  nothing in the last {} -- charting the {} instead",
                screen.period.label(),
                period.label()
            );
            screen.period = period;
        }
    }

    let mut fuel = battery::Fuel::new(gauge, i2c, now_ms());
    // §7's clock policy. Starts on the USB assumption -- the device is usually
    // plugged in, and being wrong that way costs power rather than a console.
    let mut policy = power::Policy::Awake;
    // `Some(mhz)` while `freq <mhz>` is in force. Deliberately not persisted:
    // it exists so a board can be watched over USB, and one that came back from
    let mut freq_override: Option<u32> = None;
    let mut console = console::Console::new();
    // Uptime at the last console input, and at the last clock save.
    let mut last_console_s: Option<u32> = None;
    let mut last_clock_save_s: u32 = 0;
    let mut last_log_sync_ms: u32 = 0;
    match power::apply(policy) {
        Ok(()) => match power::config() {
            Some((max, min, sleep)) => println!(
                "pm:   {} -- {}/{} MHz, light sleep {}",
                policy.label(),
                min,
                max,
                if sleep { "on" } else { "off" }
            ),
            None => println!("pm:   applied {} but could not read it back", policy.label()),
        },
        Err(e) => println!("pm:   could not apply {} -- {e}", policy.label()),
    }
    let mut tuning = tuning::Tuning::new(start_point, now_ms());
    let mut storm_watch = StormWatch::default();

    loop {
        // Re-arming is required after every trigger: esp-idf disables the
        // interrupt when it fires, so a loop that forgets this hears exactly one
        // event and then waits forever. Both sources need it.
        if let Err(e) = irq.enable_interrupt() {
            println!("irq:  could not re-arm GPIO21 -- {e}");
            return;
        }
        if let Err(e) = button.enable_interrupt() {
            println!("btn:  could not re-arm GPIO9 -- {e}");
            return;
        }

        // Wait only for what is left of the current window, so the batch closes
        // on time however many events arrive inside it.
        let elapsed = now_ms().saturating_sub(batch_started);
        let remaining = BATCH_MS.saturating_sub(elapsed).max(1);
        let woke = notification.wait(TickType::new_millis(remaining as u64).into());

        if let Some(source) = woke {
            let bits = source.get();

            if bits & NOTIFY_BUTTON != 0 {
                // **Confirm the pin is actually held down.** An edge alone is
                // not a press: GPIO9 is a strapping pin on a board sharing a
                // bench with a lightning sensor, and a glitch on it produced a
                // falling edge indistinguishable from a fingertip.
                //
                // That mattered more than a stray toggle would suggest. A
                // spurious press changes `location`, which forces a redraw, and
                // `user_acted` deliberately bypasses the 30 s floor -- so each
                // glitch bought an immediate 3.8 s refresh AND an NVS write.
                // The symptom was a screen repainting without pause and no logo
                // between repaints, which ruled out a reboot and left this.
                //
                // A real press is held for tens of milliseconds; a glitch is
                // over in microseconds. Sampling after a short settle tells them
                // apart with no state and no timer.
                let held = boot::button_held(button);
                let ready = now_ms().saturating_sub(last_button_ms) >= BUTTON_DEBOUNCE_MS;

                if held && ready {
                    last_button_ms = now_ms();
                    toggle_location(sensor, i2c, location);
                    // The point was learned at the gain that just went away --
                    // roughly a 4x change in what the front end sees, so it
                    // describes nothing now. Both ways of changing the mode do
                    // this; the console's copy is in `effects`.
                    tuning.gain_changed(sensor, i2c, "button");
                    // A deliberate press earns an immediate repaint -- see below.
                    screen.user_acted = true;
                } else if !held {
                    // Almost always a flashing tool asserting DTR, which the C3
                    // wires to this pin. Said out loud rather than swallowed,
                    // because "the button does nothing" and "the button is
                    // being pressed by your laptop" look identical otherwise.
                    println!(
                        "btn:  edge not held {} ms -- ignored (USB DTR?)",
                        boot::BUTTON_HOLD_MS
                    );
                }
            }

            // Any bit that is not the button is the sensor. Not
            // `& NOTIFY_STRIKE` on purpose: an unrecognised bit is far more
            // likely to be a bug in a notifier than a real third source, and
            // dropping it silently would lose a strike.
            if bits & !NOTIFY_BUTTON != 0 {
                collect(
                    sensor,
                    i2c,
                    &mut batch,
                    &mut totals,
                    &mut history,
                    minute_now(),
                    strike_log.as_deref_mut(),
                );
            }
        }

        // Not yet at the end of the window -- go back and keep listening.
        if now_ms().saturating_sub(batch_started) < BATCH_MS {
            continue;
        }

        // Plan B: look for events the interrupt line never announced. Before
        // `report`, so anything found is summarised in this batch rather than
        // the next one.
        if now_ms().saturating_sub(last_irq_poll_ms) >= IRQ_RESCUE_INTERVAL_MS {
            last_irq_poll_ms = now_ms();
            session::poll(
                sensor,
                i2c,
                &mut batch,
                &mut totals,
                &mut history,
                minute_now(),
                strike_log.as_deref_mut(),
            );
        }

        // **A strike earns the screen a short floor**, so the glass is not half
        // a minute behind the weather. Set before `report` consumes the batch;
        // cleared by the redraw that shows it.
        if batch.strikes > 0 {
            screen.strike_seen = true;
        }
        report(&batch);

        // Keep the rings' idea of "now" current even in a lull, so a chart drawn
        // during quiet weather shows the quiet rather than the last storm shoved
        // against its right edge.
        history.tick(minute_now());

        // --- the console ----------------------------------------------------
        //
        // Any input counts as "somebody is here", which is what holds the awake
        // policy -- see `power::decide`. It is the only supply signal that has
        // not lied, because it is about a person rather than a cable.
        if let Some(command) = console.poll() {
            last_console_s = Some(now_ms() / 1000);
            effects::handle(
                command,
                &mut effects::Hardware {
                    sensor,
                    i2c,
                    gauge,
                    die_temperature,
                    antenna_khz,
                    irq_confirmed,
                },
                &mut effects::Runtime {
                    location,
                    totals: &mut totals,
                    history: &mut history,
                    strike_log: strike_log.as_deref_mut(),
                    tuning: &mut tuning,
                    screen: &mut screen,
                    fuel: &mut fuel,
                    policy: &mut policy,
                    freq_override: &mut freq_override,
                    clock_saved_s: &mut last_clock_save_s,
                },
                now_ms(),
                minute_now(),
            );
        }

        // Flush the strike log on its own cadence. Buffered rather than synced
        // per strike so a storm producing events every few seconds does not
        // become a flash write every few seconds -- the trade being that a
        // power cut loses at most a minute. Safe on LittleFS specifically: an
        // unsynced write is lost, not corrupting.
        if now_ms().saturating_sub(last_log_sync_ms) >= log::SYNC_INTERVAL_MS {
            last_log_sync_ms = now_ms();
            if let Some(log) = strike_log.as_deref_mut() {
                match log.sync() {
                    Ok(0) => {}
                    Ok(n) => println!("log:  synced {n} record(s), {} total", log.len()),
                    Err(e) => println!("log:  sync FAILED -- {e}"),
                }
            }
        }

        // Re-save the clock periodically, so a power cut costs only the time the
        // device spent off rather than everything since it was last told.
        if let Some(epoch) = clock::now() {
            if now_ms() / 1000 - last_clock_save_s >= clock::SAVE_INTERVAL_S {
                last_clock_save_s = now_ms() / 1000;
                if let Err(e) = clock::save(epoch) {
                    println!("time: periodic save failed -- {e}");
                }
            }
        }

        // The fuel gauge, on its own slow cadence -- it is an I2C transaction
        // for values that move over hours -- and the clock policy that follows
        // from it (§7).
        if fuel.due(now_ms()) {
            fuel.poll(gauge, i2c, now_ms());

            // Skipped entirely while pinned -- otherwise the next tick would
            // quietly undo the override and the console would go away again,
            // which is the exact problem `freq` exists to solve.
            let want = power::decide(now_ms() / 1000, last_console_s);
            if freq_override.is_none() && want != policy {
                match power::apply(want) {
                    Ok(()) => {
                        policy = want;
                        match power::config() {
                            Some((max, min, sleep)) => println!(
                                "pm:   -> {} -- {}/{} MHz, light sleep {}",
                                policy.label(),
                                min,
                                max,
                                if sleep { "on" } else { "off" }
                            ),
                            None => println!("pm:   -> {} (read-back failed)", policy.label()),
                        }
                    }
                    Err(e) => println!("pm:   could not switch to {} -- {e}", want.label()),
                }
            }
        }

        // --- §4.2's noise auto-tune ----------------------------------------
        //
        // The asymmetry is the whole design: up by one per BATCH that heard
        // anything -- not per event, which saturates the ladder in a second and
        // is a counter racing the interrupt rate rather than tuning -- and down
        // by one only after a full minute of silence. Quick to defend, slow to
        // relax: a storm's first strike should not arrive into a receiver that
        // spent the afternoon relaxing toward a floor it will have to climb
        // straight back up.
        // Frozen while `sensitive on` is in force: the override sits below the
        // ladder's floor, so the first disturber would otherwise climb straight
        // off it and undo exactly what was asked for.
        // Everything this batch heard goes into the window. The window is both
        // the measurement and the decision -- the rate on screen used to come
        // from a separate 5-minute probe, so it read `0/min` while the ladder
        // was visibly climbing on events it had just counted.
        // --- §4.3's flash merge ---------------------------------------------
        //
        // Before the tuner and the screen, so a flash whose window has just
        // closed is counted this pass rather than next. The last stroke of a
        // storm has nothing behind it to push it out, so without this it would
        // wait in memory for a strike that might be a week away.
        session::flush_due(
            &mut totals,
            &mut history,
            strike_log.as_deref_mut(),
            now_ms(),
        );

        // Checked here rather than at commit time: the decision needs the bus,
        // and `commit_merged` deliberately has none.
        session::reset_if_stuck_overhead(sensor, i2c, &mut totals);

        tuning.observe(&batch);

        if tuning.due(now_ms()) {
            // --- §4.3's storm end -----------------------------------------
            //
            // Shares the tuner's window rather than keeping a clock of its
            // own, and goes first because `tuning.step` restarts the window it
            // has just judged. It reads the cumulative total, so it does not
            // care that the tuner is about to zero its own counters.
            //
            // One wrinkle worth knowing: a `calibrate` sweep sets its own probe
            // length, so during a sweep these windows are shorter than a minute
            // and the thirty of them are correspondingly shorter. A sweep only
            // runs on command and takes a quarter of an hour, so the effect is
            // to notice a storm ending slightly early, once, while someone is
            // watching.
            storm_watch.step(sensor, i2c, totals.strikes);
            tuning.step(sensor, i2c, &mut totals, now_ms());

            // **A sweep is the worst thing that can happen during a storm**, so
            // the clock alone does not get to start one. It mis-sets the sensor
            // on purpose for a quarter of an hour, which during weather is a
            // quarter of an hour of lightning nobody recorded.
            //
            // The gate is `StormWatch`'s own definition of a storm having
            // ended, not a second threshold invented here. If the interval
            // elapses while it is raining, `last_calibrate_ms` is deliberately
            // left alone so the sweep runs at the first quiet window afterwards
            // rather than waiting another six hours.
            if now_ms().saturating_sub(last_calibrate_ms) >= AUTO_CALIBRATE_INTERVAL_S * 1000
                && storm_watch.weather_quiet()
            {
                last_calibrate_ms = now_ms();
                println!("cal:  {AUTO_CALIBRATE_INTERVAL_S} s elapsed and the weather is quiet -- sweeping");
                tuning.begin_sweep(sensor, i2c, AUTO_CALIBRATE_WINDOW_S, u32::MAX, now_ms());
            }
        }

        // --- the screen ---------------------------------------------------
        //
        // The policy lives in `screen`; what stays here is the one side effect
        // that is not the screen's business -- widening the learned battery
        // range -- kept on this path because it must happen only when a redraw
        // actually does, which is what makes new extrema rare enough to persist.
        if let Some(panel) = panel.as_deref_mut() {
            let want = Drawn {
                strikes: totals.strikes,
                last_strike: totals.last_strike,
                location: *location,
                defence: tuning.point.raw() as u32,
            };

            if let Some(why) = screen.due(&want, now_ms()) {
                // On the redraw path on purpose: a new extreme is only worth a
                // flash write at the panel's cadence, not the gauge's.
                fuel.widen();

                screen.draw(
                    panel,
                    &screen::View {
                        location: *location,
                        point: tuning.point,
                        totals: &totals,
                        history: &history,
                        battery: fuel.reading,
                        trend: fuel.trend.as_ref(),
                        range: fuel.range,
                        drain: fuel.drain,
                        die_temperature,
                        log_bytes: strike_log
                            .as_deref()
                            .map(|l| (l.free_bytes(), l.used_bytes())),
                        antenna_khz,
                        irq_confirmed,
                    },
                    why,
                    now_ms(),
                );
                screen.mark_drawn(want, now_ms());
            }
        }


        batch = Batch::default();
        batch_started = now_ms();
    }
}

