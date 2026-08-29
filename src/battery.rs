//! MAX17048 LiPo fuel gauge (§2.1).
//!
//! On the shared I2C bus at **0x36**, powered from the cell in its own JST
//! rather than from VIN — so it keeps gauging whether or not the MCU is awake,
//! and its ModelGauge algorithm self-calibrates to that particular cell.
//!
//! ## Why a gauge and not a divider
//!
//! §2 has no ADC pin left: the display consumes GPIO2–5, which are the C3's
//! only ADC pads. So battery sensing had to go on a bus, and once it is on a
//! bus a fuel gauge costs the same two wires as a divider and answers a better
//! question. A divider reports volts, and volts-to-percent on a LiPo is a
//! curve that changes with age, temperature and load. This part reports
//! **percent** directly, and `CRATE` gives a real discharge rate rather than an
//! inferred one.
//!
//! ## Why not the `max170xx` crate
//!
//! Evaluated and rejected, recorded here so it is not re-litigated. It would
//! replace perhaps thirty lines of endianness-prone register plumbing — real,
//! if modest, value — but it returns `f32`, and on this soft-float target every
//! such call drags in a software floating-point routine to undo work the table
//! below does exactly in integers. The conversions, the signed `CRATE`
//! semantics and the learned range would all stay here regardless, so the crate
//! would sit between this module and the bus without removing anything this
//! module has to keep being right about.
//!
//! ## Everything here is integer arithmetic
//!
//! The C3 is `riscv32imc` — no FPU. Each scale factor below is written as an
//! exact integer ratio rather than a float, and each happens to be exact:
//!
//! | Register | Datasheet scale | As a ratio |
//! |---|---|---|
//! | `VCELL` | 78.125 µV/LSB | `raw × 5 / 64` mV |
//! | `SOC` | 1/256 %/LSB | `raw / 256` % |
//! | `CRATE` | 0.208 %/hr/LSB | `raw × 104 / 5` **centi**-%/hr |
//!
//! The first is worth noting: 78.125 µV is 0.078125 mV, which is exactly 5/64.
//! No rounding is lost.
//!
//! ## ⚠ The `CRATE` scale was wrong by 100x, and it hid in plain sight
//!
//! `crate_centi_per_hour` is *hundredths* of a percent per hour, so one LSB is
//! 0.208 %/hr = **20.8** of them = exactly `104/5`. The code used `26/125`,
//! which is also exactly 0.208 — the correct constant for the *wrong unit*. It
//! produced %/hr and stored it in a field whose name promises centi.
//!
//! Nothing looked wrong. `26/125` is defensibly derived, exact, and matches the
//! datasheet figure; the error was entirely in which unit it lands in. Two
//! visible symptoms, neither of which pointed here:
//!
//! * **Charging read as `idle`.** A real 6.24 %/hr taper displayed as
//!   `0.06 %/hr`, and a genuine 0.208 %/hr trickle truncated to `0.00`.
//! * **"Days left" almost never appeared.** [`Reading::hours_remaining`]
//!   discards rates below 5 centi-%/hr as too small to divide by. Under the old
//!   scale that threshold was really 5 *%/hr* — a rate this device never
//!   reaches — so the estimate was suppressed essentially always.
//!
//! Found by printing the raw register beside the decoded value and noticing
//! that `CRATE 0x0001` came out as `0.00 %/hr`. Reconciled against the bench:
//! raw 30 is 6.24 %/hr, which on a 2000 mAh cell is ~125 mA — consistent with
//! the USB meter, where `0.06 %/hr` (1.2 mA) plainly was not.

use esp_idf_hal::i2c::I2cDriver;
use esp_idf_hal::sys::EspError;

const ADDRESS: u8 = 0x36;

const REG_VCELL: u8 = 0x02;
const REG_SOC: u8 = 0x04;
const REG_VERSION: u8 = 0x08;
const REG_CRATE: u8 = 0x16;

const TIMEOUT_MS: u32 = 100;

