//! When the panel is redrawn, and what goes on it.
//!
//! **The refresh policy and the chart scratch, in one place.** Both were loop
//! locals in `main`, and neither is anything the rest of the loop should be able
//! to reach: `drawn` is meaningless outside the change test, and the two chart
//! buffers are 96 entries each held across iterations purely so a redraw does
//! not allocate on its way in.
//!
//! `ui` renders; this decides *whether* to render. The split matters because the
//! decision is the expensive half: a full 800×480 refresh measures 3.9 s on this
//! panel, so a policy that says yes too often costs more than any drawing bug.
//!
//! Hardware is passed in rather than held. [`Screen`] is the state that survives
//! between iterations; the panel, the sensor readings and the history rings all
//! belong to whoever owns them.

use crate::session::{Drawn, Totals};
use crate::{battery, clock, defence, display, history, power, system, ui};
use crate::as3935::Location;

/// Shortest gap between panel refreshes.
///
/// **A full 800×480 refresh measured 3.9 s on this panel.** §6's nominal 5 s
/// cadence would leave it busy roughly 80 % of the time — the device would
/// spend its life redrawing, and during a storm every refresh would be stale
/// before it finished. So the screen is change-gated with a floor under it,
/// and this is the floor.
const REDRAW_MIN_GAP_S: u32 = 30;

/// Redraw even if nothing tracked has changed, at most this often.
///
/// The backstop for everything the change test deliberately ignores — the
/// uptime, the disturber count, the battery, the die temperature. Without it
/// those fields would be correct only at boot, which is exactly how "up 0m" was
/// still on the glass long after boot.
///
/// Five minutes rather than fifteen: at 3.8 s a refresh that is ~1.3 % of the
/// panel's time, which is affordable, and it keeps every slow-moving field on
/// screen within a period short enough that nobody mistakes it for frozen.
const REDRAW_BASELINE_S: u32 = 5 * 60;

/// Everything the status screen reads, gathered for one call.
///
/// A borrowed view rather than fourteen arguments — and read-only on purpose.
/// The one thing a redraw used to *write* is the learned battery range, which
/// stays with its own island; see [`Screen::due`] for where that now happens.
pub struct View<'a> {
    pub location: Location,
    pub point: defence::Point,
    pub totals: &'a Totals,
    pub history: &'a history::History,
    pub battery: Option<battery::Reading>,
    pub trend: Option<&'a battery::Trend>,
    pub range: (u16, u16),
    pub drain: battery::Drain,
    pub die_temperature: Option<&'a system::DieTemperature>,
    /// `(free, used)` bytes of the strike log, or `None` without one.
    pub log_bytes: Option<(u32, u32)>,
    pub antenna_khz: u32,
    pub irq_confirmed: bool,
}

/// What the glass currently shows, and the scratch used to put it there.
pub struct Screen {
    /// The subset of state the last redraw was made from. The change test
    /// compares against this and nothing else.
    drawn: Option<Drawn>,
    last_draw_ms: u32,
    /// Set by a button press, cleared by the redraw it causes. A deliberate act
    /// bypasses the rate limit; see [`Screen::due`].
    pub user_acted: bool,
    pub period: ui::ChartPeriod,
    /// Scratch for the chart series, sized for the longest ring so one buffer
    /// serves all three periods — the shorter ones use a prefix.
    ///
    /// Held across iterations rather than built per redraw: the day ring alone
    /// is 96 buckets, and a redraw already costs 3.8 s of panel time without
    /// allocating on the way in.
    counts: [u16; history::MEDIUM_LEN],
    scores: [u32; history::MEDIUM_LEN],
}

impl Screen {
    pub fn new() -> Screen {
        Screen {
            drawn: None,
            // Zero rather than `now_ms()`, so the very first iteration draws
            // immediately instead of waiting out the floor. A device showing its
            // splash for the first thirty seconds reads as one that hung.
            last_draw_ms: 0,
            user_acted: false,
            period: ui::ChartPeriod::Day,
            counts: [0u16; history::MEDIUM_LEN],
            scores: [0u32; history::MEDIUM_LEN],
        }
    }

