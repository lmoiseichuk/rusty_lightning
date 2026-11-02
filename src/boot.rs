//! Bring-up: everything that happens once, before the loop.
//!
//! **Split from `main` because it is a different kind of code.** Bring-up runs
//! once, fails loudly, and is read when something is wrong at start-up; the loop
//! runs forever, must not panic, and is read when something is wrong at three in
//! the morning. Keeping them in one file meant scrolling past three hundred
//! lines of one-time setup to reach the thing that actually runs.
//!
//! What stays in `main` is the part that cannot move: the peripheral handles are
//! lifetime-bound to `Peripherals`, so returning them from a function would need
//! a self-referential struct. `main` therefore owns the hardware and calls in
//! here for the work.

use esp_idf_hal::delay::FreeRtos;
use esp_idf_hal::gpio::PinDriver;
use esp_idf_hal::i2c::I2cDriver;

use crate::as3935::{As3935, Location};
use crate::{as3935, defence, session, settings};


/// Antenna tuning capacitance, picofarads. The SEN0290's factory value, and
/// changing it needs a scope on the IRQ pin (§3 step 5).
pub const TUNING_CAPS_PF: u8 = 120;


// **The DTR argument moved to `press`.** GPIO9 carries the only button on this
// board and the C3 wires DTR to it, so a flashing tool holding the port open is
// indistinguishable from a fingertip except by duration -- which is the whole
// reason `press` classifies by how long a hold lasts. The thresholds and the
// reasoning now live together there; the 1.5 s floor these two constants
// encoded is `press::ACCEPT_MS`, since raised to 2 s to leave room for the
// five-second gesture above it.



