//! lightning project, Copyright (c) 2026 Leonid Moiseichuk under MIT License
//!
//! What the console commands actually do (§5).
//!
//! Split from `main` because it was the largest single thing in the wake loop —
//! a two-hundred-line `match` inside a function that also owns interrupts, the
//! sensor, the screen and the power policy. It is self-contained: nothing here
//! runs on a timer or an interrupt, only in response to a typed line.
//!
//! Everything it touches is borrowed through [`Ctx`] rather than passed as
//! fifteen arguments, which is also what keeps the signature honest about how
//! much state a console command can reach.

use crate::as3935::{Distance, Location, Strike};
use crate::session;
use crate::console::{ApRequest, Command};
use crate::{battery, clock, history, log, system, ui};

/// Everything a command may read or change.
///
/// A borrow struct rather than a long argument list: the set is large because
/// the console is deliberately a window onto the whole device, and naming it
/// once makes that visible instead of spreading it across a signature.
/// Note what is **absent**: the sensor and the I2C bus. No console command
/// talks to hardware — they read state the loop already gathered, or change a
/// setting the loop applies. Leaving those borrows out makes that a property of
/// the type rather than a convention.
pub struct Ctx<'a> {
    pub location: &'a mut Location,
    pub totals: &'a mut session::Totals,
    pub history: &'a mut history::History,
    pub strike_log: Option<&'a mut log::Log>,
    pub chart_period: &'a mut ui::ChartPeriod,
    pub reading: Option<battery::Reading>,
    pub range: (u16, u16),
    /// How hard the tuning state machine is defending, 0–100 (§4.2).
    pub level: u32,
    pub die_temperature: Option<&'a system::DieTemperature>,
    pub antenna_khz: u32,
    pub irq_confirmed: bool,
    pub minute: u32,
    pub uptime_minutes: u32,
    /// Whether the §4.2 ladder is currently overridden and frozen.
    pub max_sensitivity: bool,
    /// Whether the panel is powered down between refreshes.
    pub panel_sleep: bool,
    /// Whether `awake on` is in force, for `status` and `health`.
    pub awake_lock: bool,
}

