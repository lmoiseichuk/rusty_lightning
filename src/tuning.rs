//! §4.2's noise decision: one window, one verdict, one step.
//!
//! **The island `listen` used to spread across nine locals** — the point, the
//! sweep, the quiet threshold, three window counters, the window's start, what
//! is on flash, and when it was last written. They only ever move together, and
//! every bug this subsystem has produced came from one of them disagreeing with
//! another: a sweep that used a different notion of "quiet" than the walk, a
//! counter zeroed on one path and not the other, a point written to the chip but
//! not to the struct that claimed to describe it.
//!
//! [`Tuning`] owns all nine and exposes the four things the loop actually does:
//! [`observe`](Tuning::observe) a batch, ask whether the window is
//! [`due`](Tuning::due), take a [`step`](Tuning::step), and occasionally be told
//! to do something by hand.
//!
//! Hardware is passed in, never held — the same boundary `session` draws, and
//! what keeps `defence` host-testable.

use esp_idf_hal::delay::FreeRtos;
use esp_idf_hal::i2c::I2cDriver;

use crate::as3935::As3935;
use crate::session::{Batch, Totals};
use crate::{defence, session, settings};

/// The one measurement window (§4.2).
///
/// **A single period for everything**: the noise level is reconsidered once per
/// window, and the same window's counts are what the screen reports. One
/// constant rather than three, so the tuner can never be deciding on evidence
/// the display is not showing.
///
/// **Sixty seconds**, matching the calibration probe window. The ±1 walk that
/// runs between calibrations is judging the same question a probe judges — "did
/// that window hear anything" — so it deserves the same span. At ten seconds it
/// was deciding on a sample too short to distinguish a quiet room from a gap
/// between events, and stepping the point every ten seconds on that evidence.
pub const MEASURE_INTERVAL_S: u32 = 60;

/// How often the learned tuning point may be written back to NVS.
///
/// Fifteen minutes, matching `clock::SAVE_INTERVAL_S` and for the same reason:
/// the value it protects re-learns in minutes, so a power cut costs almost
/// nothing while a write every window would cost flash endurance for real. A
/// settled room stops moving the point at all, and then this writes once and
/// never again.
const DEFENCE_SAVE_S: u32 = 15 * 60;

/// How often to stop defending and simply listen.
///
/// **The device cannot detect what it is deaf to, and that is not a figure of
/// speech.** On 2026-08-19 it sat at 117/127 through the opening of a storm
/// directly overhead: no strikes, so nothing to hold for, so nothing to stop it
/// staying exactly where it was. It took three commands typed by a person to
/// make it hear. Every other rule here reacts to strikes, and none of them can
/// fire when the setting itself is what prevents one.
///
/// So once every ten minutes it spends one window fully open and finds out.
const DIP_INTERVAL_S: u32 = 10 * 60;

/// Strikes in a dip window before the weather is believed.
///
/// **One.** A dip is already the rare case, and a storm that is starting is
/// exactly when being slow is most expensive -- the opening strikes are the
/// warning the whole device exists to give. The cost of believing a single
/// event is a false alarm from something man-made, which lasts until the next
/// window judges the band and walks the point back.
const DIP_STRIKES_TO_BELIEVE: u32 = 1;

/// Everything the noise decision needs to remember between windows.
pub struct Tuning {
    /// Where the chip is set now.
    pub point: defence::Point,
    /// What is on flash, so a write only happens when the point has actually
    /// moved away from it.
    stored: defence::Point,
    last_save_s: u32,
    /// A calibration in progress, driven one probe per window. `None` when the
    /// ±1 walk is in charge.
    sweep: Option<session::Sweep>,
    /// **The zero both the search and the ±1 walk compare against.** One field
    /// rather than two constants, so they can never disagree about what a quiet
    /// window is — which is exactly how a 60 s sweep came to settle deafer than
    /// a 10 s one in the same room. See [`session::QUIET_PER_MIN`].
    quiet_per_min: u32,
    /// Where to go back to when a dip hears nothing. `Some` while dipping.
    dip_restore: Option<defence::Point>,
    /// When the last dip ran, so they are spaced rather than continuous.
    last_dip_ms: u32,
    events: u32,
    disturbers: u32,
    /// Strikes in the current window, which **veto** a climb. Kept apart from
    /// `events` because that is the interference rate and has to stay a
    /// measurement of the band, not of the weather.
    strikes: u32,
    window_started_ms: u32,
    /// Set by `sensitive on`; see `session::force_max_sensitivity`. Deliberately
    /// not persisted — it is a diagnostic override for a storm happening now,
    /// and a device that silently came back from a power cut with its noise
    /// rejection disabled would be a trap.
    pub frozen: bool,
}

