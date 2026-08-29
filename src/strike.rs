//! What a strike is, with no hardware attached.
//!
//! ## Why these two types live apart from the driver that produces them
//!
//! `history` and the score formula operate on strikes and nothing else — no
//! I2C, no registers, no ESP-IDF. Keeping them in `as3935` meant that anything
//! importing a `Distance` also imported the whole HAL, which is what forced
//! `tests/` to hand-copy the logic it wanted to test rather than compiling
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

/// The largest value the chip's energy field can hold: 20 bits.
///
/// Named because two places need it and they used to state it separately —
/// `intensity_milli` in a comment, and the `strike` console command not at all.
pub const ENERGY_RAW_MAX: u32 = 0xF_FFFF;

/// The largest intensity a real strike can carry, in thousandths.
///
/// Anything above this describes an energy the 20-bit field cannot represent,
/// so it is not a stronger strike — it is not a strike.
pub const INTENSITY_MILLI_MAX: u32 = ENERGY_RAW_MAX * 1000 / 16777;

/// Invert [`Strike::intensity_milli`]: the raw energy that would produce it.
///
/// **Clamped, and this is the whole point of the function.** The obvious
/// expression, `milli * 16777 / 1000`, overflows `u32` above 256_003 — and the
/// console's `strike` command parses an unbounded `u32`, so a typo reached it.
/// In a debug build that panics inside the wake loop, where `listen`'s rule is
/// that nothing may panic; in release it wraps silently and feeds a garbage
/// energy into the rings, the score and the CSV, where it is indistinguishable
/// from a real reading.
///
/// Clamping rather than widening to `u64`: `u64` would compute a number
/// faithfully, but a faithful answer to "how much energy is 300 000 milli" is
/// still an energy no strike can have. The ceiling is the physical one.
pub fn energy_for_intensity(intensity_milli: u32) -> u32 {
    let clamped = intensity_milli.min(INTENSITY_MILLI_MAX);
    clamped * 16777 / 1000
}
