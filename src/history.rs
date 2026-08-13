//! Strike history, bucketed by time (§4.3, §6).
//!
//! Feeds two things: the "last hour" statistics on screen, and the day / week /
//! month charts under them.
//!
//! ## Three resolutions, not one
//!
//! A single ring fine enough for a storm and long enough for a month would be
//! 2880 fifteen-minute buckets — 20 KB to answer questions that mostly want a
//! summary. Three rings cost a tenth of that and each is the right shape for
//! its chart:
//!
//! | Ring | Bucket | Span | Answers |
//! |---|---|---|---|
//! | Fine | 15 min | 24 h | what is happening now, and the last hour |
//! | Medium | 1 h | 7 days | this week's storms |
//! | Coarse | 6 h | 30 days | the season |
//!
//! Every strike is written to all three. That is deliberate duplication: the
//! alternative is deriving the coarse rings from the fine one, which fails the
//! moment the fine ring wraps — and it wraps every day.
//!
//! ## What a bucket keeps, and why not just a count
//!
//! §4.3 wants distance, intensity and score. A count alone cannot tell a
//! violent storm from a busy one, and an average alone cannot tell one strike
//! from a hundred — so a bucket keeps enough to produce both, and the charts
//! show them side by side rather than choosing.
//!
//! Sums rather than averages, because a running average cannot absorb a new
//! sample without also knowing the count, and keeping the count is what makes
//! every other statistic derivable.
//!
//! ## Absolute minutes, not minutes since boot
//!
//! Buckets are indexed from the **Unix epoch**, not from power-on. That is what
//! lets the rings be rebuilt from the CSV at startup: a record written last
//! Tuesday has to land in last Tuesday's bucket, and a clock that restarts at
//! zero on every boot cannot say where that is.
//!
//! A device whose clock has never been set falls back to uptime, which is
//! self-correcting rather than merely tolerated: the indices jump by decades
//! the moment the clock is set, and a jump larger than the ring clears it —
//! which is exactly right, because everything recorded before the device knew
//! the time is unplaceable anyway.

use crate::strike::{Distance, Strike};

/// One time bucket.
///
/// `Copy` and eight bytes, so a ring of them is a plain array with no
/// allocation and no per-bucket bookkeeping.
#[derive(Debug, Clone, Copy)]
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
    /// Whether any strike in this window was reported **overhead**.
    ///
    /// **A flag rather than a zero in `distance_km_min`.** Overhead is the
    /// closest a strike can be, but it is not "0 km" — folding it in that way is
    /// what once made a storm read as permanently overhead, and the correction
    /// then dropped overhead from the distance statistics altogether. So the
    /// screen showed `closest -` for the one classification that means *directly
    /// above*, which is the reading this device exists to give.
    ///
    /// Kept separate so `distance_km_min` stays a true minimum over measured
    /// kilometres, and the display can say "overhead" instead of a number.
    pub overhead: bool,
}

/// **Hand-written, because `derive(Default)` is wrong for this type.**
///
/// `distance_km_min` uses `u8::MAX` as "nothing recorded", and a derived
/// default would give it `0` — which then wins every `min()` it takes part in.
/// `Ring::new` set the sentinel correctly, but every bucket cleared by
/// `advance_to` used the derived default, so an empty or partly-empty window
/// reported the **closest strike as 0 km**: a storm directly overhead, forever,
/// on a device that had seen nothing.
///
/// Caught on hardware after a refactor: three replayed strikes at 20, 9 and
/// 3 km reported `closest 0 km`.
impl Default for Bucket {
    fn default() -> Self {
        Self {
            strikes: 0,
            score_milli_sum: 0,
            distance_km_sum: 0,
            distance_samples: 0,
            distance_km_min: u8::MAX,
            overhead: false,
        }
    }
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

    #[allow(dead_code)]
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
    /// Bucket index the ring first saw, so a partly-filled ring knows how much
    /// of itself is real.
    first_index: u32,
    minutes_per_bucket: u32,
    started: bool,
}