impl Tuning {
    pub fn new(start: defence::Point, now_ms: u32) -> Tuning {
        let quiet_per_min = match settings::quiet_per_min() {
            Some(stored) => {
                println!("as:   quiet threshold {stored}/min (from NVS)");
                stored
            }
            None => {
                println!(
                    "as:   quiet threshold {}/min (default)",
                    session::QUIET_PER_MIN
                );
                session::QUIET_PER_MIN
            }
        };
        Tuning {
            point: start,
            stored: start,
            last_save_s: now_ms / 1000,
            sweep: None,
            quiet_per_min,
            dip_restore: None,
            // Starts now, so a board that reboots mid-storm does not dip
            // immediately on top of the weather it just came up in.
            last_dip_ms: now_ms,
            events: 0,
            disturbers: 0,
            strikes: 0,
            window_started_ms: now_ms,
            frozen: false,
        }
    }

    /// Fold one batch into the window now being judged.
    pub fn observe(&mut self, batch: &Batch) {
        self.events += batch.noise + batch.disturbers;
        self.disturbers += batch.disturbers;
        self.strikes += batch.strikes;
    }

    /// How long this window runs. A sweep sets its own, so `calibrate 60` means
    /// 60 s probes even if the ordinary cadence is something else.
    pub fn window_s(&self) -> u32 {
        match self.sweep.as_ref() {
            Some(sweep) => sweep.window_s,
            None => MEASURE_INTERVAL_S,
        }
    }

    pub fn due(&self, now_ms: u32) -> bool {
        !self.frozen && now_ms.saturating_sub(self.window_started_ms) >= self.window_s() * 1000
    }

    /// Begin the window again from now, discarding what it had counted.
    ///
    /// Used whenever something outside the tuner changes the point, so the next
    /// verdict is judged on the new setting rather than on the old one's tail.
    pub fn restart(&mut self, now_ms: u32) {
        self.events = 0;
        self.disturbers = 0;
        self.strikes = 0;
        self.window_started_ms = now_ms;
    }

    /// The whole §4.2 decision for one window.
    ///
    /// Writes the window's rates into `totals` on the way through, because the
    /// screen has to report the same evidence the decision was taken on — a rate
    /// from a separate probe once read `0/min` while the tuner was visibly
    /// climbing on events it had just counted.
    pub fn step(
        &mut self,
        sensor: &As3935,
        i2c: &mut I2cDriver<'_>,
        totals: &mut Totals,
        now_ms: u32,
    ) {
        let window_s = self.window_s();
        self.window_started_ms = now_ms;

        // Scaled by the window actually used, so a 60 s probe and a 10 s window
        // both report a per-minute rate rather than a raw count. Multiply before
        // dividing, so a window that is not a whole divisor of 60 still scales
        // correctly instead of collapsing to 1.
        let per_min = |count: u32| count * 60 / window_s.max(1);
        totals.noise_per_min = per_min(self.events);
        totals.disturbers_per_min = per_min(self.disturbers);

        // **The one place "quiet" is decided.** A rate, not a count, so the
        // verdict means the same thing whatever the window length — see
        // `session::QUIET_PER_MIN` for what testing `== 0` cost.
        let quiet = totals.noise_per_min <= self.quiet_per_min;

        // Captured before the branch: a sweep that finishes inside it clears
        // itself, and the ±1 walk must still be skipped for this window rather
        // than stepping the point the search just chose.
        let sweeping = self.sweep.is_some();
        if sweeping {
            self.probe(sensor, i2c, totals, quiet, now_ms);
        }

        // A dip spent this window wide open on purpose, so the window's evidence
        // is about the weather rather than about the tuning point -- judged
        // here, and it must pre-empt the ordinary walk for the same reason a
        // sweep does.
        let dipping = self.dip_restore.is_some();

        let moved = match () {
            // The search owns this window; it has already moved the point.
            _ if sweeping => None,
            _ if dipping => self.judge_dip(sensor, i2c),
            _ if self.strikes > 0 && !quiet => {
                self.hold(totals);
                None
            }
            _ if !quiet => self.climb(totals),
            _ => self.relax(),
        };

        let heard_strikes = self.strikes > 0;

        self.events = 0;
        self.disturbers = 0;
        self.strikes = 0;

        // Programmed only when it actually moved. At either end the decision is
        // taken every window and changes nothing.
        if let Some(direction) = moved {
            session::tune(sensor, i2c, self.point, direction);
        }

        // **Only when there is nothing else going on.** A sweep owns the point
        // outright; a window that already heard strikes needs no dip, because
        // the device is plainly not deaf and `hold` is doing its job.
        if !sweeping && !dipping && !heard_strikes && self.dip_due(now_ms) {
            self.begin_dip(sensor, i2c, now_ms);
        }

        self.persist(now_ms);
    }