/// §3's init sequence, in the datasheet's order.
pub fn configure(
    sensor: &As3935,
    i2c: &mut I2cDriver<'_>,
    location: Location,
) -> Result<defence::Point, esp_idf_hal::sys::EspError> {
    // **The antenna is trimmed inside `power_up`, before the calibration**,
    // which is the datasheet's order (v1-04 page 36). It was the last write of
    // this init until 0.22.0, half a second after the oscillators had been
    // calibrated against an LC oscillator still at 0 pF.
    sensor.power_up(i2c, TUNING_CAPS_PF)?;

    // **Did the oscillators calibrate?** Read straight after `power_up`, which
    // is the calibration. Reported and nothing more: a failure here would
    // explain disturbers-but-never-lightning, and the chip has been saying
    // whether it happened on every boot without anyone asking. See
    // `As3935::calibration_status` for what 0.21.2 read, which is what this
    // line is to be compared with.
    match sensor.calibration_status(i2c) {
        Ok(((trco, trco_raw), (srco, srco_raw))) => println!(
            "as:   RCO calibration -- TRCO {}, SRCO {} (0x3A=0x{trco_raw:02X} 0x3B=0x{srco_raw:02X})",
            trco.label(),
            srco.label()
        ),
        Err(e) => println!("as:   RCO calibration status unreadable -- {e}"),
    }

    sensor.set_location(i2c, location)?;

    // **Disturbers are MASKED, and the reason they were not has expired.**
    //
    // For most of this device's life the comment here said disturbers had to
    // stay enabled because §4.2's noise auto-tune was driven by them. That was
    // true of an earlier tuner and is not true of this one: `Tuning::observe`
    // folds `batch.noise` alone into the verdict, and its own note says why --
    // `NF_LEV` gates the noise floor and cannot touch a disturber, so climbing
    // on disturbers spent the ladder's whole range on a question the knob does
    // not answer. Masking suppresses `INT_D` only; `INT_NH` still raises the
    // IRQ, and `INT_NH` is the tuner's entire input. Nothing goes blind.
    //
    // What masking buys was measured here on 2026-09-10, in a live storm, at
    // `nf 1` / `wd 0` / outdoor gain, one audible strike every thirty seconds:
    //
    // ```text
    // unmasked:  1 strike, 380 disturbers, several missed edges  (95 s)
    // masked:    2 strikes,  0 disturbers, 0 missed edges        (95 s)
    // ```
    //
    // The missed edges are the mechanism. At four disturbers a second every one
    // raises the IRQ, and the poll then finds events with the edge already
    // gone -- so the flood is not merely noise in the log, it is contention on
    // the one wire a *strike* also has to arrive over. Twice the strikes is
    // what falls out of not spending the interrupt path on returns the chip has
    // already classified as not-lightning.
    //
    // **That mechanism is gone since 0.21.0, and the contention is not.** The
    // IRQ is a level now, so an INT still high when the loop re-arms fires at
    // once and no edge can be missed. What unmasking still costs is the one
    // register a strike and a disturber share: the AS3935 shows only the
    // latest reason, so a disturber that lands before a pending strike is read
    // -- inside the settle, or behind a 3.8 s redraw -- replaces it, and at
    // 6-10 a second a redraw is long enough to expect that. And on the cell
    // each disturber is a wake of its own (`power::wake_on_irq`).
    //
    // The cost is real and is the disturber log: `mask off` is how to run the
    // "are real flashes arriving here as disturbers" experiment, and that
    // question is still open. It is an experiment, so it is a command.
    //
    // **And since 0.21.0 the command's answer survives a restart.** Every open
    // of the console port resets the board, so a `mask off` held only in RAM
    // ended the moment anybody connected to watch it. Masked remains what a
    // device never told starts as; whatever was set last is what it resumes,
    // and the banner says which, because an unmasked device on battery is an
    // experiment somebody may have forgotten they started.
    let masked = settings::mask_disturbers().unwrap_or(true);
    sensor.set_disturber_enabled(i2c, !masked)?;
    session::DISTURBERS_MASKED.store(masked, core::sync::atomic::Ordering::Relaxed);
    match (masked, settings::mask_disturbers().is_some()) {
        (true, false) => println!("as:   disturbers masked -- the default"),
        (true, true) => println!("as:   disturbers masked (saved)"),
        (false, _) => {
            println!("as:   ⚠ disturbers UNMASKED (saved by `mask off`) -- they raise the IRQ");
            println!("as:     a disturber can replace a strike's reason before it is read, and on the cell");
            println!("as:     each one is a wake; `mask on` to restore");
        }
    }
    sensor.clear_irq_display(i2c)?;
    FreeRtos::delay_ms(500);

    // **And that is the end of the init, deliberately.**
    //
    // The reference's whole configuration is the six calls above — power up,
    // indoor/outdoor, disturbers on, IRQ source, 500 ms, tuning caps — with
    // the tuning caps moved to the front in 0.22.0, into `power_up`, where the
    // datasheet wants them: before the calibration, not after it. It never
    // writes `NF_LEV`, `WDTH`, `SREJ` or `MIN_NUM_LIGH` at start-up, and never
    // writes the last three at all. That is the configuration which has actually
    // detected lightning on this hardware.
    //
    // Two calls used to live here and have been removed:
    //
    // * `apply_defence(0)`, which wrote all three registers and in particular
    //   forced **`SREJ` from its power-on 2 down to 0**. Spike rejection is part
    //   of the validation chain rather than a sensitivity knob, so "lower is
    //   more sensitive" was the wrong model — and no configuration that ever
    //   detected a strike had it anywhere but the default.
    // * `set_min_strikes(1)`, which is a no-op against a chip that powers up at
    //   one strike, and was therefore paying a bus transaction to state a
    //   preference the datasheet already guarantees.
    //
    // What the runtime still does is exactly what the reference does: walk
    // `NF_LEV` between 0 and 7 and touch nothing else (§4.2).
    let min_strikes = sensor.min_strikes(i2c)?;

    // **Every boot starts at the most sensitive noise floor**, written
    // explicitly so the chip and the ladder's idea of it cannot disagree.
    //
    // The chip powers up at `NF_LEV` 2, and leaving it there meant a `level`
    // variable of 0 describing hardware set to 2 -- so the first "climb" would
    // have written a *lower* floor than was already in force, making the
    // receiver more sensitive in response to noise. Reading the chip back would
    // fix the disagreement; starting from a known floor fixes it *and* makes
    // every session begin identically, which is worth more on a device whose
    // whole difficulty has been telling configuration apart from environment.
    //
    // Nothing is persisted. The ladder re-finds the room's noise floor within a
    // few minutes, so storing it would buy a few minutes of accuracy at the cost
    // of a flash write on a value that changes with the weather.
    //
    // `NF_LEV` is the one register the reference tunes, so writing it here is
    // its first tweak brought forward, not a new deviation -- unlike `WDTH` and
    // `SREJ`, which stay untouched at their power-on defaults.
    // Resume where this room left off, rather than climbing from zero again.
    //
    // **A combination that has heard lightning outranks the one the tuner
    // happened to stop at.** The stored point is wherever the walk was when
    // power went; if the device had been driven deaf, that is the deafness it
    // resumes, and it resumes it in silence. The golden record is the one
    // setting this room is *measured* to produce strikes at, so when the two
    // disagree and the record is trusted, the measurement wins.
    //
    // Only when the record's gain matches: a noise floor learned indoors
    // describes a front end seeing roughly four times what the outdoor one
    // does, and carrying it across is how a good point becomes a deaf one.
    let here_outdoor = matches!(settings::location(), Some(crate::as3935::Location::Outdoor));
    let here = settings::golden().filter(|record| record.combo.outdoor == here_outdoor);

    // **A record from before 0.22.0 is not one of these.** It was counted with
    // `overhead` as lightning, so it is read only for display -- see
    // `settings::legacy_golden`. Said at boot when it is the only record there
    // is, because that is the boot whose start point it would once have set.
    if settings::golden().is_none() {
        if let Some(old) = settings::legacy_golden() {
            println!(
                "gold: the record from before 0.22.0 (nf {} wd {} sr {}, {} strike(s)) counted `overhead`",
                old.combo.nf, old.combo.wdth, old.combo.srej, old.strikes
            );
            println!("gold:   as lightning -- set aside; the next strike with a distance starts a new one");
        }
    }
    let known_good = here.filter(|record| record.strikes >= crate::golden::TRUSTED_STRIKES);

    // **An untrusted record still outranks a point that is merely more open.**
    // A record with one strike is not evidence a setting hears *lightning* --
    // that is why `TRUSTED_STRIKES` is two -- but it is the only evidence in the
    // device that anything was ever heard at all, and the stored point carries
    // none. Resuming below it is how the tuner deadlocks: the second strike can
    // only be earned at the record's own noise floor, and a boot that starts
    // deafer than that never earns it. Measured here on 2026-09-07 with a storm
    // overhead: golden `nf 1` with one strike, stored `nf 0`, seven disturbers
    // and no strikes in ninety seconds.
    //
    // A floor, not a promotion. Starting *less* open cannot admit more man-made
    // impulses than starting at the bottom would, and the tuner is still free to
    // walk wherever the noise takes it.
    let floor = here.map(|record| defence::Point::new(record.combo.nf as u16));

    let point = match (known_good, settings::defence_point()) {
        (Some(record), stored) => {
            let golden = defence::Point::new(record.combo.nf as u16);
            match stored {
                // **Deafer than the record, not more open than it.** This read
                // `stored <= golden` and called that "learned something
                // better", which assumes the ladder is monotonic in
                // sensitivity. It is not: the bottom is the swamped end, and
                // nf 0 has been measured here at 300-480 noise/min with zero
                // strikes. So a stored point *below* the record is not an
                // improvement on it, it is the deafness this whole module
                // exists to escape.
                //
                // Measured 2026-09-09, after a power cut: golden had earned its
                // trust at nf 1 with three strikes, and boot resumed to nf 0
                // anyway -- through this arm, which the promotion had just made
                // reachable. Above the record the tuner really has learned
                // something: the room is noisier than it was, and that is worth
                // keeping.
                Some(stored) if stored.raw() >= golden.raw() => {
                    println!("as:   resumed defence point from NVS");
                    stored
                }
                _ => {
                    println!(
                        "as:   starting at nf {} -- {} strike(s) have been heard here at it",
                        golden.raw(),
                        record.strikes
                    );
                    golden
                }
            }
        }
        (None, Some(stored)) => match floor {
            // The stored point is deafer than, or level with, the one that has
            // heard something: nothing to raise it to.
            Some(heard) if stored.raw() >= heard.raw() => {
                println!("as:   resumed defence point from NVS");
                stored
            }
            Some(heard) => {
                println!(
                    "as:   NVS had nf {}, raising to nf {} -- lightning has been heard there",
                    stored.raw(),
                    heard.raw()
                );
                heard
            }
            None => {
                println!("as:   resumed defence point from NVS");
                stored
            }
        },
        (None, None) => {
            // Not `OPEN`. A never-calibrated device booting fully open drowns --
            // measured here at 7-9 noise events per batch, continuously -- and
            // the +/-1 walk needs about a thousand windows to climb out of it.
            // `default_start` opens mid-range on the two volume knobs and leaves
            // the two that decide whether a strike is reported at all untouched.
            println!("as:   no stored defence point -- starting from the default");
            defence::Point::default_start()
        }
    };
    session::apply(sensor, i2c, point)?;

    // **Written here and nowhere else on the hot path.** Spike rejection is a
    // setting rather than part of the point, so `apply` no longer touches it --
    // which is the whole fix: while the tuner could write it, it walked it to
    // zero on every quiet spell and the chip began reporting man-made impulses
    // as lightning.
    // **A trusted record carries all three of its settings, not just `nf`.**
    // `golden` stores the whole combination that produced lightning here --
    // noise floor, watchdog and spike rejection -- and until now boot restored
    // only the noise floor. The other two came from separate NVS settings that
    // nothing reconciled against the record, so the device could boot knowing
    // the right watchdog and not using it.
    //
    // Measured 2026-09-10 during a live storm, one audible strike every thirty
    // seconds: at the stored `wd 2` it logged **0 strikes and 11 disturbers in
    // 100 s**; at the record's own `wd 0`, **2 strikes and 294 disturbers in
    // 90 s**. The watchdog gates on amplitude, so it was discarding the
    // returns before anything could classify them -- and `defence`'s own
    // weighting says `wd` carries 80 % of the harm against `nf`'s 20 %.
    //
    // **The more permissive of the two wins, never the deafer.** A person who
    // has set something more sensitive by hand keeps it; a stored value deafer
    // than the measured one is overruled. The rule is
    // `defence::spike_rejection_at_boot`, host-tested, and it is where a
    // record made at `srej 0` is kept from reopening the scar that took spike
    // rejection out of the tuner's reach -- this comment claimed that guard
    // before anything enforced it.
    //
    // **Only a record counted since 0.22.0 reaches it.** `known_good` comes
    // from `settings::golden`, which does not read the record from before, so
    // that record's `sr 1` cannot drag a board back down: see
    // `settings::legacy_golden`.
    let heard = known_good.map(|record| record.combo);
    let stored_spike = crate::settings::spike_rejection();
    let spike = defence::spike_rejection_at_boot(stored_spike, heard.map(|combo| combo.srej));
    if spike < defence::SPIKE_REJECTION_AUTO_MIN {
        println!("as:   ⚠ spike rejection is 0 -- set by hand, not chosen by the device.");
        println!(
            "as:     man-made impulses validate as lightning here; `srej {}` restores the default.",
            defence::SPIKE_REJECTION_DEFAULT
        );
    }
    // **A stored value below the default is left alone, and said every boot.**
    // The default became the chip's own 2 in 0.22.0; a board told `srej 1`
    // before that keeps 1, because only a person writes this setting and the
    // device does not overrule one -- LM is to judge 1 against 2 in the field,
    // and a firmware that moved it quietly would be a second variable in every
    // comparison across the flash. See `defence::SPIKE_REJECTION_DEFAULT`.
    //
    // **And when the default offered here would last.** The device does not
    // overrule a person on its own account, but a trusted record is a
    // measurement and does, at boot: once two strikes with a distance have
    // made a record at this value, a `srej 2` typed afterwards is undone at
    // the next restart, which is every open of the console port. So the
    // order matters for a 1-against-2 comparison, and this is where it is
    // said -- the line that offers the default is the one being acted on.
    if defence::spike_rejection_below_default(stored_spike) && Some(spike) == stored_spike {
        println!(
            "as:   ⚠ spike rejection {spike} -- saved by `srej {spike}`, below the default of {} since 0.22.0",
            defence::SPIKE_REJECTION_DEFAULT
        );
        println!(
            "as:     kept, because a person set it; `srej {}` takes the default (the chip's own)",
            defence::SPIKE_REJECTION_DEFAULT
        );
        let record_holds_it = match heard {
            Some(combo) => combo.srej <= spike,
            None => false,
        };
        match record_holds_it {
            true => println!(
                "as:     a trusted record heard lightning at {spike}: `golden clear` first, or the next restart brings {spike} back"
            ),
            false => println!(
                "as:     type it before two strikes with a distance arrive at {spike}, or the record they make brings {spike} back at every restart"
            ),
        }
    }
    // What the setting would have been without the record, to say when the
    // record lowered it -- the same courtesy the watchdog line below pays.
    let unheard = defence::spike_rejection_for_autorun(stored_spike);
    match sensor.set_spike_rejection(i2c, spike) {
        Ok(()) => {
            session::SPIKE_REJECTION.store(spike, core::sync::atomic::Ordering::Relaxed);
            match spike < unheard {
                true => {
                    println!(
                        "as:   spike rejection {spike} -- lowered from {unheard}, lightning with a distance has been heard at it"
                    );
                    println!("as:     `golden clear` keeps {unheard} from the next restart");
                }
                false => println!("as:   spike rejection {spike}"),
            }
        }
        Err(e) => println!("as:   could not set spike rejection -- {e}"),
    }
    // The watchdog is written by `session::apply` above, on every apply -- but
    // it is reported here, because a setting the tuner can no longer reach is
    // one a person has to be able to see without asking for it. It left the
    // search space for a sharper reason than spike rejection did: it gates on
    // amplitude, so the tuner spending it cost the distant strikes this device
    // exists to report before the thunder arrives.
    let stored_watchdog = crate::settings::watchdog().unwrap_or(defence::WATCHDOG_DEFAULT);
    let watchdog = match heard {
        Some(combo) => stored_watchdog.min(combo.wdth),
        None => stored_watchdog,
    };
    match heard {
        Some(combo) if combo.wdth < stored_watchdog => println!(
            "as:   watchdog threshold {watchdog} -- lowered from {stored_watchdog}, \
             lightning has been heard at it"
        ),
        _ => println!("as:   watchdog threshold {watchdog}"),
    }
    if let Err(e) = sensor.set_watchdog_threshold(i2c, watchdog) {
        println!("as:   could not set the watchdog threshold -- {e}");
    }
    // `reset` issues PRESET_DEFAULT, which restores registers -- the datasheet
    // treats discarding the statistics as a separate operation, so until this
    // line no power cycle in the device's life had ever cleared them.
    session::restart_statistics(sensor, i2c, "boot");

    println!(
        "as:   {}, {} pF, defence {}/{} ({}%), report after {} strike(s)",
        location.label(),
        TUNING_CAPS_PF,
        point.raw(),
        defence::MAX,
        point.percent(),
        min_strikes
    );
    Ok(point)
}

