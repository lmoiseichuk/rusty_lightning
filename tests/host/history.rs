//! Host checks for the strike history rings.
//!
//! A copy of `src/history.rs`'s logic — see README.md.
//!
//! Worth testing because a ring's failures are all quiet ones. A gap that is
//! not cleared shows last week's storm as today's; an off-by-one in `series`
//! draws the chart reversed; a wrong score formula produces plausible numbers
//! that rank storms wrongly. None of it errors, and all of it needs days of
//! weather to notice on hardware.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Distance { Km(u8), Overhead, OutOfRange }

#[derive(Debug, Clone, Copy)]
pub struct Strike { pub distance: Distance, pub energy_raw: u32 }

impl Strike {
    pub fn intensity_milli(&self) -> u32 { self.energy_raw * 1000 / 16777 }
}

/// One time bucket.
///
/// `Copy` and eight bytes, so a ring of them is a plain array with no
/// allocation and no per-bucket bookkeeping.
#[derive(Debug, Clone, Copy, Default)]
pub struct Bucket {
    pub strikes: u16,
    /// Sum of per-strike scores, in thousandths.
    pub score_milli_sum: u32,
    /// Sum of distances in km, over strikes that reported one.
    pub distance_km_sum: u32,
    /// How many strikes contributed a usable distance — which is **not** the
    /// same as `strikes`. "Overhead" and "out of range" are strikes without a
    /// distance, and averaging them in as 1 km or 63 km would be inventing data.
    pub distance_samples: u16,
    /// Closest strike in this bucket, km. `u8::MAX` when none reported one.
    pub distance_km_min: u8,
}

impl Bucket {
    /// Mean score in thousandths, or `None` for an empty bucket.
    ///
    /// **The average, which §4.3 wants** — it says how severe the storm was,
    /// where the count says how busy it was. Neither substitutes for the other,
    /// which is why both are kept.
    pub fn mean_score_milli(&self) -> Option<u32> {
        (self.strikes > 0).then(|| self.score_milli_sum / self.strikes as u32)
    }

    pub fn mean_distance_km(&self) -> Option<u32> {
        (self.distance_samples > 0)
            .then(|| self.distance_km_sum / self.distance_samples as u32)
    }

    pub fn is_empty(&self) -> bool {
        self.strikes == 0
    }
}

/// §3's score: intensity divided by distance, so it rises sharply as a storm
/// closes.
///
/// Computed in **deci-kilometres** so the reference's `max(distance, 0.1 km)`
/// floor is an integer. Checked against §4.3's observed figures: a 6 km strike
/// of intensity 3 gives `3000 × 10 / 60 = 500`, i.e. 0.5 — the bottom of the
/// 0.5–1.2 range quoted there.
///
/// `Overhead` is treated as that 0.1 km floor rather than as 1 km, which is
/// what makes an overhead strike score ten times a 1 km one. `OutOfRange`
/// yields `None`: there is no distance to divide by, and substituting 63 km
/// would report the most distant possible strike as the calmest reading the
/// device can produce.
pub fn score_milli(strike: &Strike) -> Option<u32> {
    let deci_km = match strike.distance {
        Distance::Km(km) => (km as u32).max(1) * 10,
        Distance::Overhead => 1,
        Distance::OutOfRange => return None,
    };
    Some(strike.intensity_milli() * 10 / deci_km)
}

/// A fixed-size ring of buckets over a fixed bucket width.
pub struct Ring<const N: usize> {
    buckets: [Bucket; N],
    /// Bucket index of `buckets[head]`, in units of `minutes_per_bucket` since
    /// boot. Monotonic; the ring position is `index % N`.
    newest_index: u32,
    minutes_per_bucket: u32,
    started: bool,
}

impl<const N: usize> Ring<N> {
    pub const fn new(minutes_per_bucket: u32) -> Self {
        Self {
            buckets: [Bucket {
                strikes: 0,
                score_milli_sum: 0,
                distance_km_sum: 0,
                distance_samples: 0,
                distance_km_min: u8::MAX,
            }; N],
            newest_index: 0,
            minutes_per_bucket,
            started: false,
        }
    }