    /// One probe of a running sweep.
    fn probe(
        &mut self,
        sensor: &As3935,
        i2c: &mut I2cDriver<'_>,
        totals: &Totals,
        quiet: bool,
        now_ms: u32,
    ) {
        let tested = self.point;
        let events = self.events;
        // Each probe is a different receiver, so the previous probe's figures
        // cannot describe this one.
        session::restart_statistics(sensor, i2c, "next probe");
        let (finished, next) = match self.sweep.as_mut() {
            None => return,
            Some(active) => {
                active.record(quiet);
                println!(
                    "cal:  probe {} [{}..{}] {} -> {} -- {} event(s) = {}/min, {}, ~{} left",
                    active.probe,
                    active.low,
                    active.high,
                    tested.raw(),
                    session::describe(tested),
                    events,
                    totals.noise_per_min,
                    if quiet { "quiet" } else { "noisy" },
                    active.remaining()
                );
                let finished = active.done();
                let next = match finished {
                    true => active.settled(),
                    false => active.point(),
                };
                (finished, next)
            }
        };

        self.point = next;
        if let Err(e) = session::apply(sensor, i2c, self.point) {
            println!("cal:  could not program -- {e}");
        }
        // Let the new thresholds take effect before the next window opens, so a
        // probe measures its own setting rather than the tail of the previous.
        FreeRtos::delay_ms(session::CALIBRATE_SETTLE_MS);
        self.window_started_ms = crate::now_ms();

        if finished {
            println!(
                "cal:  settled at {}/{} ({}%) -- {}",
                self.point.raw(),
                defence::MAX,
                self.point.percent(),
                session::describe(self.point)
            );
            match settings::store_defence_point(self.point) {
                Ok(()) => {
                    self.stored = self.point;
                    self.last_save_s = now_ms / 1000;
                    println!("cal:  point stored -- +/-1 from here");
                }
                Err(e) => println!("cal:  point NOT stored -- {e}"),
            }
            self.sweep = None;
        }
    }

    /// **A window that heard a strike never raises the defence.**
    ///
    /// A nearby strike is not a clean impulse: it throws harmonics that arrive
    /// as disturbers, so a storm close enough to matter looks like a noisy band
    /// to a counter that cannot tell them apart. Climbing on that would deafen
    /// the device at exactly the moment it exists for — and each notch of
    /// `MIN_NUM_LIGH` then hides the following strikes too, which is a loop that
    /// closes on itself.
    ///
    /// Holding rather than relaxing: the window genuinely was noisy, so this is
    /// a refusal to escalate, not evidence of quiet.
    fn hold(&self, totals: &Totals) {
        println!(
            "tune: holding at {}/{} ({}%) -- {} strike(s) this window, {}/min",
            self.point.raw(),
            defence::MAX,
            self.point.percent(),
            self.strikes,
            totals.noise_per_min
        );
    }

