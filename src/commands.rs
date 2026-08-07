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
use crate::console::Command;
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
    pub calibrate: Option<u32>,
    /// `Some(on)` when `sensitive` was used. Applied by the caller rather than
    /// here, because `Ctx` deliberately has no sensor or bus — every other
    /// console command is pure, and one hardware command should not change that
    /// for all of them.
    pub sensitivity: Option<bool>,
    /// Set by `regs`; the caller owns the bus. Same reason as `sensitivity`.
    pub dump_registers: bool,
    /// `Some(indoor)` when `mode` was used.
    pub set_indoor: Option<bool>,
    /// Set by `freq`. Handled by the caller, which owns the policy loop — doing
    /// it here would be undone on the next tick.
    pub freq: Option<crate::console::FreqRequest>,
    /// Set by `sleep on|off`. Same reason as `freq`.
    pub light_sleep: Option<bool>,
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
                // arithmetic a real one would have.
                energy_raw: intensity_milli * 16777 / 1000,
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
            println!("health: {} MHz, light sleep {}", health.cpu_mhz,
                if crate::power::config().map(|(_, _, ls)| ls).unwrap_or(false) { "on" } else { "off" });
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
            let hour = ctx.history.last_hour();
            println!(
                "status: last hour -- {} strikes, mean score {}, closest {}",
                hour.strikes,
                hour.mean_score_milli()
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "-".into()),
                if hour.distance_km_min == u8::MAX {
                    "-".to_string()
                } else {
                    format!("{} km", hour.distance_km_min)
                }
            );
            println!("status: antenna {} kHz, IRQ {}", ctx.antenna_khz,
                if ctx.irq_confirmed { "confirmed" } else { "NOT CONFIRMED" });
            println!("status: charts showing the last {}", ctx.chart_period.label());
        }

        Command::Dump => match ctx.strike_log.as_deref() {
            Some(log) => log::dump_csv(log),
            None => println!("dump: no log -- the storage partition is missing"),
        },
        Command::Calibrate(seconds) => {
            // Acknowledged here so the reply lands before the sweep starts
            // holding the loop for minutes.
            println!("cal:  queued at {seconds} s per probe -- the sensor is");
            println!("cal:  deliberately mis-set while it searches, and this");
            println!("cal:  takes a while");
            effects.calibrate = Some(seconds);
        }
        Command::Sensitive(on) => {
            effects.sensitivity = Some(on);
            effects.redraw_now = true;
        }
        Command::Freq(request) => effects.freq = Some(request),
        Command::Sleep(on) => effects.light_sleep = Some(on),
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