/// What the gauge reports.
#[derive(Debug, Clone, Copy)]
pub struct Reading {
    pub millivolts: u16,
    /// State of charge, 0–100 %.
    pub percent: u8,
    /// Discharge rate in hundredths of a percent per hour. **Negative while
    /// discharging**, positive while charging — the sign is the datasheet's and
    /// is kept rather than normalised, because "is it charging" is a question
    /// the UI has to answer and the sign is the only thing that answers it.
    pub crate_centi_per_hour: i32,
}

impl Reading {
    /// Decode the three registers.
    ///
    /// Split out so the `battery` command can decode **the same read** it
    /// prints raw. Reading twice made the two lines disagree by a few
    /// hundredths, which defeats the purpose: raw values are printed so the
    /// arithmetic can be checked against them, and that only works if they
    /// describe the same instant.
    pub fn from_raw(vcell: u16, soc: u16, crate_raw: u16) -> Self {
        Reading {
            // 78.125 µV/LSB == 5/64 mV/LSB, exactly.
            millivolts: (vcell as u32 * 5 / 64) as u16,
            // The low byte is the fraction; the UI has no use for 1/256ths.
            percent: (soc >> 8).min(100) as u8,
            // Signed: negative while discharging. Reading this as unsigned turns
            // every discharge into an enormous positive rate.
            //
            // **This field is hundredths of a percent per hour**, so one LSB is
            // 0.208 %/hr = 20.8 of them = exactly 104/5. It was `26/125` for a
            // long time, which is 0.208 exactly — the right constant for the
            // wrong unit, giving %/hr where the field's name promises centi.
            // Everything downstream was therefore **100x low**; see the module
            // comment for what that cost.
            crate_centi_per_hour: crate_raw as i16 as i32 * 104 / 5,
        }
    }

    /// Hours until empty at the current rate, or `None` when that is not a
    /// meaningful question.
    ///
    /// `None` covers three genuinely different situations, and collapsing them
    /// into a number would be a lie in all three: the pack is **charging**
    /// (rate positive), the rate is **too small to divide by** (a device that
    /// has just woken has not discharged measurably yet), or the cell is
    /// already **empty**.
    pub fn hours_remaining(&self) -> Option<u32> {
        if self.crate_centi_per_hour >= 0 || self.percent == 0 {
            return None;
        }
        let rate = self.crate_centi_per_hour.unsigned_abs();
        // Below ~0.05 %/hr the quotient is dominated by quantisation: 100 % at
        // 0.04 %/hr computes as 250 days, which is not a battery estimate, it
        // is a division by nearly zero wearing a hat.
        if rate < 5 {
            return None;
        }
        Some(self.percent as u32 * 100 / rate)
    }

}

pub struct Max17048;

impl Max17048 {
    /// Confirm the gauge is there by reading its version register.
    ///
    /// A version read rather than a bare address probe: 0x36 is a common
    /// address and an ACK alone only proves *something* is listening.
    pub fn find(i2c: &mut I2cDriver<'_>) -> Option<(Self, u16)> {
        let version = read_u16(i2c, REG_VERSION).ok()?;
        // A MAX17048 reports 0x001x. All-zeroes or all-ones is a bus that is
        // answering with its own idle state rather than a chip that is talking.
        if version == 0x0000 || version == 0xFFFF {
            return None;
        }
        Some((Max17048, version))
    }

    /// The three registers unscaled, for the `battery` command.
    ///
    /// A diagnostic in the same spirit as `as3935::dump_registers`: when the
    /// derived numbers look wrong, the question is whether the chip said
    /// something different or whether we did the arithmetic wrong, and only the
    /// raw values separate those.
    pub fn read_raw(&self, i2c: &mut I2cDriver<'_>) -> Result<(u16, u16, u16), EspError> {
        Ok((
            read_u16(i2c, REG_VCELL)?,
            read_u16(i2c, REG_SOC)?,
            read_u16(i2c, REG_CRATE)?,
        ))
    }