/// What the caller must do as a result.
///
/// Returned rather than done here, because these are the two effects that
/// belong to the loop rather than to the command: when the clock was last
/// saved, and whether the screen should repaint now instead of waiting for the
/// baseline.
#[derive(Default)]
pub struct Effects {
    pub clock_saved: bool,
    pub redraw_now: bool,
    /// `Some(seconds)` when `calibrate` was used, carrying how long each probe
    /// should listen. Applied by the caller for the same reason `sensitivity`
    /// is: the sweep needs the sensor, the bus and the IRQ notification, and
    /// `Ctx` deliberately has none of them.
    /// `Some((seconds, quiet_per_min))` when `calibrate` was used. `u32::MAX`
    /// for the threshold means "leave the stored one alone".
    pub calibrate: Option<(u32, u32)>,
    /// What `ap` asked for, if anything.
    ///
    /// Applied by the caller for the same reason as the two below: the listen
    /// loop owns the `Portal`, and a second place that can create a radio is a
    /// second place that can leave one running.
    pub access_point: Option<ApRequest>,
    /// `Some(raw)` when `defence <raw>` was used. Applied by the caller, which
    /// is the only place that owns the point and the bus.
    pub set_point: Option<u16>,
    /// Set by a bare `defence`, which only wants to be told where it is.
    pub show_point: bool,
    /// `Some(on)` when `sensitive` was used. Applied by the caller rather than
    /// here, because `Ctx` deliberately has no sensor or bus — every other
    /// console command is pure, and one hardware command should not change that
    /// for all of them.
    pub sensitivity: Option<bool>,
    /// `Some(level)` when `srej` was used; the caller owns the bus.
    pub set_spike_rejection: Option<u8>,
    /// Set by a bare `srej`, which only wants to be told.
    pub show_spike_rejection: bool,
    /// `Some(level)` when `wdth` was used; the caller owns the bus.
    pub set_watchdog: Option<u8>,
    /// Set by a bare `wdth`, which only wants to be told.
    pub show_watchdog: bool,
    /// Set by `clearstats`; the caller owns the bus, same as `regs`.
    pub clear_statistics: bool,
    /// Set by `regs`; the caller owns the bus. Same reason as `sensitivity`.
    pub dump_registers: bool,
    /// `Some(indoor)` when `mode` was used.
    pub set_indoor: Option<bool>,
    /// `mask on` stops disturbers raising the IRQ; `mask off` lets them back.
    ///
    /// **On by default since 2026-09-10** -- see `boot::configure` for the
    /// storm measurement that moved it. The auto-tune is unaffected either way,
    /// since it folds noise and not disturbers.
    ///
    /// `mask off` is therefore the interesting direction: it restores the
    /// disturber log, which is the only evidence about whether real flashes
    /// reach this board classified as not-lightning. It costs strikes in a close
    /// storm -- a disturber can replace a pending strike's reason before it is
    /// read -- and on the cell a wake per disturber, so it is an experiment and
    /// not a resting state.
    pub mask_disturbers: Option<bool>,
    /// `Some(on)` when `panel sleep on|off` was used. Applied by the caller,
    /// which owns the screen the setting lives on.
    pub panel_sleep: Option<bool>,
    /// Set by `freq`. Handled by the caller, which owns the policy loop — doing
    /// it here would be undone on the next tick.
    pub freq: Option<crate::console::FreqRequest>,
    /// Set by `sleep on|off`. Same reason as `freq`.
    pub light_sleep: Option<bool>,
    /// `Some(on)` when `awake on|off` was used. Same reason as `freq`: the
    /// caller owns the policy, the override and the lock itself.
    pub set_awake_lock: Option<bool>,
    /// Set by a bare `awake`, which reports what the chip is enforcing as
    /// well -- and only the caller knows whether `freq` is pinning it.
    pub show_awake_lock: bool,
    /// Set by `battery`; the raw read needs the bus, which `Ctx` does not have.
    pub read_battery: bool,
}

/// A short heapless string, for the "not set" cases.
///
/// One helper rather than the same `try_from(..).unwrap_or_default()` written
/// out at four call sites.
fn fallback(text: &str) -> heapless::String<20> {
    heapless::String::try_from(text).unwrap_or_default()
}