    /// Whether to redraw, and the reason to log if so.
    ///
    /// **Change-gated, with a floor and a backstop.** The panel takes ~3.9 s per
    /// refresh, so "redraw when anything might have changed" is not an option —
    /// it would be busy most of the time and every image would be stale before
    /// it finished.
    ///
    /// **A button press bypasses the floor.** The 30 s limit exists to stop the
    /// panel being pinned by things that change on their own; a person pressing
    /// the only button on the device is not one of those. Waiting it out made a
    /// press taken shortly after any other redraw appear to do nothing for half
    /// a minute, which reads as a broken button rather than as a considered
    /// refresh policy. The worst case is bounded anyway: `show` blocks for the
    /// whole refresh, so a second press during one is handled after it rather
    /// than queued on top of it.
    pub fn due(&self, want: &Drawn, now_ms: u32) -> Option<&'static str> {
        let since_draw_s = now_ms.saturating_sub(self.last_draw_ms) / 1000;
        let changed = self.drawn.as_ref() != Some(want);
        let stale = since_draw_s >= REDRAW_BASELINE_S;
        let allowed = self.user_acted || since_draw_s >= REDRAW_MIN_GAP_S;

        if !allowed || !(changed || stale) {
            return None;
        }
        match changed {
            true => Some("content changed"),
            false => Some("baseline"),
        }
    }

    /// Render and push one status screen.
    pub fn draw(
        &mut self,
        panel: &mut display::Panel<'_>,
        view: &View<'_>,
        why: &str,
        now_ms: u32,
    ) {
        // Flatten the ring just before drawing it, so the chart shows the state
        // at draw time rather than whenever it last changed.
        let (len, _capacity) = self.fill(view.history);

        let status = ui::Status {
            location: view.location,
            health: system::health(view.die_temperature, view.log_bytes),
            battery: view.battery,
            battery_flow: match view.battery {
                Some(reading) => battery::flow(&reading, view.trend),
                None => battery::Flow::Unknown,
            },
            now: clock::now(),
            uptime_minutes: now_ms / 60_000,
            antenna_khz: view.antenna_khz,
            irq_confirmed: view.irq_confirmed,
            defence_level: view.point.percent(),
            defence_max: 100,
            noise_per_min: view.totals.noise_per_min,
            strikes_total: view.totals.strikes,
            disturbers_per_min: view.totals.disturbers_per_min,
            last_hour: view.history.last_hour(),
            battery_range: view.range,
            battery_drain: view.drain,
            chart_period: self.period,
            recent: &view.history.recent,
            chart_scores: &self.scores[..len],
            light_sleep: power::config().map(|(_, _, ls)| ls).unwrap_or(false),
        };

        let mut frame = display::Panel::frame();
        ui::status(&mut frame, &status);

        let started = now_ms;
        match panel.show(&frame) {
            Ok(0) => println!("epd:  *** sent, but BUSY never fell -- nothing was drawn ***"),
            Ok(busy_ms) => println!(
                "epd:  redrawn ({why}) -- {} ms total, panel busy {} ms",
                crate::now_ms().saturating_sub(started),
                busy_ms
            ),
            Err(e) => println!("epd:  draw FAILED -- {e}"),
        }
    }

    /// Forget what is on the glass, forcing the next check to redraw.
    ///
    /// For commands that change something the change test cannot see — a chart
    /// period, say, which alters the whole lower half of the screen without
    /// touching any field in [`Drawn`].
    pub fn invalidate(&mut self) {
        self.drawn = None;
    }

    /// Record what was just drawn, so the change test has something to compare.
    pub fn mark_drawn(&mut self, want: Drawn, now_ms: u32) {
        self.drawn = Some(want);
        self.last_draw_ms = now_ms;
        self.user_acted = false;
    }

    /// Flatten the ring for the current period into the scratch buffers.
    ///
    /// Returns `(filled, capacity)` — how much of the buffer holds live data,
    /// and how many buckets the ring can hold. The chart needs both: the second
    /// fixes the column width so bars keep their size as the ring fills.
    fn fill(&mut self, history: &history::History) -> (usize, usize) {
        // One generic helper rather than three arms differing only in a const:
        // the rings have different lengths but a chart does the same thing to
        // all of them, and three near-identical blocks is three places to fix a
        // bug.
        fn flatten<const N: usize>(
            ring: &history::Ring<N>,
            counts: &mut [u16],
            scores: &mut [u32],
        ) -> (usize, usize) {
            let mut c = [0u16; N];
            let mut s = [0u32; N];
            let live = history::series_of(ring, &mut c, &mut s);
            counts[..N].copy_from_slice(&c);
            scores[..N].copy_from_slice(&s);
            (live, N)
        }

        match self.period {
            ui::ChartPeriod::Day => flatten(&history.day, &mut self.counts, &mut self.scores),
            ui::ChartPeriod::Week => flatten(&history.week, &mut self.counts, &mut self.scores),
            ui::ChartPeriod::Month => flatten(&history.month, &mut self.counts, &mut self.scores),
        }
    }
}