    pub fn read(&self, i2c: &mut I2cDriver<'_>) -> Result<Reading, EspError> {
        let (vcell, soc, crate_raw) = self.read_raw(i2c)?;
        Ok(Reading::from_raw(vcell, soc, crate_raw))
    }
}

/// Read a big-endian 16-bit register.
///
/// The MAX17048 is big-endian, unlike most of what shares a bus with it — a
/// byte-swap here reads as a plausible voltage rather than as an error, so it
/// is the sort of mistake that ships.
fn read_u16(i2c: &mut I2cDriver<'_>, register: u8) -> Result<u16, EspError> {
    let mut bytes = [0u8; 2];
    i2c.write_read(ADDRESS, &[register], &mut bytes, TIMEOUT_MS)?;
    Ok(u16::from_be_bytes(bytes))
}

// === The learned range =====================================================
//
// The gauge reports a percentage from its own ModelGauge curve, which is better
// than anything a linear voltage map could do — a LiPo's discharge curve is
// famously flat in the middle and a cliff at both ends. So the learned range
// below **does not replace** that percentage. It answers two different
// questions the gauge cannot:
//
// * **How long will it run?** Runtime needs the span this particular cell
//   actually works over, on this particular load. The datasheet's 3.0–4.2 V is
//   a chemistry figure; what matters is where *this* device browns out and
//   where *this* charger stops.
// * **Is the cell ageing?** A pack whose observed maximum has fallen from 4.20
//   to 4.05 V has lost capacity, and nothing else on the device would say so.

/// Where the range starts before anything has been observed.
///
/// Deliberately narrow rather than the chemistry's full 3.0–4.2 V: a range that
/// starts wide can only ever be confirmed, never learned, and would report a
/// confident span this unit has never actually reached.
pub const SEED_RANGE: (u16, u16) = (3500, 4100);

// === How long is left ========================================================
//
// ## Why `CRATE` cannot answer this, however the display is written
//
// The MAX17048 reports its charge rate in units of **0.208 %/hr** — one LSB, and
// [`Reading::from_raw`] converts by exactly that. Put §7's measured draw beside
// it:
//
// | policy | measured | implied rate | in LSBs |
// |---|---|---|---|
// | Awake, 160 MHz | 33.5 mA, ~2.5 d | 1.68 %/hr | 8 |
// | Frugal, 80/10 MHz + light sleep | 2.5–3.0 mA, ~28–33 d | **0.13–0.15 %/hr** | **under 1** |
//
// So on the policy this device spends its life in, the true discharge is finer
// than the gauge can count. `CRATE` reads exactly zero — not briefly, but for
// the whole run — and every estimate derived from it is `None`. The 13× power
// win is precisely what put the discharge under the gauge's resolution.
//
// The answer therefore has to come from voltage over a long baseline. [`Trend`]
// cannot supply it either: five minutes and an 8 mV threshold, built to tell
// direction, not to be divided by.
//
// ## Two tiers, and why both exist
//
// * **Position** ([`hours_from_position`]) — where `cur` sits between the
//   learned borders, times how long a full span is known to take. Coarse,
//   because it assumes this cell drains like the measured one, but available
//   *immediately* — including in the window after a charge, when the
//   accumulator has just been reset and has nothing to say.
// * **Measured** ([`hours_from_drain`]) — millivolts actually shed per hour by
//   *this* cell under *this* load. Strictly better once it has data, and the
//   reason the accumulator exists at all.
//
// The measured one wins where it exists, which is the same rule the learned
// range already follows over the gauge's own percentage.

/// How long a full span takes at each of §7's two policies, in hours.
///
/// Measured at the USB port, not modelled: 2.5–3.0 mA frugal and 33.5 mA awake,
/// against the 2000 mAh cell §7 specifies. These are only ever used by the
/// coarse tier — the moment the accumulator has a real rate, this device's own
/// number replaces them.
///
/// They differ by 13×, which is why the policy has to be passed in rather than
/// averaged: a single constant would be wrong by an order of magnitude in
/// whichever state the device was not in.
pub const NOMINAL_HOURS_FRUGAL: u32 = 30 * 24;
pub const NOMINAL_HOURS_AWAKE: u32 = 60;

