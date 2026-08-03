//! What a strike is, with no hardware attached.
//!
//! ## Why these two types live apart from the driver that produces them
//!
//! `history` and the score formula operate on strikes and nothing else — no
//! I2C, no registers, no ESP-IDF. Keeping them in `as3935` meant that anything
//! importing a `Distance` also imported the whole HAL, which is what forced
//! `tests/host` to hand-copy the logic it wanted to test rather than compiling
//! the real thing.
//!
//! Splitting them is what lets the host harnesses `#[path]`-include the actual
//! source files, so a divergence between the tested code and the shipped code
//! becomes a compile error instead of a note in a README.
//!
//! `as3935` re-exports both, so callers still write `as3935::Distance`.

/// How far away the storm is.
///
/// The reference driver returns the raw 6-bit field and lets the caller treat
/// it as kilometres. **Two of its 64 values are not distances**, and conflating
/// them with real ones is how a storm directly overhead gets charted at 1 km
/// alongside a genuine 1 km reading, or an out-of-range flash gets charted at
/// 63 km and drags every average with it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Distance {
    /// Estimated distance to the head of the storm, in kilometres.
    Km(u8),
    /// `0x01` — the storm is overhead. Not "1 km".
    Overhead,
    /// `0x3F` — detected, but out of the estimation range.
    OutOfRange,
}

/// One detected strike.
#[derive(Debug, Clone, Copy)]
pub struct Strike {
    pub distance: Distance,
    /// The raw 20-bit energy field.
    ///
    /// **Kept raw, and deliberately not divided.** The reference returns
    /// `raw / 16777` as a float, and the C3 is `riscv32imc` — no FPU, so every
    /// such division is a software routine. §4.3's "intensity" is derived from
    /// this in fixed point instead; see [`Strike::intensity_milli`].
    ///
    /// The datasheet is also explicit that this figure has no physical unit and
    /// is not calibrated — it is only comparable between strikes on the same
    /// sensor, which is exactly what §4.3 uses it for.
    pub energy_raw: u32,
}

impl Strike {
    /// The reference's "intensity" figure, ×1000, in integer arithmetic.
    ///
    /// `energy_raw / 16777` on the reference's scale; here that is
    /// `energy_raw * 1000 / 16777`, which keeps three decimals without ever
    /// touching a float. §4.3 quotes observed intensities of 3–7, so this
    /// returns roughly 3000–7000.
    pub fn intensity_milli(&self) -> u32 {
        // u32 is safe: the field is 20 bits, so the largest product is
        // 0xFFFFF * 1000 = 1_048_575_000, inside u32::MAX.
        self.energy_raw * 1000 / 16777
    }
}