impl<const N: usize> Ring<N> {
    pub const fn new(minutes_per_bucket: u32) -> Self {
        Self {
            // `Bucket::default()` is not `const`, so the sentinel is spelled
            // out once here. It must match the `Default` impl above.
            buckets: [Bucket {
                strikes: 0,
                score_milli_sum: 0,
                distance_km_sum: 0,
                distance_samples: 0,
                distance_km_min: u8::MAX,
                overhead: false,
            }; N],
            newest_index: 0,
            first_index: 0,
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
            self.first_index = index;
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
        if let Distance::Overhead = strike.distance {
            bucket.overhead = true;
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
    ///
    /// Not on the draw path yet — the day/week/month charts are the next piece
    /// of §6. Kept because it is the ring's natural read API and is covered by
    /// the host checks, not as a stub for something unwritten.
    #[allow(dead_code)]
    pub fn series(&self, into: &mut [Bucket; N]) {
        for offset in 0..N {
            // `newest_index + 1` is the oldest slot: one past the newest, which
            // is where the ring wraps to.
            let index = self.newest_index as usize + 1 + offset;
            into[offset] = self.buckets[index % N];
        }
    }

    /// How many buckets hold real data.
    ///
    /// **Not always `N`.** A ring that has been running for two hours has eight
    /// fifteen-minute buckets, not ninety-six — and drawing the other
    /// eighty-eight as empty columns to the *left* of them reads as "a day of
    /// silence, then this", which is a different and wrong story. Knowing the
    /// live length lets a chart fill from the left and only scroll once it is
    /// genuinely full.
    pub fn live_len(&self) -> usize {
        if !self.started {
            return 0;
        }
        ((self.newest_index - self.first_index + 1) as usize).min(N)
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
            total.overhead |= bucket.overhead;
        }
        total
    }
}

/// Bucket widths and lengths for the three rings.
pub const FINE_MINUTES: u32 = 5;
pub const FINE_LEN: usize = 288; // 24 h
pub const MEDIUM_MINUTES: u32 = 60;
pub const MEDIUM_LEN: usize = 168; // 7 days
pub const COARSE_MINUTES: u32 = 6 * 60;
pub const COARSE_LEN: usize = 120; // 30 days

/// All three, written together.
pub struct History {
    pub day: Ring<FINE_LEN>,
    pub week: Ring<MEDIUM_LEN>,
    pub month: Ring<COARSE_LEN>,
    /// The last few strikes, whole rather than bucketed — what the screen's
    /// table draws.
    pub recent: RecentLog,
}

impl History {
    pub const fn new() -> Self {
        Self {
            day: Ring::new(FINE_MINUTES),
            week: Ring::new(MEDIUM_MINUTES),
            month: Ring::new(COARSE_MINUTES),
            recent: RecentLog::new(),
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


/// Flatten a ring into the two series the charts draw.
///
/// Two separate arrays rather than one of structs, because the renderer wants
/// each as a contiguous run of numbers to scale and walk — and because the two
/// scale independently: counts and mean scores share a time axis and nothing
/// else.
/// Returns how many buckets are real, which is what the chart should draw.
///
/// The output is the **live tail**, oldest first: a partly-filled ring yields
/// just its own buckets so a chart can fill from the left, and a full one
/// yields all `N` so it scrolls.
pub fn series_of<const N: usize>(
    ring: &Ring<N>,
    counts: &mut [u16; N],
    scores: &mut [u32; N],
) -> usize {
    let mut buckets = [Bucket::default(); N];
    ring.series(&mut buckets);

    // `series` is oldest-first over the whole ring, so the live buckets are its
    // last `live` entries -- everything before them is a lap that never
    // happened.
    let live = ring.live_len();
    let start = N - live;
    for offset in 0..live {
        let bucket = buckets[start + offset];
        counts[offset] = bucket.strikes;
        // **The sum, not the mean.** A mean flattens exactly the thing the
        // chart is for: ten violent strikes in a bucket and one give the same
        // mean and wildly different weather. The sum is `energy/distance`
        // accumulated over the bucket, so a bar's height is how much the sky
        // threw during it. Zero stays zero -- an empty bucket has no bar, which
        // is the difference between calm and no data.
        scores[offset] = bucket.score_milli_sum;
    }
    live
}

/// How many recent strikes the screen's log keeps.
///
/// Seventeen because that is what fits: the table runs from y=176 to the footer
/// rule at 452 at a 16 px pitch, and the last row ends at 447.
pub const RECENT_LEN: usize = 17;

/// One line of the screen's strike log.
///
/// **Separate from [`Bucket`], which cannot answer this.** The rings aggregate
/// into 5-minute, 1-hour and 6-hour buckets, so an individual strike stops
/// existing the moment it is recorded. This keeps the last few whole, which is
/// what a person watching a storm actually reads.
#[derive(Clone, Copy)]
pub struct Recent {
    pub epoch: Option<u64>,
    pub distance: Distance,
    pub energy_raw: u32,
    pub score_milli: u32,
    /// Strokes folded into this record by §4.3's merge window.
    pub strokes: u32,
}

/// The last [`RECENT_LEN`] strikes, newest first.
///
/// **Not rebuilt from the CSV at boot**, deliberately: reboots are rare, the
/// charts already carry the history, and teaching the log reader to return the
/// stroke column would be work in service of the first thirty seconds after a
/// restart.
pub struct RecentLog {
    entries: [Option<Recent>; RECENT_LEN],
}

impl RecentLog {
    pub const fn new() -> Self {
        Self {
            entries: [None; RECENT_LEN],
        }
    }
}

impl Default for RecentLog {
    fn default() -> Self {
        Self::new()
    }
}

impl RecentLog {
    /// Newest to the front, oldest off the end.
    pub fn push(&mut self, entry: Recent) {
        self.entries.rotate_right(1);
        self.entries[0] = Some(entry);
    }

    /// Newest first, skipping the slots never filled.
    pub fn iter(&self) -> impl Iterator<Item = &Recent> {
        self.entries.iter().flatten()
    }
}