/// Below this, an accumulated fall is indistinguishable from noise.
///
/// A resting cell drifts a millivolt or two over five minutes and a panel
/// refresh dips the rail briefly — see [`TREND_THRESHOLD_MV`], which sets 8 mV
/// over five minutes for the same reason. At the frugal policy's ~20 mV/day this
/// is reached in half a day.
pub const MIN_DRAIN_MV: u32 = 10;

/// And below this much elapsed time, a real fall is still too short a lever.
///
/// Both gates must pass. Voltage alone is not enough: a sag under load can shed
/// 10 mV in seconds, and dividing by that span would claim the cell has hours
/// left when it has weeks.
pub const MIN_DRAIN_S: u32 = 6 * 3600;

/// How often the accumulator is written back to NVS.
///
/// Fifteen minutes, matching [`crate::clock::SAVE_INTERVAL_S`] and for the same
/// reason: the gauge is polled every ten seconds, and writing flash at that
/// cadence to protect a number that averages over *days* would spend endurance
/// to buy nothing. A power cut costs the last interval.
pub const DRAIN_SAVE_S: u32 = 15 * 60;

/// A rise this large means the cell is being charged, not resting.
///
/// Comfortably above the few millivolts a cell rebounds when a load drops, and
/// far below what a charger does. Crossing it throws the accumulation away and
/// starts a new one, because a rate averaged across a charge is not a rate.
pub const DRAIN_RESET_MV: u16 = 20;

/// Millivolts shed, and over how many seconds — the accumulator behind the
/// accurate tier.
///
/// **Sum and count rather than a start-anchor.** An anchor would need only two
/// numbers too, but it measures net change, so a cell that sags and recovers
/// reports having shed nothing. Summing the falls charges every real decline to
/// the total and lets `seconds` carry the whole elapsed span, including the flat
/// parts — which is what makes the quotient an average rate rather than a
/// best-case one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Drain {
    /// Total millivolts fallen since the last reset.
    pub sum_mv: u32,
    /// Total seconds observed since the last reset, falling or not.
    pub seconds: u32,
}

/// Fold one reading into the accumulator.
///
/// `previous` is the last voltage seen, which lives in RAM rather than NVS: a
/// reboot costs one sample of resolution, where persisting it would cost a flash
/// write per poll to protect a number that is re-established on the next one.
///
/// Returns the new accumulator, and whether the caller should treat this as a
/// reset — the one event worth reporting, since it discards history.
pub fn drained(drain: Drain, previous: u16, now_mv: u16, elapsed_s: u32) -> (Drain, bool) {
    // Going back up. Charging, or a rebound big enough to be indistinguishable
    // from one -- either way the run being averaged has ended.
    if now_mv > previous.saturating_add(DRAIN_RESET_MV) {
        return (Drain::default(), true);
    }

    // Time always accrues; millivolts only when they actually fall. A flat hour
    // is evidence about the rate and belongs in the denominator.
    let fall = previous.saturating_sub(now_mv) as u32;
    (
        Drain {
            sum_mv: drain.sum_mv.saturating_add(fall),
            seconds: drain.seconds.saturating_add(elapsed_s),
        },
        false,
    )
}

/// Hours left at the rate this cell has actually been shedding.
///
/// `None` until both gates in [`MIN_DRAIN_MV`] and [`MIN_DRAIN_S`] pass, or once
/// the reading is at or below the learned floor — at which point the honest
/// answer is that it is over, not a number.
pub fn hours_from_drain(drain: Drain, now_mv: u16, low: u16) -> Option<u32> {
    if drain.sum_mv < MIN_DRAIN_MV || drain.seconds < MIN_DRAIN_S {
        return None;
    }
    let above_floor = now_mv.checked_sub(low)?;
    if above_floor == 0 {
        return None;
    }
    // Seconds first, hours after: dividing to hours up front would round a
    // multi-day span through a one-hour quantum before it is multiplied.
    let seconds_left = above_floor as u64 * drain.seconds as u64 / drain.sum_mv as u64;
    Some((seconds_left / 3600) as u32)
}