    /// **Proportional: how many notches, from how far over the line.**
    ///
    /// One notch a window is a rate, and it has to answer a range of rates.
    /// Measured here, a microwave door swing took the band from 6/min to 94/min;
    /// at one notch a minute the machine answers that by fiddling the bottom
    /// bits while the watchdog — the knob that would actually stop it — sits
    /// untouched for eight minutes. Observed doing exactly that at 102/min:
    /// `323 → 324 → 325`, thrashing min strikes 3 → 0 → 1.
    ///
    /// Dividing by the quiet threshold makes the step mean "how many times over
    /// the line is this", which needs no separate constant and scales with
    /// whatever the room's threshold has been set to. It saturates naturally: a
    /// fully jammed band here is ~480/min, which is 40 notches against a ladder
    /// exactly 40 notches deep, so the worst case is "fully deaf in one window"
    /// rather than an unbounded number nobody has budgeted for.
    fn climb(&mut self, totals: &Totals) -> Option<&'static str> {
        let notches = (totals.noise_per_min / self.quiet_per_min.max(1)).max(1);
        let mut stepped = false;
        for _ in 0..notches {
            match self.point.tightened() {
                Some(firmer) => {
                    self.point = firmer;
                    stepped = true;
                }
                None => break,
            }
        }
        match stepped {
            true => Some("up"),
            false => None,
        }
    }

    /// **One notch. Always one.**
    ///
    /// Briefly this accelerated -- each consecutive quiet window relaxing by one
    /// more than the last -- to balance a climb that is proportional. In the
    /// 13-bit space that was reasonable arithmetic. In the 11-bit space it is
    /// not: with `MIN_NUM_LIGH` gone there are 2048 points rather than 8192, and
    /// each notch of the three that remain is correspondingly more valuable.
    /// An accelerating descent overshoots a boundary the device spent minutes
    /// finding.
    ///
    /// The asymmetry is now deliberate and one-sided: climb fast, leave slowly.
    /// A storm's first strike should not arrive into a receiver that sprinted
    /// back toward a floor it will have to climb again.
    /// Whether it is time to stop defending and listen.
    fn dip_due(&self, now_ms: u32) -> bool {
        now_ms.saturating_sub(self.last_dip_ms) >= DIP_INTERVAL_S * 1000
    }

    /// Drop to fully open for one window, remembering where to come back to.
    ///
    /// The point is programmed directly rather than walked: this is not a step
    /// in the search, it is a deliberate departure from it, and a `+-1` walk
    /// would take a hundred windows to get here and another hundred back.
    fn begin_dip(&mut self, sensor: &As3935, i2c: &mut I2cDriver<'_>, now_ms: u32) {
        self.last_dip_ms = now_ms;
        if self.point == defence::Point::new(0) {
            return; // already listening; nothing to dip to
        }
        self.dip_restore = Some(self.point);
        self.point = defence::Point::new(0);
        match session::apply(sensor, i2c, self.point) {
            Ok(()) => println!(
                "dip:  listening wide open for one window -- back to {}/{} unless something arrives",
                self.dip_restore.map(|p| p.raw()).unwrap_or(0),
                defence::MAX
            ),
            Err(e) => println!("dip:  could not program -- {e}"),
        }
    }

    /// Decide what a dip window found.
    ///
    /// **A strike is believed and kept.** Staying open is what makes the dip
    /// worth running -- the point of finding a storm is to be listening to it,
    /// and `hold` then keeps the point here for as long as strikes keep
    /// arriving, which is the mechanism that was already right.
    ///
    /// Hearing nothing restores the point that was defending before, because a
    /// dip is a question rather than a decision.
    fn judge_dip(&mut self, sensor: &As3935, i2c: &mut I2cDriver<'_>) -> Option<&'static str> {
        let restore = self.dip_restore.take();
        if self.strikes >= DIP_STRIKES_TO_BELIEVE {
            println!(
                "dip:  {} strike(s) while open -- weather, staying at {}/{}",
                self.strikes,
                self.point.raw(),
                defence::MAX
            );
            return None;
        }
        let Some(restore) = restore else { return None };
        self.point = restore;
        match session::apply(sensor, i2c, self.point) {
            Ok(()) => println!(
                "dip:  nothing heard -- defending again at {}/{}",
                self.point.raw(),
                defence::MAX
            ),
            Err(e) => println!("dip:  could not restore -- {e}"),
        }
        None
    }

    fn relax(&mut self) -> Option<&'static str> {
        match self.point.relaxed() {
            Some(gentler) => {
                self.point = gentler;
                Some("down")
            }
            None => None,
        }
    }

    /// Persist the point, rarely.
    ///
    /// The machine can move every window, and a flash write at that cadence to
    /// protect a value that re-learns in minutes would spend endurance for
    /// nothing — but a settled room stops moving, so in practice this writes
    /// once and then never.
    fn persist(&mut self, now_ms: u32) {
        // **A dip is a temporary lie about the point**, so it must never reach
        // NVS -- a power cut mid-dip would otherwise resume fully open and stay
        // there, which is the opposite of what the stored value is for.
        if self.dip_restore.is_some() {
            return;
        }
        let now_s = now_ms / 1000;
        if self.point == self.stored || now_s.saturating_sub(self.last_save_s) < DEFENCE_SAVE_S {
            return;
        }
        self.last_save_s = now_s;
        match settings::store_defence_point(self.point) {
            Ok(()) => self.stored = self.point,
            Err(e) => println!("as:   defence point NOT saved -- {e}"),
        }
    }

    /// Start a calibration sweep.
    ///
    /// `requested_quiet` of `u32::MAX` is the "not given" marker — the stored
    /// threshold stands unless the operator names a new one.
    pub fn begin_sweep(
        &mut self,
        sensor: &As3935,
        i2c: &mut I2cDriver<'_>,
        window_s: u32,
        requested_quiet: u32,
        now_ms: u32,
    ) {
        if requested_quiet != u32::MAX && requested_quiet != self.quiet_per_min {
            self.quiet_per_min = requested_quiet;
            match settings::store_quiet_per_min(self.quiet_per_min) {
                Ok(()) => println!(
                    "cal:  quiet threshold now {}/min (saved)",
                    self.quiet_per_min
                ),
                Err(e) => println!(
                    "cal:  threshold {}/min but NOT saved -- {e}",
                    self.quiet_per_min
                ),
            }
        }

        let started = session::Sweep::new(window_s);
        println!(
            "cal:  starting -- 0..={}, {} s per probe, quiet is <={}/min, about {} probes",
            defence::MAX,
            started.window_s,
            self.quiet_per_min,
            started.remaining()
        );
        self.point = started.point();
        // The sweep deliberately mis-sets the sensor while it searches, so
        // everything the estimator holds was gathered by a receiver the next
        // eleven probes will not resemble.
        session::restart_statistics(sensor, i2c, "calibration started");
        if let Err(e) = session::apply(sensor, i2c, self.point) {
            println!("cal:  could not program the first probe -- {e}");
        }
        FreeRtos::delay_ms(session::CALIBRATE_SETTLE_MS);
        self.sweep = Some(started);
        self.restart(now_ms);
    }

    /// Put the point somewhere by hand — how a room with a known answer skips
    /// the sweep.
    pub fn place(&mut self, sensor: &As3935, i2c: &mut I2cDriver<'_>, raw: u16, now_ms: u32) {
        self.point = defence::Point::new(raw);
        // A jump rather than a ±1 step: the population of events feeding the
        // estimate changes with it.
        session::restart_statistics(sensor, i2c, "point set by hand");
        match session::apply(sensor, i2c, self.point) {
            Ok(()) => println!(
                "def:  set to {}/{} ({}%) -- {}",
                self.point.raw(),
                defence::MAX,
                self.point.percent(),
                session::describe(self.point)
            ),
            Err(e) => println!("def:  could not program -- {e}"),
        }
        self.restart(now_ms);
    }

    /// Back to fully receptive — what `sensitive off` returns to.
    ///
    /// Not to wherever the point happened to be: the walk was frozen, so it is
    /// stale by however long the override was on.
    pub fn open(&mut self, sensor: &As3935, i2c: &mut I2cDriver<'_>, now_ms: u32) -> Result<(), esp_idf_hal::sys::EspError> {
        self.point = defence::Point::OPEN;
        self.restart(now_ms);
        session::restart_statistics(sensor, i2c, "sensitivity override lifted");
        session::apply(sensor, i2c, self.point)
    }

    /// One line describing where the tuner is, for the console.
    pub fn report(&self) {
        println!(
            "def:  {}/{} ({}%) -- {}, quiet <={}/min",
            self.point.raw(),
            defence::MAX,
            self.point.percent(),
            session::describe(self.point),
            self.quiet_per_min
        );
    }
}