    /// Advance to the bucket covering `minute`, clearing anything skipped.
    ///
    /// **Clearing the gap is the whole job.** A device that is quiet for six
    /// hours and then sees a strike must not find five-hour-old data sitting in
    /// the buckets between — those buckets are being reused from a previous lap
    /// of the ring, and their contents describe a different day.
    fn advance_to(&mut self, minute: u32) {
        let index = minute / self.minutes_per_bucket;

        if !self.started {
            self.newest_index = index;
            self.started = true;
            return;
        }
        if index <= self.newest_index {
            return;
        }

        // More than a full lap skipped: everything in the ring is stale.
        if index - self.newest_index >= N as u32 {
            self.buckets = [Bucket::default(); N];
        } else {
            for skipped in (self.newest_index + 1)..=index {
                self.buckets[skipped as usize % N] = Bucket::default();
            }
        }
        self.newest_index = index;
    }

    pub fn record(&mut self, minute: u32, strike: &Strike) {
        self.advance_to(minute);
        let bucket = &mut self.buckets[self.newest_index as usize % N];

        bucket.strikes = bucket.strikes.saturating_add(1);
        if let Some(score) = score_milli(strike) {
            bucket.score_milli_sum = bucket.score_milli_sum.saturating_add(score);
        }
        if let Distance::Km(km) = strike.distance {
            bucket.distance_km_sum = bucket.distance_km_sum.saturating_add(km as u32);
            bucket.distance_samples = bucket.distance_samples.saturating_add(1);
            bucket.distance_km_min = bucket.distance_km_min.min(km);
        }
    }

    /// Keep the ring's idea of "now" current even when nothing is happening, so
    /// a chart drawn during a lull shows the lull rather than the last storm
    /// pushed up against the right edge.
    pub fn tick(&mut self, minute: u32) {
        self.advance_to(minute);
    }

    /// Buckets oldest-first, for a chart. Length is always `N`.
    pub fn series(&self, into: &mut [Bucket; N]) {
        for offset in 0..N {
            // `newest_index + 1` is the oldest slot: one past the newest, which
            // is where the ring wraps to.
            let index = self.newest_index as usize + 1 + offset;
            into[offset] = self.buckets[index % N];
        }
    }

    /// Totals over the most recent `buckets` entries, newest first.
    pub fn recent(&self, buckets: usize) -> Bucket {
        let mut total = Bucket {
            distance_km_min: u8::MAX,
            ..Default::default()
        };
        for back in 0..buckets.min(N) {
            let index = self.newest_index as usize + N - back;
            let bucket = self.buckets[index % N];
            total.strikes = total.strikes.saturating_add(bucket.strikes);
            total.score_milli_sum = total.score_milli_sum.saturating_add(bucket.score_milli_sum);
            total.distance_km_sum = total.distance_km_sum.saturating_add(bucket.distance_km_sum);
            total.distance_samples =
                total.distance_samples.saturating_add(bucket.distance_samples);
            total.distance_km_min = total.distance_km_min.min(bucket.distance_km_min);
        }
        total
    }
}

/// Bucket widths and lengths for the three rings.
pub const FINE_MINUTES: u32 = 15;
pub const FINE_LEN: usize = 96; // 24 h
pub const MEDIUM_MINUTES: u32 = 60;
pub const MEDIUM_LEN: usize = 168; // 7 days
pub const COARSE_MINUTES: u32 = 6 * 60;
pub const COARSE_LEN: usize = 120; // 30 days

/// All three, written together.
pub struct History {
    pub day: Ring<FINE_LEN>,
    pub week: Ring<MEDIUM_LEN>,
    pub month: Ring<COARSE_LEN>,
}

impl History {
    pub const fn new() -> Self {
        Self {
            day: Ring::new(FINE_MINUTES),
            week: Ring::new(MEDIUM_MINUTES),
            month: Ring::new(COARSE_MINUTES),
        }
    }

    pub fn record(&mut self, minute: u32, strike: &Strike) {
        self.day.record(minute, strike);
        self.week.record(minute, strike);
        self.month.record(minute, strike);
    }

    pub fn tick(&mut self, minute: u32) {
        self.day.tick(minute);
        self.week.tick(minute);
        self.month.tick(minute);
    }

    /// The last hour, from the fine ring: four fifteen-minute buckets.
    pub fn last_hour(&self) -> Bucket {
        self.day.recent(60 / FINE_MINUTES as usize)
    }
}