/// Hours left from position in the learned range alone.
///
/// The coarse tier: what fraction of the span is left, times how long a whole
/// span is known to take. It assumes this cell behaves like the one §7 metered,
/// which is exactly the assumption [`hours_from_drain`] removes once it can.
pub fn hours_from_position(range: (u16, u16), now_mv: u16, light_sleep: bool) -> Option<u32> {
    let (low, high) = range;
    let span = high.checked_sub(low)?;
    if span == 0 {
        return None;
    }
    let above_floor = now_mv.min(high).checked_sub(low)?;
    if above_floor == 0 {
        return None;
    }
    let full = if light_sleep {
        NOMINAL_HOURS_FRUGAL
    } else {
        NOMINAL_HOURS_AWAKE
    };
    Some(above_floor as u32 * full / span as u32)
}

/// Readings outside this are not the cell, whatever they say.
///
/// A LiPo below 2.5 V is damaged or absent, and above 4.5 V is a measurement
/// fault. Widening to either would poison the range permanently, and the range
/// is the one thing here that survives a reboot.
const SANE_MV: (u16, u16) = (2500, 4500);

/// Move a range to admit `mv`, halfway.
///
/// **The same midpoint rule the moisture project settled on**, and for the same
/// reason: moving an endpoint straight to a new extreme makes the range a
/// one-way ratchet driven by the single worst reading the device ever takes —
/// and here that reading is a sag under a panel refresh, or the moment a
/// charger is unplugged. Halving turns it into a low-pass filter: a one-off
/// event moves the endpoint half way and stops, while a genuine change keeps
/// producing out-of-range readings and converges in about five.
///
/// Returns `None` when nothing moves, which is the normal answer and the reason
/// this writes flash so rarely.
pub fn widened(range: (u16, u16), mv: u16) -> Option<(u16, u16)> {
    if mv < SANE_MV.0 || mv > SANE_MV.1 {
        return None;
    }

    let (mut low, mut high) = range;
    if mv < low {
        low = midpoint(low, mv);
    } else if mv > high {
        high = midpoint(high, mv);
    }

    // Integer division returns the same endpoint for a 1 mV excess, which is
    // also what stops ADC noise generating a flash write on every reading.
    ((low, high) != range).then_some((low, high))
}

/// Halfway between two values, without overflowing on the way.
fn midpoint(from: u16, to: u16) -> u16 {
    if to > from {
        from + (to - from) / 2
    } else {
        from - (from - to) / 2
    }
}

/// Hours of runtime left, predicted from the learned range and the measured
/// discharge rate.
///
/// This is the **second** of the two predictions, and it differs from
/// [`Reading::hours_remaining`] in what it trusts. That one divides the gauge's
/// own percentage by the gauge's own rate — self-consistent, and wrong in the
/// same direction as the gauge whenever the gauge is wrong about this cell.
/// This one asks how much of the *observed* voltage span is left and how fast
/// the cell is crossing it.
///
/// They disagree most where it matters: near empty, where a LiPo's curve falls
/// off a cliff and a percentage flatters the remaining time.
///
/// `None` when the range has not been learned widely enough to divide by, when
/// charging, or when the reading is already at or below the learned floor.
pub fn hours_from_range(range: (u16, u16), reading: &Reading) -> Option<u32> {
    let (low, high) = range;
    let span = high.checked_sub(low)?;
    // Under 200 mV of observed span, this has seen a fraction of one discharge
    // and any figure derived from it is invented.
    if span < 200 || reading.crate_centi_per_hour >= 0 {
        return None;
    }

    let above_floor = reading.millivolts.saturating_sub(low);
    if above_floor == 0 {
        return None;
    }

    // Fraction of the span remaining, as a percentage, then the same division
    // the gauge does -- but over a span this device has actually measured.
    let percent_of_span = above_floor as u32 * 100 / span as u32;
    let rate = reading.crate_centi_per_hour.unsigned_abs();
    if rate < 5 {
        return None;
    }
    Some(percent_of_span * 100 / rate)
}


