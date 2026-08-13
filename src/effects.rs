//! Applying the console's hardware effects.
//!
//! **`commands` decides, this acts.** `commands::run` is deliberately pure — its
//! `Ctx` has no sensor, no bus and no notification — because a console handler
//! that can touch hardware is one that has to be reasoned about alongside the
//! event loop. What it returns instead is an [`Effects`](crate::commands::Effects)
//! record of what should happen, and this is where those happen.
//!
//! That indirection is not ceremony. It is why `commands` can be read on its own
//! and why the loop below is a list of independent `if`s rather than a switch
//! that reaches into thirty locals.
//!
//! The parameters come in two halves, matching the split the rest of the crate
//! draws: [`Hardware`] is borrowed for the call and owned by `main`, while
//! [`Runtime`] is the state islands — [`Tuning`](crate::tuning::Tuning),
//! [`Screen`](crate::screen::Screen), [`Fuel`](crate::battery::Fuel) — that
//! survive between iterations.

use esp_idf_hal::i2c::I2cDriver;

use crate::as3935::{As3935, Location};
use crate::session::Totals;
use crate::{
    battery, commands, console, history, log, power, screen, session, settings, system, tuning,
};

/// What the effects may talk to. Borrowed for one call, never held.
pub struct Hardware<'a, 'd> {
    pub sensor: &'a As3935,
    pub i2c: &'a mut I2cDriver<'d>,
    pub gauge: Option<&'a battery::Max17048>,
    pub die_temperature: Option<&'a system::DieTemperature>,
    pub antenna_khz: u32,
    pub irq_confirmed: bool,
}

/// The state islands an effect may move.
pub struct Runtime<'a> {
    pub location: &'a mut Location,
    pub totals: &'a mut Totals,
    pub history: &'a mut history::History,
    pub strike_log: Option<&'a mut log::Log>,
    pub tuning: &'a mut tuning::Tuning,
    pub screen: &'a mut screen::Screen,
    pub fuel: &'a mut battery::Fuel,
    pub policy: &'a mut power::Policy,
    pub freq_override: &'a mut Option<u32>,
    /// Uptime at the last clock save, which `date <epoch>` resets.
    pub clock_saved_s: &'a mut u32,
}

