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
    /// `awake on`, read from NVS at boot and changed only by `awake on|off`.
    /// Held beside the policy and the override because it is an input to the
    /// one and gives way to the other.
    pub awake_lock: &'a mut bool,
    /// Uptime at the last clock save, which `date <epoch>` resets.
    pub clock_saved_s: &'a mut u32,
}

/// What `esp_pm` is enforcing, and who chose it, for `freq` and `awake`.
///
/// Read back from the chip rather than restated from what was asked for --
/// `esp_pm_configure` can reject a config and leave the previous one running.
/// One function, because `freq` and `awake` answer the same question and a
/// line that drifted between them would be two answers.
fn enforced(freq_override: Option<u32>, policy: power::Policy, awake_lock: bool) -> String {
    let chosen_by = match (freq_override, awake_lock) {
        (Some(mhz), _) => format!("pinned at {mhz} MHz"),
        (None, true) => format!("policy {}, held by `awake on`", policy.label()),
        (None, false) => format!("policy {}", policy.label()),
    };
    match power::config() {
        Some((max, min, sleep)) => format!(
            "now {} MHz -- pm {}/{} MHz, light sleep {}, {chosen_by}",
            system::cpu_mhz(),
            min,
            max,
            if sleep { "on" } else { "off" },
        ),
        None => format!("{} MHz (pm read-back failed)", system::cpu_mhz()),
    }
}

/// Say so when the lock can change nothing on this board as flashed.
///
/// The recovery build compiles light sleep out (`policy::decide` returns awake
/// before it reads the lock), so `awake off` there would read as a promise to
/// sleep that the image cannot keep. The setting is still saved, for the
/// default image that will read it.
fn awake_in_this_build() {
    if cfg!(feature = "no-light-sleep") {
        println!("awake:   this is the no-light-sleep build, which never sleeps either way --");
        println!("awake:   the setting is kept for the default image");
    }
}