// === Which way the charge is going ==========================================

/// What the cell is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flow {
    Charging,
    Discharging,
    /// Neither, as far as we can tell — including "charging so gently that
    /// nothing we can measure moves".
    Idle,
    /// Not enough history yet. Distinct from `Idle`, which is a conclusion.
    Unknown,
}

impl Flow {
    pub fn label(self) -> &'static str {
        match self {
            Flow::Charging => "charging",
            Flow::Discharging => "discharging",
            Flow::Idle => "idle",
            Flow::Unknown => "unknown",
        }
    }
}

/// Cell voltage over time, so charging can be detected when `CRATE` cannot.
///
/// ## Why this exists
///
/// `CRATE` is the right answer eventually and the wrong one immediately. The
/// MAX17048 has no current sense — it infers a rate from how fast its *SOC
/// estimate* moves, and that estimate is heavily filtered, so `CRATE` takes
/// minutes to respond to a supply change.
///
/// Measured on this board: the USB meter read **1.16 W** — about 232 mA at 5 V,
/// against roughly 33 mA for the board itself, so ~200 mA was going into the
/// cell — while `CRATE` still read `0.00 %/hr` and the screen said `idle`.
/// Several minutes later `CRATE` came good and the screen said `charging` on
/// its own. Nothing was broken; the reading was simply late, and "late" on a
/// status line is indistinguishable from "wrong".
///
/// Cell voltage moves immediately. A charger holds the cell up; a load pulls it
/// down. So this keeps an anchor sample and compares against it, which fills
/// exactly the window in which `CRATE` has nothing to say.
///
/// It also matters for the **MAX17043**, the older part in the same family:
/// that one has no `CRATE` register at all, only VCELL and SOC. There, this is
/// not a fallback — it is the only way to answer the question.
///
/// ## Why an anchor rather than consecutive samples
///
/// Between two samples a minute apart the change is smaller than the noise a
/// panel refresh puts on the rail. Holding one anchor for several minutes gives
/// the signal time to exceed that, at the cost of taking that long to notice a
/// change — acceptable for something displayed beside a battery percentage.
pub struct Trend {
    anchor_mv: u16,
    anchor_s: u32,
    latest_mv: u16,
    latest_s: u32,
}

/// How long an anchor is held before it is replaced.
pub const TREND_WINDOW_S: u32 = 5 * 60;

/// How much the cell must move across the window to count as going somewhere.
///
/// Under this device's ~33 mA load a resting cell drifts by a millivolt or two
/// over five minutes, and a panel refresh can dip the rail briefly. 8 mV is
/// comfortably above both and well below what 200 mA of charge produces.
pub const TREND_THRESHOLD_MV: i32 = 8;

impl Trend {
    pub fn new(millivolts: u16, now_s: u32) -> Self {
        Trend {
            anchor_mv: millivolts,
            anchor_s: now_s,
            latest_mv: millivolts,
            latest_s: now_s,
        }
    }

    pub fn observe(&mut self, millivolts: u16, now_s: u32) {
        self.latest_mv = millivolts;
        self.latest_s = now_s;
        if crate::uptime::due(now_s, self.anchor_s, TREND_WINDOW_S) {
            self.anchor_mv = millivolts;
            self.anchor_s = now_s;
        }
    }

    /// Millivolts moved since the anchor. Positive is rising.
    pub fn delta_mv(&self) -> i32 {
        self.latest_mv as i32 - self.anchor_mv as i32
    }

    /// How long the trend window has been accumulating.
    ///
    /// **`uptime::since`, not `saturating_sub`.** Both fields come from the
    /// uptime counter, which wraps every 49.7 days, and across the wrap
    /// `latest_s` is small while `anchor_s` is near the top — so a saturating
    /// subtraction reports a zero-length span. `verdict` refuses to judge a
    /// span shorter than the window, so the battery trend would read `Unknown`
    /// until the anchor next rolled. Milder than the same mistake in `policy`,
    /// because it heals itself, but the same mistake: `roll` on the line above
    /// already uses wrapping arithmetic and this did not.
    pub fn span_s(&self) -> u32 {
        crate::uptime::since(self.latest_s, self.anchor_s)
    }