static mut PASS: u32 = 0;
static mut FAIL: u32 = 0;
fn check(name: &str, ok: bool) {
    unsafe {
        if ok { PASS += 1; println!("  ok   {name}"); }
        else { FAIL += 1; println!("  FAIL {name}"); }
    }
}

/// A strike whose intensity_milli is `intensity`, at `distance`.
fn strike(distance: Distance, intensity: u32) -> Strike {
    Strike { distance, energy_raw: intensity * 16777 / 1000 }
}

fn main() {
    println!("history:");

    // --- the score formula, against §4.3's observed figures ---------------
    // "distance ~6 km, intensity ~3-7, score ~0.5-1.2"
    let s = strike(Distance::Km(6), 3000);
    check("6 km at intensity 3 scores ~0.5", (490..=510).contains(&score_milli(&s).unwrap()));
    let s = strike(Distance::Km(6), 7000);
    check("6 km at intensity 7 scores ~1.16", (1140..=1180).contains(&score_milli(&s).unwrap()));

    check("overhead scores 10x a 1 km strike",
        score_milli(&strike(Distance::Overhead, 3000)).unwrap()
            == 10 * score_milli(&strike(Distance::Km(1), 3000)).unwrap());
    check("out of range has no score", score_milli(&strike(Distance::OutOfRange, 3000)).is_none());

    // --- bucketing ---------------------------------------------------------
    let mut ring: Ring<96> = Ring::new(15);
    ring.record(0, &strike(Distance::Km(6), 3000));
    ring.record(5, &strike(Distance::Km(6), 3000));
    ring.record(14, &strike(Distance::Km(4), 3000));
    check("three strikes inside one 15-min bucket land together", ring.recent(1).strikes == 3);
    ring.record(15, &strike(Distance::Km(6), 3000));
    check("minute 15 opens a new bucket", ring.recent(1).strikes == 1);
    check("...and the previous one is still there", ring.recent(2).strikes == 4);

    // --- the gap clear, which is the bug this file exists for ---------------
    let mut ring: Ring<96> = Ring::new(15);
    ring.record(0, &strike(Distance::Km(6), 3000));
    // One full lap of the ring (96 x 15 min = 24 h) plus a bit.
    ring.tick(15 * 96 + 30);
    check("a full lap clears every stale bucket", ring.recent(96).strikes == 0);

    let mut ring: Ring<96> = Ring::new(15);
    ring.record(0, &strike(Distance::Km(6), 3000));
    ring.tick(60);   // four buckets later, no strikes between
    check("a short gap clears only the skipped buckets", ring.recent(4).strikes == 0);
    check("...and leaves the older one intact", ring.recent(5).strikes == 1);

    // --- distance samples are not strike counts ----------------------------
    let mut ring: Ring<96> = Ring::new(15);
    ring.record(0, &strike(Distance::Km(10), 3000));
    ring.record(1, &strike(Distance::Overhead, 3000));
    ring.record(2, &strike(Distance::OutOfRange, 3000));
    let b = ring.recent(1);
    check("all three count as strikes", b.strikes == 3);
    check("only the one with a distance is averaged", b.distance_samples == 1);
    check("mean distance is that one, not a third of it", b.mean_distance_km() == Some(10));
    check("closest is tracked", b.distance_km_min == 10);

    // --- series ordering ---------------------------------------------------
    let mut ring: Ring<4> = Ring::new(15);
    ring.record(0, &strike(Distance::Km(6), 3000));
    ring.record(45, &strike(Distance::Km(6), 3000));
    ring.record(45, &strike(Distance::Km(6), 3000));
    let mut out = [Bucket::default(); 4];
    ring.series(&mut out);
    check("series is oldest-first, newest last", out[3].strikes == 2 && out[0].strikes == 1);

    // --- mean score --------------------------------------------------------
    let mut ring: Ring<96> = Ring::new(15);
    ring.record(0, &strike(Distance::Km(6), 3000));   // 500
    ring.record(1, &strike(Distance::Km(6), 7000));   // ~1160
    let mean = ring.recent(1).mean_score_milli().unwrap();
    check("mean score averages the two", (820..=840).contains(&mean));
    check("an empty bucket has no mean", Bucket::default().mean_score_milli().is_none());

    unsafe {
        println!("\n{PASS} passed, {FAIL} failed");
        std::process::exit(if FAIL == 0 { 0 } else { 1 });
    }
}