/// Run one command.
pub fn run(command: Command, ctx: &mut Ctx<'_>) -> Effects {
    let mut effects = Effects::default();
    match command {
        Command::Date(None) => match clock::now() {
            Some(epoch) => println!(
                "date: {} {}",
                clock::format_local(epoch),
                clock::tz_label()
            ),
            None => println!("date: not set -- use: date <unix-epoch>"),
        },
        Command::Date(Some(epoch)) => match clock::set(epoch) {
            Ok(()) => {
                effects.clock_saved = true;
                println!(
                    "date: {} {}",
                    clock::format_local(epoch),
                    clock::tz_label()
                );
            }
            Err(e) => println!("date: could not set -- {e}"),
        },
        Command::SetTz(minutes) => match clock::set_tz_minutes(minutes) {
            Ok(()) => println!(
                "time: local offset {} -- now {}",
                clock::tz_label(),
                clock::now()
                    .map(clock::format_local)
                    .unwrap_or_else(|| fallback("(clock not set)"))
            ),
            Err(e) => println!("time: could not set offset -- {e}"),
        },

        // A synthetic strike, straight into the same path a real one
        // takes. **This exists because a real one cannot be provoked.**
        // The AS3935 validates a waveform against a lightning signature
        // before classifying it, so a spark -- a piezo lighter, a relay
        // -- raises a *disturber*, never a strike. Confirmed here: a
        // lighter moved the disturber count and produced no strike, and
        // that is the chip working correctly rather than failing.
        //
        // So everything downstream of `Interrupt::Lightning` -- the
        // distance decode, the score, the rings, the CSV line -- would
        // otherwise be unexercised until real weather arrives.
        Command::Simulate(km, intensity_milli) => {
            let strike = Strike {
                distance: Distance::Km(km),
                // Invert `intensity_milli` so the synthetic strike carries a
                // plausible raw energy rather than a magic number -- the same
                // arithmetic a real one would have, including its ceiling.
                energy_raw: crate::strike::energy_for_intensity(intensity_milli),
            };
            session::record_strike(
                ctx.totals,
                ctx.history,
                ctx.strike_log.as_deref_mut(),
                &strike,
                clock::now(),
                ctx.minute,
                true,
            );
        }

        Command::Clear => match ctx.strike_log.as_deref_mut() {
            // No confirmation prompt. The console is non-blocking and
            // line-based, so a prompt would mean holding state across
            // polls for a command whose damage is bounded and whose
            // main use is exactly this: wiping test data.
            Some(log) => match log.clear() {
                Ok(()) => {
                    // The charts go with it. They are rebuilt from this
                    // file at boot, so leaving them populated after
                    // erasing it means the screen and the log disagree
                    // until the next power cycle -- and the screen is
                    // the one nobody thinks to doubt. Caught in
                    // testing: `clear` then `status` reported eleven
                    // strikes against four records.
                    *ctx.history = history::History::new();
                    *ctx.totals = session::Totals::default();
                    
                    effects.redraw_now = true;
                    println!("log:  cleared -- charts and counters reset too");
                }
                Err(e) => println!("log:  could not clear -- {e}"),
            },
            None => println!("log:  no log to clear"),
        },

        Command::Scope(which) => {
            *ctx.chart_period = match which {
                1 => ui::ChartPeriod::Week,
                2 => ui::ChartPeriod::Month,
                _ => ui::ChartPeriod::Day,
            };
            // Force the next redraw: the period is not in the change
            // test -- it is set by a person, like the mode button, so
            // it cannot churn and should not wait for the baseline.
            effects.redraw_now = true;
            println!("scope: showing the last {}", ctx.chart_period.label());
        }

        // The screen's top line, in text. Same figures, same order, so
        // a reading taken over the console and a photograph of the
        // panel can be compared without translating between them.
        Command::Health => {
            let health = system::health(
                ctx.die_temperature,
                ctx.strike_log.as_deref().map(|l| (l.free_bytes(), l.used_bytes())),
            );
            match clock::now() {
                Some(epoch) => println!(
                    "health: {} {}",
                    clock::format_local(epoch),
                    clock::tz_label()
                ),
                None => println!("health: clock not set, up {} min", ctx.uptime_minutes),
            }
            // The restart that started this session and the ones before it --
            // the boot banner says the same, but the banner is gone by the time
            // anybody asks, and the earlier entries are the ones nobody saw.
            println!(
                "health: fw {} {}, up {} min after {}",
                env!("CARGO_PKG_VERSION"),
                env!("LIGHTNING_COMMIT"),
                ctx.uptime_minutes,
                system::this_restart()
            );
            match system::earlier_restarts() {
                earlier if earlier.is_empty() => {}
                earlier => println!("health: earlier restarts, newest first: {earlier}"),
            }
            println!("health: {} MHz, light sleep {}", health.cpu_mhz,
                if crate::power::config().map(|(_, _, ls)| ls).unwrap_or(false) { "on" } else { "off" });
            // **Why it is not sleeping, when the reason is a saved setting.** A
            // board that came back from a power cut holding itself awake is the
            // trap the lock sets; this is one of the places that springs it.
            // The recovery build never sleeps at all, and says that instead.
            match (ctx.awake_lock, cfg!(feature = "no-light-sleep")) {
                (_, true) => println!("health: no-light-sleep build -- never sleeps"),
                (true, false) => println!(
                    "health: held awake by `awake on` (saved) -- {}",
                    crate::policy::AWAKE_LOCK_COST
                ),
                (false, false) => {}
            }
            match health.die_temp_tenths {
                Some(t) => println!("health: die {}.{} C", t / 10, (t % 10).abs()),
                None => println!("health: die temperature unavailable"),
            }
            println!("health: ram {} KB free, {} KB used", health.heap_kb.0, health.heap_kb.1);
            match health.flash_kb {
                Some((free, used)) => println!("health: flash {free} KB free, {used} KB used"),
                None => println!("health: no filesystem"),
            }
            match ctx.reading {
                Some(r) => println!(
                    "health: batt {}% {} mV, rate {}.{:02} %/hr, learned range {}-{} mV",
                    r.percent, r.millivolts,
                    r.crate_centi_per_hour / 100, (r.crate_centi_per_hour % 100).abs(),
                    ctx.range.0, ctx.range.1
                ),
                None => println!("health: no fuel gauge"),
            }
        }

        // The screen's second line, plus what it cannot fit.
        Command::Status => {

            println!("status: mode {}", ctx.location.label());
            if ctx.max_sensitivity {
                // Not just "level 0": the ladder is overridden and frozen.
                println!("status: MAX SENSITIVITY -- nf 0, auto-tune frozen");
                println!("status: `sensitive off` restores the ladder");
            } else {
                // **Only `nf`, because only `nf` is written.** This used to
                // print `settings().watchdog` and `.spike_reject` beside it,
                // which stopped being true the moment the ladder became a
                // one-register tune: the chip holds those at its power-on
                // defaults and the line was reporting values nobody had sent it.
                //
                // `regs` reads the hardware and is the place to look for what
                // the chip actually holds -- this line says what the firmware
                // chose, and the two are different questions.
                // The position alone. `regs` reads the hardware, and which
                // register the machine is leaning on is a fact about the
                // implementation rather than about the weather.
                println!(
                    "status: defending {} % -- `regs` for the register values",
                    ctx.level
                );
            }
            println!(
                "status: {} strike(s), {} disturber(s) this session",
                ctx.totals.strikes, ctx.totals.disturbers
            );
            // **The raw noise count and the 0.20 one, side by side.** The rate
            // is what the tuner judges; the batches are what an unattended
            // 0.20 board could count, at about one read a batch while it slept
            // -- so a log from either firmware can be read against this one.
            let (noisy, batches) = ctx.totals.noisy_batches;
            println!(
                "status: noise {} this session -- last window {}/min, in {} of its {} batch(es)",
                ctx.totals.noise, ctx.totals.noise_per_min, noisy, batches
            );
            let hour = ctx.history.last_hour();
            // **Overhead wins**, the same way it does on the screen -- see the
            // matching branch in `ui.rs`. The flag exists so a reader can be
            // told the closest classification there is, but this line went on
            // testing `distance_km_min` alone, which overhead deliberately does
            // not touch. A storm of 417 overhead strikes therefore reported
            // `closest -`: the console saying "no idea how far" about the one
            // reading that means directly above. Same data, two renderings, and
            // only the screen had been fixed.
            let closest = if hour.overhead {
                "nearby < 5 km".to_string()
            } else if hour.distance_km_min == u8::MAX {
                "-".to_string()
            } else {
                format!("{} km", hour.distance_km_min)
            };
            println!(
                "status: last hour -- {} strikes, mean score {}, closest {}",
                hour.strikes,
                hour.mean_score_milli()
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "-".into()),
                closest
            );
            println!("status: antenna {} kHz, IRQ {}", ctx.antenna_khz,
                if ctx.irq_confirmed { "confirmed" } else { "NOT CONFIRMED" });
            // The two experiments that now survive a restart, so the line that
            // says what the device is doing has to say whether one is running.
            println!(
                "status: panel {} between refreshes",
                if ctx.panel_sleep { "powered down" } else { "left POWERED" }
            );
            // Another setting that survives a restart, and the only one that
            // costs the battery whatever the weather -- so it is said in both
            // states, like the panel's, and the recovery build says what it is.
            match (ctx.awake_lock, cfg!(feature = "no-light-sleep")) {
                (_, true) => println!("status: never sleeps -- the no-light-sleep build"),
                (true, false) => println!(
                    "status: HELD AWAKE from every boot (`awake on`) -- {}",
                    crate::policy::AWAKE_LOCK_COST
                ),
                (false, false) => println!("status: sleeps on its own -- `awake on` holds it awake across restarts"),
            }
            println!(
                "status: disturbers {}, events {}",
                match crate::session::DISTURBERS_MASKED.load(core::sync::atomic::Ordering::Relaxed) {
                    true => "masked".to_string(),
                    false => "UNMASKED".to_string(),
                },
                match ctx.strike_log.as_deref().map(|log| log.event_budget()) {
                    None | Some(0) => "off".to_string(),
                    Some(left) => format!("{left} row(s) left"),
                }
            );
            println!("status: charts showing the last {}", ctx.chart_period.label());
        }

        Command::Events(request) => match ctx.strike_log.as_deref_mut() {
            None => println!("events: no log on this device"),
            // Bare: where the budget stands, and how much more would fit.
            Some(log) if request.is_none() => {
                match log.event_budget() {
                    0 => println!("events: off -- only lightning is logged"),
                    left => println!("events: {left} row(s) left -- saved, so a restart resumes the count"),
                }
                report_room(log.free_bytes());
            }
            Some(log) => {
                let asked = request.unwrap_or(0);
                let armed = log.arm_events(asked);
                // **What the save means depends on the direction**, so it is
                // said here, where the direction is known, and never claimed
                // when it did not happen. A failed arm is lost by a restart; a
                // failed *disarm* is undone by one, which is the case that
                // used to print "(saved)" regardless.
                let saved = match (&armed.saved, armed.rows) {
                    (Ok(()), _) => "(saved)".to_string(),
                    (Err(e), 0) => format!(
                        "-- but NOT saved ({e}): a restart before the next sync would arm {} row(s) again",
                        armed.on_disk
                    ),
                    (Err(e), _) => format!(
                        "-- but NOT saved ({e}): a restart before the next sync would resume {}",
                        match armed.on_disk {
                            0 => "nothing".to_string(),
                            left => format!("{left} row(s)"),
                        }
                    ),
                };
                match armed.rows {
                    0 if asked == 0 => println!("events: off -- only lightning is logged {saved}"),
                    0 => {
                        // "(saved)" would read as though something were armed.
                        match armed.saved {
                            Ok(()) => println!("events: no room -- nothing armed"),
                            Err(_) => println!("events: no room -- nothing armed {saved}"),
                        }
                        report_room(log.free_bytes());
                        println!("events: `dump` to keep what is there, then `clear` to make room");
                    }
                    rows => {
                        if rows < asked {
                            println!("events: asked for {asked}, the log has room for {rows}");
                            report_room(log.free_bytes());
                        }
                        println!("events: logging disturbers and noise for the next {rows} row(s) {saved}");
                        // The rate is what makes the budget necessary, so it is
                        // quoted where the decision is made rather than left in
                        // a doc comment nobody reads at the console. Six to ten
                        // a second is what an unmasked board measured indoors on
                        // 2026-11-01; masked, only noise reaches the log.
                        println!(
                            "        at 6-10 a second, the indoor rate unmasked, that is {}-{} minute(s)",
                            rows / 600,
                            rows / 360
                        );
                        println!("        the log does not rotate; `dump` reads it, `clear` empties it");
                    }
                }
            }
        },
        Command::Dump => match ctx.strike_log.as_deref() {
            Some(log) => log::dump_csv(log),
            None => println!("dump: no log -- the storage partition is missing"),
        },
        Command::Defence(None) => effects.show_point = true,
        Command::Defence(Some(raw)) => effects.set_point = Some(raw),

        // A read of the sensor's state rather than a change to ours, so it
        // goes to the caller like `regs` does -- `Ctx` has no bus by design.
        Command::Srej(None) => effects.show_spike_rejection = true,
        Command::Srej(Some(level)) => effects.set_spike_rejection = Some(level),
        Command::Wdth(None) => effects.show_watchdog = true,
        Command::Wdth(Some(level)) => effects.set_watchdog = Some(level),
        Command::Import(None) => {
            println!("import: usage -- import <epoch>,<distance>,<energy>,<strokes>");
            println!("import:   distance is `overhead`, `far`, or kilometres");
            println!("import:   e.g. import 1786651612,5,67997,1");
        }
        Command::Import(Some((epoch, distance, energy_raw, strokes))) => {
            match ctx.strike_log.as_deref_mut() {
                None => println!("import: no log on this device"),
                Some(log) => {
                    let strike = Strike { distance, energy_raw };
                    // **`srej` is written as 1, and that is a judgement.** The
                    // rows this exists to restore were all recorded at 0 -- the
                    // configuration in which the chip validates man-made
                    // impulses -- and 99.1% of that history was `overhead`
                    // noise. What is imported is the remainder that carried a
                    // real distance, which is the part there is reason to
                    // trust; marking it 1 says so in the column that means
                    // "trust this row", rather than restating a register value
                    // that was the same for every row and told you nothing.
                    log.append(epoch, 0, 0, 1, &strike, false, strokes.max(1));
                    println!(
                        "import: {} -- {} strike(s), energy {}",
                        clock::format_local(epoch),
                        strokes.max(1),
                        energy_raw,
                    );
                    effects.clock_saved = false;
                }
            }
        }
        Command::Golden(clear) => {
            if clear {
                match crate::settings::clear_golden() {
                    Ok(()) => println!(
                        "gold: forgotten -- the next strike with a distance starts a new record"
                    ),
                    Err(e) => println!("gold: could not clear -- {e}"),
                }
                return effects;
            }
            match crate::settings::golden() {
                None => {
                    println!("gold: nothing recorded -- this device has not heard a strike with a");
                    println!("gold: distance yet, or the record was cleared. The first one writes it;");
                    println!("gold: `overhead` never does -- see `golden::is_evidence`.");
                }
                Some(record) => {
                    println!(
                        "gold: nf {} wd {} sr {} ({}) -- {} strike(s) heard here",
                        record.combo.nf,
                        record.combo.wdth,
                        record.combo.srej,
                        if record.combo.outdoor { "outdoor" } else { "indoor" },
                        record.strikes,
                    );
                    if record.strikes < crate::golden::TRUSTED_STRIKES {
                        println!(
                            "gold: not yet trusted -- {} strike(s) needed before the tuner will",
                            crate::golden::TRUSTED_STRIKES
                        );
                        println!("gold: return to it, because one detection at srej 0 may be man-made");
                    } else {
                        println!(
                            "gold: trusted -- the tuner returns here after {} min of silence at a deafer point",
                            crate::golden::PATIENCE_MINUTES
                        );
                    }
                }
            }
            // **What a firmware before 0.22.0 wrote, shown and not used.** It
            // counted `overhead` as lightning, so its count cannot say how much
            // of it was evidence; a person who remembers it should be able to
            // see where it went rather than wonder why the trust is gone.
            if let Some(old) = crate::settings::legacy_golden() {
                println!(
                    "gold: set aside -- nf {} wd {} sr {} ({}), {} strike(s), from before 0.22.0",
                    old.combo.nf,
                    old.combo.wdth,
                    old.combo.srej,
                    if old.combo.outdoor { "outdoor" } else { "indoor" },
                    old.strikes,
                );
                println!("gold:   it counted `overhead` as lightning, so nothing acts on it;");
                println!("gold:   `golden clear` removes it with the current record");
            }
        }
        Command::AccessPoint(request) => {
            // Credentials are stored here, because that is NVS and not a radio.
            // Raising the network itself is the loop's job.
            if let ApRequest::Set(ssid, password) = &request {
                match crate::credentials::Credentials::check(ssid, password) {
                    Ok(()) => {
                        let credentials = crate::credentials::Credentials {
                            ssid: ssid.clone(),
                            password: password.clone(),
                            generated: false,
                        };
                        match credentials.save() {
                            Ok(()) => println!("ap:   stored `{ssid}`"),
                            Err(e) => println!("ap:   could not store -- {e}"),
                        }
                    }
                    Err(why) => {
                        println!("ap:   refused -- {why}");
                        // Not raised: the pair the operator just gave is the
                        // pair they expect to see, and raising the old one
                        // under a rejection reads as the rejection not having
                        // happened.
                        return effects;
                    }
                }
            }
            effects.access_point = Some(request);
        }
        Command::ClearStats => effects.clear_statistics = true,
        Command::Merge(None) => {
            let window_ms = ctx.totals.merger.window_ms();
            match window_ms {
                0 => println!("merge: off -- every return stroke is its own record"),
                ms => println!("merge: {ms} ms -- strokes inside it become one flash"),
            }
        }
        Command::Merge(Some(window_ms)) => {
            // Whatever was mid-flash under the old window is committed rather
            // than dropped. Those strokes were detected; a settings change is
            // not a reason to lose them.
            let flushed = ctx.totals.merger.set_window_ms(window_ms);
            if let Some(merged) = flushed {
                session::commit_merged(
                    ctx.totals,
                    ctx.history,
                    ctx.strike_log.as_deref_mut(),
                    &merged,
                );
            }
            match crate::settings::store_merge_window_ms(window_ms) {
                Ok(()) => println!("merge: {window_ms} ms (saved)"),
                Err(e) => println!("merge: {window_ms} ms but NOT saved -- {e}"),
            }
        }

        Command::Calibrate(seconds, quiet) => {
            // Acknowledged here so the reply lands before the sweep starts.
            println!("cal:  queued at {seconds} s per probe -- the sensor is");
            println!("cal:  deliberately mis-set while it searches");
            effects.calibrate = Some((seconds, quiet));
        }
        Command::Sensitive(on) => {
            effects.sensitivity = Some(on);
            effects.redraw_now = true;
        }
        Command::MaskDisturbers(Some(on)) => effects.mask_disturbers = Some(on),
        Command::PanelSleep(Some(on)) => {
            effects.panel_sleep = Some(on);
            // A redraw now, so the new setting is in force at once rather than
            // after the next baseline -- an A/B whose B starts five minutes
            // late is a comparison of the wrong five minutes.
            effects.redraw_now = true;
        }
        Command::PanelSleep(None) => match ctx.panel_sleep {
            true => println!("panel: powered down between refreshes -- `panel sleep off` keeps it up"),
            false => println!("panel: left powered between refreshes -- `panel sleep on` to power it down"),
        },
        // Reported from what was last written rather than read off the chip:
        // the bit shares `0x03` with the interrupt reason, and reading that
        // register clears a pending event. See `session::DISTURBERS_MASKED`.
        Command::MaskDisturbers(None) => {
            let masked =
                crate::session::DISTURBERS_MASKED.load(core::sync::atomic::Ordering::Relaxed);
            let saved = match crate::settings::mask_disturbers() {
                Some(_) => "saved",
                None => "the default",
            };
            match masked {
                true => println!("mask: disturbers kept off the IRQ ({saved})"),
                false => println!("mask: disturbers RAISE the IRQ ({saved}) -- `mask on` to stop them"),
            }
        }
        Command::Freq(request) => effects.freq = Some(request),
        Command::Sleep(on) => effects.light_sleep = Some(on),
        Command::Awake(Some(on)) => effects.set_awake_lock = Some(on),
        Command::Awake(None) => effects.show_awake_lock = true,
        Command::Battery => effects.read_battery = true,
        Command::Regs => effects.dump_registers = true,
        Command::SetMode(indoor) => {
            effects.set_indoor = Some(indoor);
            effects.redraw_now = true;
        }
        Command::Help => crate::console::print_help(),
        Command::Unknown => println!("?  try: help"),
    }
    effects
}


/// How much more the log could take as event rows, and what is held back.
///
/// Said wherever a budget is set or asked about, because "capped" with no
/// figures reads as the device refusing for no reason.
fn report_room(free_bytes: u32) {
    println!(
        "events: room for {} row(s) -- {} KB free, {} KB kept back so strikes can still be written",
        crate::budget::rows_that_fit(free_bytes),
        free_bytes / 1024,
        crate::budget::RESERVED_BYTES / 1024
    );
}