    /// What the voltage alone says.
    ///
    /// `Unknown` until the window has had time to mean something — reporting
    /// `Idle` from a two-second span would be a guess wearing a conclusion's
    /// clothes.
    pub fn verdict(&self) -> Flow {
        if self.span_s() < 60 {
            return Flow::Unknown;
        }
        let delta = self.delta_mv();
        if delta >= TREND_THRESHOLD_MV {
            return Flow::Charging;
        }
        if delta <= -TREND_THRESHOLD_MV {
            return Flow::Discharging;
        }
        Flow::Idle
    }
}

/// Combine what the gauge says with what the voltage does.
///
/// `CRATE` is trusted whenever it is saying anything at all — it is a proper
/// filtered estimate over a long window and beats a two-point voltage
/// difference. The trend only gets a say while `CRATE` reads exactly zero,
/// which in practice means the first few minutes after a supply change.
pub fn flow(reading: &Reading, trend: Option<&Trend>) -> Flow {
    if reading.crate_centi_per_hour > 0 {
        return Flow::Charging;
    }
    if reading.crate_centi_per_hour < 0 {
        return Flow::Discharging;
    }
    match trend {
        Some(trend) => trend.verdict(),
        None => Flow::Unknown,
    }
}


/// How often the fuel gauge is read.
///
/// An I2C transaction for values that move over hours, so ten seconds is
/// already generous — it exists to keep the screen's figure fresh rather than
/// because the cell changes that fast.
pub const GAUGE_POLL_S: u32 = 10;

/// Everything the fuel gauge remembers between polls.
///
/// **Eight loop locals that were one subject.** `reading`, `trend`, `range`,
/// `drain`, `previous_mv` and three timestamps all describe the cell, and every
/// one of them was threaded separately through `listen`, the console handler and
/// the redraw path — which is how the learned range came to be widened from the
/// *drawing* code, three hundred lines from everything else that touches it.
///
/// The gauge itself is passed in, not held: it is hardware, and this is state.
pub struct Fuel {
    /// The most recent reading, or `None` until the first poll or without a
    /// gauge. Cached because the screen wants it far less often than the loop
    /// runs, and it is an I2C transaction.
    pub reading: Option<Reading>,
    /// Voltage over time, because `CRATE` cannot see a charger in taper — see
    /// [`Trend`].
    pub trend: Option<Trend>,
    /// §2.1's learned range. Seeded rather than empty, so the first reading has
    /// something to widen from — and `None` from NVS is a virgin device, not an
    /// error.
    pub range: (u16, u16),
    /// The discharge accumulator (§7). Restored rather than started fresh: it
    /// averages over days, so a device that reset an hour ago must not go back
    /// to "no estimate" — that is exactly the window in which somebody is
    /// watching it and wants one.
    pub drain: Drain,
    /// Deliberately RAM-only. Persisting it would cost a flash write every ten
    /// seconds to protect a value the very next poll re-establishes.
    previous_mv: Option<u16>,
    last_drain_s: u32,
    last_drain_save_s: u32,
    last_poll_ms: u32,
}

impl Fuel {
    /// Read once up front rather than waiting out the first poll interval, so
    /// the very first screen carries a real battery figure instead of "no gauge".
    pub fn new(
        gauge: Option<&Max17048>,
        i2c: &mut esp_idf_hal::i2c::I2cDriver<'_>,
        now_ms: u32,
    ) -> Fuel {
        let reading = gauge.and_then(|g| g.read(i2c).ok());
        let now_s = now_ms / 1000;
        Fuel {
            reading,
            trend: reading.map(|r| Trend::new(r.millivolts, now_s)),
            range: crate::settings::battery_range().unwrap_or(SEED_RANGE),
            drain: crate::settings::battery_drain().unwrap_or_default(),
            // Seeded from the reading taken above, so the first interval
            // measured is a real one rather than the gap between boot and the
            // first poll.
            previous_mv: reading.map(|r| r.millivolts),
            last_drain_s: now_s,
            last_drain_save_s: now_s,
            last_poll_ms: now_ms,
        }
    }