/// The event loop: batch what arrives, then tune once per batch (§4.2).


/// Drive the IRQ pin from the antenna oscillator and count the edges.
///
/// **This answers a question that silence cannot.** A lightning detector that
/// reports nothing is indistinguishable from one whose IRQ is on the wrong pad —
/// both look like a quiet day, and the second only reveals itself during the
/// storm you built the thing for. §2's own pin table says GPIO21 while §2.1's
/// diagram and §10's build order say GPIO20, so the ambiguity is real.
///
/// With `DISP_LCO` set the sensor puts LCO ÷ 16 on the pin — roughly 31 kHz. So
/// counting transitions for a fixed window says two things at once:
///
/// * **zero edges** → the pin is not connected to this sensor;
/// * **the rate** → the antenna's resonant frequency, which §3 step 5 otherwise
///   defers to a scope. It should be 500 kHz ±3.5 %.
///
/// Polled rather than interrupt-driven on purpose: 31 000 interrupts a second
/// would swamp the notifier this loop shares with the button, and the whole
/// measurement is over in a tenth of a second.
pub fn antenna_self_test(
    sensor: &As3935,
    i2c: &mut I2cDriver<'_>,
    irq: &PinDriver<'_, esp_idf_hal::gpio::Input>,
) -> (bool, u32) {
    const WINDOW_MS: u32 = 100;

    if let Err(e) = sensor.set_irq_display_lco(i2c) {
        println!("self: could not enable LCO output -- {e}");
        return (false, 0);
    }

    // Settle: the pin has just changed function, and the first transitions
    // after that are not the oscillator.
    FreeRtos::delay_ms(10);

    let started = unsafe { esp_idf_hal::sys::esp_timer_get_time() };
    let window_us = (WINDOW_MS as i64) * 1000;
    let mut edges: u32 = 0;
    let mut previous = irq.is_high();

    while unsafe { esp_idf_hal::sys::esp_timer_get_time() } - started < window_us {
        let now = irq.is_high();
        if now && !previous {
            edges += 1;
        }
        previous = now;
    }

    if let Err(e) = sensor.clear_irq_display(i2c) {
        println!("self: could not restore the IRQ pin -- {e}");
    }

    if edges == 0 {
        println!("self: *** NO EDGES on the IRQ pin while the sensor drove it at ~31 kHz ***");
        println!("self:     The IRQ wire is not on GPIO21 (D6). The other candidate is");
        println!("self:     GPIO20 (D7), on the opposite side of the board.");
        println!("self:     Until this passes, 'no strikes' means nothing.");
        return (false, 0);
    }

    // edges over WINDOW_MS -> Hz on the pin -> x16 for the antenna, in kHz.
    let pin_hz = edges * 1000 / WINDOW_MS;
    let antenna_khz = pin_hz * as3935::LCO_DIVISOR / 1000;

    // Tolerance is carried in tenths of a percent to stay in integers.
    let tolerance_khz =
        as3935::ANTENNA_NOMINAL_KHZ * as3935::ANTENNA_TOLERANCE_PERCENT / 1000;
    let low = as3935::ANTENNA_NOMINAL_KHZ - tolerance_khz;
    let high = as3935::ANTENNA_NOMINAL_KHZ + tolerance_khz;

    println!("self: {edges} edges in {WINDOW_MS} ms -> antenna {antenna_khz} kHz");
    if (low..=high).contains(&antenna_khz) {
        println!("self: IRQ wire confirmed, antenna in tune ({low}-{high} kHz)");
    } else {
        println!("self: IRQ wire confirmed, but antenna is OUT OF TUNE");
        println!("self:     wanted {low}-{high} kHz -- adjust TUNING_CAPS_PF (§3 step 5)");
    }
    (true, antenna_khz)
}