/// Run one console command and apply whatever it asked for.
/// Returns what `ap` asked for, if anything, for the caller to act on — see
/// [`commands::Effects::access_point`].
pub fn handle(
    command: console::Command,
    hw: &mut Hardware<'_, '_>,
    rt: &mut Runtime<'_>,
    now_ms: u32,
    minute: u32,
) -> Option<console::ApRequest> {
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
                panel_sleep: rt.screen.panel_sleep,
                awake_lock: *rt.awake_lock,
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
                    //
                    // The lock goes in as it stands: `freq auto` hands the
                    // clock back to the policy, and while `awake on` is saved
                    // the policy is awake.
                    let want = power::decide(
                        now_ms / 1000,
                        Some(now_ms / 1000),
                        None,
                        power::Supply::Unknown,
                        *rt.awake_lock,
                    );
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
            println!("freq: {}", enforced(*rt.freq_override, *rt.policy, *rt.awake_lock));
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
                            // An override for this session only, so say what
                            // the restart that ends it will do.
                            if *rt.awake_lock {
                                println!("sleep: `awake on` is saved -- after `freq auto` or a restart it is held awake again");
                            }
                        }
                    }
                    Err(e) => println!("sleep: could not change it -- {e}"),
                },
                None => println!("sleep: could not read the current config -- unchanged"),
            }
        }

        if let Some(on) = effects.set_awake_lock {
            // **Saved first, then applied.** The sensor's settings go the other
            // way -- tell the chip, keep only what it accepted -- but there is
            // no chip here to refuse anything: what `awake` promises is the
            // next boot, so the save is the half that matters, and the answer
            // says whether it landed.
            //
            // **What a failed save means depends on the direction**, so it is
            // said from what NVS still holds rather than guessed: a failed
            // `awake on` is lost by a restart, a failed `awake off` is undone
            // by one.
            let held_before = crate::policy::awake_lock(settings::awake_lock());
            let saved = match settings::store_awake_lock(on) {
                Ok(()) => "saved".to_string(),
                Err(e) => format!(
                    "NOT saved ({e}) -- a restart goes back to {}",
                    match held_before {
                        true => "holding it awake",
                        false => "sleeping on its own",
                    }
                ),
            };
            *rt.awake_lock = on;
            match on {
                true => println!(
                    "awake: held at 160 MHz with light sleep off, from every boot until `awake off` ({saved})"
                ),
                false => println!("awake: off -- the board sleeps on its own again ({saved})"),
            }

            // **A `freq` or `sleep` override in force is left alone.** It is a
            // person's choice for this session, newer than the lock's, and
            // `freq auto` is the documented way to hand the clock back -- to a
            // policy that is now awake or not as this command said.
            match *rt.freq_override {
                Some(mhz) => println!(
                    "awake:   `freq`/`sleep` still pins {mhz} MHz until `freq auto` or a restart"
                ),
                None => {
                    // Decided now rather than at the next gauge poll, as `freq
                    // auto` does, with the console stamped this instant for the
                    // same reason -- so `awake off` keeps the port for the ten
                    // minutes any command does, and then the board sleeps.
                    let want = power::decide(
                        now_ms / 1000,
                        Some(now_ms / 1000),
                        None,
                        power::Supply::Unknown,
                        on,
                    );
                    match power::apply(want) {
                        Ok(()) => *rt.policy = want,
                        Err(e) => println!("awake:   could not apply the {} policy -- {e}", want.label()),
                    }
                    if !on && !cfg!(feature = "no-light-sleep") {
                        println!(
                            "awake:   the console holds it {} min, then it light-sleeps and the USB port goes",
                            crate::policy::CONSOLE_AWAKE_S / 60
                        );
                    }
                }
            }
            if on {
                println!("awake:   costs the battery -- {}", crate::policy::AWAKE_LOCK_COST);
                println!("awake:   `awake off` before it goes out on its cell");
            }
            awake_in_this_build();
            println!("awake: {}", enforced(*rt.freq_override, *rt.policy, *rt.awake_lock));
        }

        if effects.show_awake_lock {
            let source = match settings::awake_lock() {
                Some(_) => "saved",
                None => "the default",
            };
            match *rt.awake_lock {
                true => {
                    println!("awake: ON ({source}) -- 160 MHz, light sleep off, from every boot");
                    println!(
                        "awake:   costs the battery -- {}; `awake off` lets it sleep",
                        crate::policy::AWAKE_LOCK_COST
                    );
                }
                false => println!(
                    "awake: off ({source}) -- light sleep {} min after boot and {} after a command; `awake on` holds it",
                    crate::policy::GRACE_S / 60,
                    crate::policy::CONSOLE_AWAKE_S / 60
                ),
            }
            awake_in_this_build();
            println!("awake: {}", enforced(*rt.freq_override, *rt.policy, *rt.awake_lock));
        }

        if let Some(mask) = effects.mask_disturbers {
            // **`mask off` is the experiment now, not `mask on`.** Boot masks
            // disturbers -- see `boot::configure` for the measurement -- so the
            // interesting thing to ask this command for is the disturber log
            // back, which is the only record of whether real flashes are
            // arriving here classified as not-lightning.
            //
            // Unmasking costs what boot's note describes. Under the old edge
            // trigger it was missed edges; under the level trigger it is the
            // shared reason register -- a disturber that lands before a
            // pending strike is read replaces it -- and on the cell a wake per
            // disturber.
            //
            // **Saved, since 0.21.0**, in the order every other setting uses:
            // tell the chip, and persist only what the chip accepted. Held in
            // RAM alone, `mask off` ended the first time anybody opened the
            // console port to see how the experiment was going -- every open
            // restarts the board.
            match hw.sensor.set_disturber_enabled(hw.i2c, !mask) {
                Ok(()) => {
                    crate::session::DISTURBERS_MASKED
                        .store(mask, core::sync::atomic::Ordering::Relaxed);
                    let saved = match settings::store_mask_disturbers(mask) {
                        Ok(()) => "saved".to_string(),
                        Err(e) => format!("NOT saved -- {e}"),
                    };
                    println!(
                        "mask: disturbers {} the IRQ ({saved})",
                        match mask {
                            true => "no longer raise",
                            false => "raise",
                        }
                    );
                    if !mask {
                        println!("mask: a disturber can replace a strike's reason before it is read,");
                        println!("mask: and on the cell each one is a wake -- see boot::configure");
                        println!("mask: it stays off across restarts until `mask on`");
                    }
                }
                Err(e) => println!("mask: could not change it -- {e}"),
            }
        }

        if let Some(on) = effects.panel_sleep {
            // Held on the screen, which is what passes it to every refresh, and
            // saved so the comparison outlives the restart the next console
            // connection causes. The redraw `commands` asked for applies it.
            rt.screen.panel_sleep = on;
            let saved = match settings::store_panel_sleep(on) {
                Ok(()) => "saved".to_string(),
                Err(e) => format!("NOT saved -- {e}"),
            };
            match on {
                true => println!("panel: powered down between refreshes ({saved}) -- from this redraw"),
                false => println!("panel: left powered between refreshes ({saved}) -- from this redraw"),
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
                Ok(()) => {
                    // Same reason as the button's path: a gain change rescales
                    // every energy the distance estimate was built from.
                    session::restart_statistics(hw.sensor, hw.i2c, "gain changed");
                    // And the defence point, for the same reason one step
                    // further on: it is a description of a noise floor measured
                    // through the old gain. The button's copy is in `listen`.
                    rt.tuning.gain_changed(hw.sensor, hw.i2c, "mode");
                    match settings::store_location(*rt.location) {
                        Ok(()) => println!("mode: {} gain applied (saved)", rt.location.label()),
                        Err(e) => println!(
                            "mode: {} gain applied but NOT saved -- {e}",
                            rt.location.label()
                        ),
                    }
                }
                Err(e) => println!("mode: could not apply -- {e}"),
            }
        }
        if effects.show_spike_rejection {
            // What the chip holds, then where it came from -- the two differ
            // when boot took a golden record's value over the saved one.
            let level = crate::session::spike_rejection();
            match level {
                0 => println!("srej: 0 -- man-made impulses will be reported as strikes"),
                n => println!("srej: {n} (`regs` reads it back off the chip)"),
            }
            let default = crate::defence::SPIKE_REJECTION_DEFAULT;
            match crate::settings::spike_rejection() {
                None => println!("srej:   nothing saved -- the default, {default}, the chip's own"),
                Some(saved) if saved != level => println!(
                    "srej:   saved {saved}; boot took {level} from the golden record -- `golden clear` keeps {saved}"
                ),
                Some(saved) if crate::defence::spike_rejection_below_default(Some(saved)) => println!(
                    "srej:   saved by `srej {saved}`, below the default of {default}"
                ),
                Some(_) => {}
            }
        }
        if let Some(level) = effects.set_spike_rejection {
            // Allowed, and said out loud. Zero is a legitimate experiment and
            // an illegitimate resting state; the difference is whether somebody
            // knows it is set.
            if level < crate::defence::SPIKE_REJECTION_AUTO_MIN {
                println!("srej: ⚠ 0 is the configuration that logged 503 false strikes in 3.5 h");
                println!("srej:   from an electric hammer, every one graded `overhead`.");
                println!("srej:   the device will not choose this for itself -- you have.");
            }
            match hw.sensor.set_spike_rejection(hw.i2c, level) {
                Ok(()) => {
                    crate::session::SPIKE_REJECTION
                        .store(level, core::sync::atomic::Ordering::Relaxed);
                    match crate::settings::store_spike_rejection(level) {
                        Ok(()) => println!("srej: {level} (saved)"),
                        Err(e) => println!("srej: {level} applied but NOT saved -- {e}"),
                    }
                    // **Say now what the next restart will do**, because every
                    // open of the console port is one. Boot takes the more
                    // permissive of this and a trusted record's, so raising it
                    // above the record's is undone at the next connect -- the
                    // trap for a 1-against-2 comparison, and invisible unless
                    // told.
                    let here_outdoor = matches!(*rt.location, crate::as3935::Location::Outdoor);
                    // The record boot would act on: trusted, at this gain.
                    let heard = match crate::settings::golden() {
                        Some(record)
                            if record.strikes >= crate::golden::TRUSTED_STRIKES
                                && record.combo.outdoor == here_outdoor =>
                        {
                            Some(record.combo.srej)
                        }
                        _ => None,
                    };
                    let at_boot = crate::defence::spike_rejection_at_boot(Some(level), heard);
                    if at_boot < level {
                        println!(
                            "srej: note -- the golden record heard lightning at {at_boot}, and boot takes the"
                        );
                        println!(
                            "srej:   more permissive, so the next restart returns to {at_boot}; `golden clear` keeps {level}"
                        );
                    }
                }
                Err(e) => println!("srej: could not apply -- {e}"),
            }
        }
        if effects.show_watchdog {
            let level = crate::settings::watchdog().unwrap_or(crate::defence::WATCHDOG_DEFAULT);
            match level {
                0 => println!("wdth: 0 -- no amplitude gate; expect man-made impulses"),
                n => println!("wdth: {n} (`regs` reads it back off the chip)"),
            }
        }
        if let Some(level) = effects.set_watchdog {
            // Applied straight to the chip and stored, exactly as `srej` is. It
            // does not go through `session::apply`, which reads the stored value
            // -- so storing first and applying second would work too, but this
            // order is the one every other setting uses: tell the sensor, and
            // only persist what the sensor accepted.
            match hw.sensor.set_watchdog_threshold(hw.i2c, level) {
                Ok(()) => match crate::settings::store_watchdog(level) {
                    Ok(()) => println!("wdth: {level} (saved)"),
                    Err(e) => println!("wdth: {level} applied but NOT saved -- {e}"),
                },
                Err(e) => println!("wdth: could not apply -- {e}"),
            }
        }
        if effects.clear_statistics {
            // The one way to say "re-estimate from here" without perturbing the
            // receiver. Every other reset is a side effect of changing the gain,
            // the point or the sensitivity -- each of which changes what is
            // being measured as well as what is remembered.
            session::restart_statistics(hw.sensor, hw.i2c, "asked for");
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

        let access_point = effects.access_point.clone();

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
                true => {
                    let applied = session::force_max_sensitivity(hw.sensor, hw.i2c);
                    // **Tell the tuner where the chip actually is.**
                    //
                    // `force_max_sensitivity` programs the registers and
                    // returns; nothing told `Tuning` its point had moved. So the
                    // gauge went on reporting the pre-override level -- "3/7
                    // 43%" while the chip sat wide open -- and because `frozen`
                    // stops `step` from running, the noise caption and the
                    // JAMMED warning froze with it.
                    //
                    // A panel that reports a level the hardware is not at is
                    // worse than one that reports nothing: it is the number
                    // somebody will use to decide whether the device is working.
                    if applied.is_ok() {
                        rt.tuning.observe_forced_point(crate::defence::Point::OPEN);
                    }
                    applied
                }
                false => rt.tuning.open(hw.sensor, hw.i2c, now_ms),
            };
            match outcome {
                Ok(()) if on => println!(
                    // **Names only what it actually sets.** The override now
                    // opens the one knob left in the point and leaves both
                    // settings where the operator put them -- which is the
                    // point: maximum sensitivity to weak distant strikes
                    // without re-admitting man-made impulses. Earlier wordings
                    // claimed `srej 0`, then `wdth 0`, while the chip held
                    // neither; a status line that overstates what it changed is
                    // how an operator ends up debugging the wrong register.
                    "sens: MAX -- nf 0, min strikes 1; auto-tune frozen \
(watchdog and spike rejection unchanged -- `wdth`, `srej`)"
                ),
                Ok(()) => println!("sens: normal -- defence back to 0"),
                Err(e) => println!("sens: could not apply -- {e}"),
            }
        }

        access_point
}
