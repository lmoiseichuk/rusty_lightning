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

        let moved = match () {
            // The search owns this window; it has already moved the point.
            _ if sweeping => None,
            _ if self.strikes > 0 && !quiet => {
                self.hold(totals);
                None
            }
            _ if !quiet => self.climb(totals),
            _ => self.relax(),
        };

        self.events = 0;
        self.disturbers = 0;
        self.strikes = 0;

        // Programmed only when it actually moved. At either end the decision is
        // taken every window and changes nothing.
        if let Some(direction) = moved {
            session::tune(sensor, i2c, self.point, direction);
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

    /// One notch gentler. Quick to defend, slow to relax: a storm's first strike
    /// should not arrive into a receiver that spent the afternoon sprinting back
    /// toward a floor it will have to climb again.
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