    pub fn due(&self, now_ms: u32) -> bool {
        crate::uptime::due(now_ms, self.last_poll_ms, GAUGE_POLL_S * 1000)
    }

    /// Take a reading and fold it into the trend and the discharge baseline.
    ///
    /// The accumulator lives here rather than in the redraw path because it must
    /// see *every* sample: a rate assembled from the handful of polls that
    /// happened to coincide with a repaint would be an average of nothing in
    /// particular. `CRATE` cannot supply one — on the frugal policy this cell
    /// drains at ~0.14 %/hr against a gauge whose LSB is 0.208, so the register
    /// reads a hard zero for the whole run, and millivolts over hours is the
    /// only measurement left.
    pub fn poll(
        &mut self,
        gauge: Option<&Max17048>,
        i2c: &mut esp_idf_hal::i2c::I2cDriver<'_>,
        now_ms: u32,
    ) {
        self.last_poll_ms = now_ms;
        self.reading = gauge.and_then(|g| g.read(i2c).ok());
        let reading = match self.reading {
            Some(reading) => reading,
            None => return,
        };
        let now_s = now_ms / 1000;

        match self.trend.as_mut() {
            Some(trend) => trend.observe(reading.millivolts, now_s),
            None => self.trend = Some(Trend::new(reading.millivolts, now_s)),
        }

        match self.previous_mv {
            None => self.previous_mv = Some(reading.millivolts),
            Some(previous) => {
                let elapsed = crate::uptime::since(now_s, self.last_drain_s);
                let (next, reset) = drained(self.drain, previous, reading.millivolts, elapsed);
                self.previous_mv = Some(reading.millivolts);
                self.last_drain_s = now_s;
                self.drain = next;

                // A reset is the one event worth both saying and saving
                // immediately: it throws away the baseline, and a power cut that
                // restored the discarded one would put a stale rate back into
                // service.
                if reset {
                    println!("bat:  charging or rebounding -- discharge baseline reset");
                    if let Err(e) = crate::settings::store_battery_drain(self.drain) {
                        println!("bat:  baseline reset but NOT saved -- {e}");
                    }
                    self.last_drain_save_s = now_s;
                } else if crate::uptime::due(now_s, self.last_drain_save_s, DRAIN_SAVE_S) {
                    self.last_drain_save_s = now_s;
                    if let Err(e) = crate::settings::store_battery_drain(self.drain) {
                        println!("bat:  baseline NOT saved -- {e}");
                    }
                }
            }
        }

        println!(
            "bat:  {} mV, {}%, rate {}.{:02} %/hr",
            reading.millivolts,
            reading.percent,
            reading.crate_centi_per_hour / 100,
            (reading.crate_centi_per_hour % 100).abs()
        );
    }

    /// Widen the learned range if this reading is a new extreme.
    ///
    /// Written to NVS only when an endpoint actually moves, which the midpoint
    /// rule makes rare: it takes a NEW extreme, and new extrema in a noisy
    /// series get rarer the longer it runs. Called from the redraw path, so the
    /// write cadence is bounded by the panel's rather than by the gauge's.
    pub fn widen(&mut self) {
        let reading = match self.reading {
            Some(reading) => reading,
            None => return,
        };
        let moved = match widened(self.range, reading.millivolts) {
            Some(moved) => moved,
            None => return,
        };
        match crate::settings::store_battery_range(moved.0, moved.1) {
            Ok(()) => {
                println!(
                    "bat:  range {}-{} -> {}-{} mV",
                    self.range.0, self.range.1, moved.0, moved.1
                );
                self.range = moved;
            }
            Err(e) => println!("bat:  range moved but NOT saved -- {e}"),
        }
    }
}