/// Run one console command and apply whatever it asked for.
pub fn handle(
    command: console::Command,
    hw: &mut Hardware<'_, '_>,
    rt: &mut Runtime<'_>,
    now_ms: u32,
    minute: u32,
) {
        let effects = commands::run(
            command,
            &mut commands::Ctx {
                location: rt.location,
                totals: rt.totals,
                history: rt.history,
                strike_log: rt.strike_log.as_deref_mut(),
                chart_period: &mut rt.screen.period,
                reading: rt.fuel.reading,
                range: rt.fuel.range,
                level: rt.tuning.point.percent(),
                die_temperature: hw.die_temperature,
                antenna_khz: hw.antenna_khz,
                irq_confirmed: hw.irq_confirmed,
                minute,
                uptime_minutes: now_ms / 60_000,
                max_sensitivity: rt.tuning.frozen,
            },
        );
        if effects.clock_saved {
            *rt.clock_saved_s = now_ms / 1000;
        }
        if effects.redraw_now {
            rt.screen.invalidate();
            rt.screen.user_acted = true;
        }
        if effects.read_battery {
            match hw.gauge {
                // One read, decoded and printed raw, so the two lines always
                // describe the same instant and can be checked against each
                // other. Also deliberately fresh rather than the main loop's
                // cached sample: `battery` is asked when somebody wants to
                // know *now*.
                Some(gauge) => match gauge.read_raw(hw.i2c) {
                    Ok((vcell, soc, crate_raw)) => {
                        let now = battery::Reading::from_raw(vcell, soc, crate_raw);
                        println!(
                            "batt: {}% -- {}.{:02} V, {} mV",
                            now.percent,
                            now.millivolts / 1000,
                            (now.millivolts % 1000) / 10,
                            now.millivolts
                        );
                        println!(
                            "batt: {} -- CRATE {}.{:02} %/hr",
                            battery::flow(&now, rt.fuel.trend.as_ref()).label(),
                            now.crate_centi_per_hour / 100,
                            (now.crate_centi_per_hour % 100).abs()
                        );
                        match rt.fuel.trend.as_ref() {
                            Some(t) if t.span_s() >= 60 => println!(
                                "batt: trend {:+} mV over {} min (>= {} mV counts)",
                                t.delta_mv(),
                                t.span_s() / 60,
                                battery::TREND_THRESHOLD_MV
                            ),
                            // Say so rather than printing a delta that has
                            // had no time to mean anything.
                            _ => println!("batt: trend -- not enough history yet"),
                        }
                        println!("batt: learned range {}-{} mV", rt.fuel.range.0, rt.fuel.range.1);
                        println!(
                            "batt: raw VCELL 0x{vcell:04X}  SOC 0x{soc:04X}  CRATE 0x{crate_raw:04X}"
                        );
                    }
                    Err(e) => println!("batt: read failed -- {e}"),
                },
                None => println!("batt: no gauge found on the bus"),
            }
        }
        if let Some(request) = effects.freq {
            use crate::console::FreqRequest;
            match request {
                FreqRequest::Pin(mhz) if !power::PINNABLE_MHZ.contains(&mhz) => println!(
                    "freq: {mhz} MHz is not one of {:?} -- unchanged",
                    power::PINNABLE_MHZ
                ),
                FreqRequest::Pin(mhz) => match power::pin(mhz) {
                    Ok(()) => {
                        (*rt.freq_override) = Some(mhz);
                        println!("freq: pinned at {mhz} MHz, light sleep off, policy paused");
                    }
                    Err(e) => println!("freq: could not pin {mhz} MHz -- {e}"),
                },
                FreqRequest::Auto => {
                    (*rt.freq_override) = None;
                    // Re-apply immediately rather than waiting for the next
                    // tick: the loop below only acts on a *change*, and the
                    // policy it last recorded is what the pin displaced.
                    // `Some(now)` rather than the loop's `last_console_s`,
                    // and identically so: this runs *because* a console command
                    // just arrived, and the caller stamps that variable with
                    // this same instant immediately before calling in.
                    let want = power::decide(now_ms / 1000, Some(now_ms / 1000));
                    match power::apply(want) {
                        Ok(()) => {
                            *rt.policy = want;
                            println!("freq: back on the {} policy", rt.policy.label());
                        }
                        Err(e) => println!("freq: could not restore the policy -- {e}"),
                    }
                }
                FreqRequest::Report => {}
            }
            // Always report what the chip is enforcing, not what we asked
            // for -- `esp_pm_configure` can reject a config and leave the
            // previous one running.
            match power::config() {
                Some((max, min, sleep)) => println!(
                    "freq: now {} MHz -- pm {}/{} MHz, light sleep {}, {}",
                    system::cpu_mhz(),
                    min,
                    max,
                    if sleep { "on" } else { "off" },
                    match *rt.freq_override {
                        Some(mhz) => format!("pinned at {mhz} MHz"),
                        None => format!("policy {}", rt.policy.label()),
                    }
                ),
                None => println!("freq: {} MHz (pm read-back failed)", system::cpu_mhz()),
            }
        }
        if let Some(on) = effects.light_sleep {
            // Keep whatever clock is in force and change only the sleep
            // flag, so `sleep off` after `freq 80` does not silently drag
            // the frequency back to something else.
            match power::config() {
                Some((max, min, _)) => match power::set_light_sleep(max, min, on) {
                    Ok(()) => {
                        // Counts as an override either way: the policy loop
                        // would otherwise restore its own idea of both.
                        (*rt.freq_override) = Some(max);
                        println!(
                            "sleep: light sleep {} at {min}/{max} MHz, policy paused",
                            if on { "on" } else { "off" }
                        );
                        if on {
                            println!("sleep: the USB port will go with it -- `freq auto` or a power cycle to return");
                        }
                    }
                    Err(e) => println!("sleep: could not change it -- {e}"),
                },
                None => println!("sleep: could not read the current config -- unchanged"),
            }
        }
        if let Some(indoor) = effects.set_indoor {
            *rt.location = match indoor {
                true => Location::Indoor,
                false => Location::Outdoor,
            };
            match hw.sensor.set_location(hw.i2c, *rt.location) {
                // Outdoor is the LOWER gain, which is the counter-intuitive
                // half: indoor gain is ~4x, and a strike close enough to hear
                // saturates the front end, fails validation, and is reported
                // as a disturber. Less gain is what recovers a near storm.
                //
                // **Persisted, exactly as the button's copy of this does.**
                // Until 0.7.4 it was not, so the two ways of changing the mode
                // disagreed about whether the change survived a reboot: BOOT
                // saved it, `mode` did not. A reset during an overhead storm
                // silently restored the gain the *button* had last chosen,
                // which is the one setting that was actually mattering.
                Ok(()) => match settings::store_location(*rt.location) {
                    Ok(()) => println!("mode: {} gain applied (saved)", rt.location.label()),
                    Err(e) => println!(
                        "mode: {} gain applied but NOT saved -- {e}",
                        rt.location.label()
                    ),
                },
                Err(e) => println!("mode: could not apply -- {e}"),
            }
        }
        if effects.dump_registers {
            match hw.sensor.dump_registers(hw.i2c) {
                Ok(r) => {
                    for (n, value) in r.iter().enumerate() {
                        println!("regs: 0x0{n} = 0x{value:02X}  {:08b}", value);
                    }
                    // Decoded, because the failure this exists to catch is a
                    // bit being set that nobody meant to set.
                    println!(
                        "regs: pwd {}  afe 0x{:02X} ({})",
                        r[0] & 0x01,
                        (r[0] & 0x3E) >> 1,
                        if (r[0] & 0x3E) >> 1 == 0x12 { "indoor" } else { "outdoor" }
                    );
                    println!(
                        "regs: nf {}  wdth {}  srej {}  min-strikes-bits {}",
                        (r[1] & 0x70) >> 4,
                        r[1] & 0x0F,
                        r[2] & 0x0F,
                        (r[2] & 0x30) >> 4
                    );
                    println!(
                        "regs: int 0x{:X}  mask_dist {}  lco_fdiv {}",
                        r[3] & 0x0F,
                        (r[3] & 0x20) >> 5,
                        (r[3] & 0xC0) >> 6
                    );
                    // The one that makes the sensor deaf: any of these three
                    // set means the IRQ pin is a clock output, not an
                    // interrupt line.
                    let display = (r[8] & 0xE0) >> 5;
                    println!(
                        "regs: tun_cap {}  irq_display {:03b}{}",
                        r[8] & 0x0F,
                        display,
                        if display != 0 {
                            "  *** IRQ PIN IS A CLOCK OUTPUT -- SENSOR IS DEAF ***"
                        } else {
                            ""
                        }
                    );
                }
                Err(e) => println!("regs: read failed -- {e}"),
            }
        }

        if effects.show_point {
            rt.tuning.report();
        }

        if let Some(raw) = effects.set_point {
            rt.tuning.place(hw.sensor, hw.i2c, raw, now_ms);
        }

        // A sweep runs as ordinary measurement windows from here on -- the
        // tune block below spends each one on the search instead of on the
        // +/-1 walk -- so the loop, the console and the screen all keep
        // working throughout.
        if let Some((window_s, requested_quiet)) = effects.calibrate {
            rt.tuning.begin_sweep(hw.sensor, hw.i2c, window_s, requested_quiet, now_ms);
            rt.screen.user_acted = true;
        }


        // Applied here rather than in the command handler because `Ctx` has
        // no sensor or bus -- see `Effects::sensitivity`.
        if let Some(on) = effects.sensitivity {
            rt.tuning.frozen = on;
            let outcome = match on {
                true => session::force_max_sensitivity(hw.sensor, hw.i2c),
                false => rt.tuning.open(hw.sensor, hw.i2c, now_ms),
            };
            match outcome {
                Ok(()) if on => println!(
                    "sens: MAX -- nf 0, wdth 0, srej 0, min strikes 1; auto-tune frozen"
                ),
                Ok(()) => println!("sens: normal -- defence back to 0"),
                Err(e) => println!("sens: could not apply -- {e}"),
            }
        }
}
