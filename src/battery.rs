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
//! | `CRATE` | 0.208 %/hr/LSB | `raw × 26 / 125` %/hr |
//!
//! The first is worth noting: 78.125 µV is 0.078125 mV, which is exactly 5/64.
//! No rounding is lost.

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

    pub fn is_charging(&self) -> bool {
        self.crate_centi_per_hour > 0
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

    pub fn read(&self, i2c: &mut I2cDriver<'_>) -> Result<Reading, EspError> {
        let vcell = read_u16(i2c, REG_VCELL)?;
        let soc = read_u16(i2c, REG_SOC)?;
        let crate_raw = read_u16(i2c, REG_CRATE)? as i16;

        Ok(Reading {
            // 78.125 µV/LSB == 5/64 mV/LSB, exactly.
            millivolts: (vcell as u32 * 5 / 64) as u16,
            // The low byte is the fraction; the UI has no use for 1/256ths.
            percent: (soc >> 8).min(100) as u8,
            // 0.208 %/hr per LSB == 26/125, exactly, in hundredths of a percent.
            crate_centi_per_hour: crate_raw as i32 * 26 / 125,
        })
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
pub const SEED_RANGE: (u16, u16) = (3600, 4100);

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
