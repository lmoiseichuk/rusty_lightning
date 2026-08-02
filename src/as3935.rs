//! AS3935 franklin lightning sensor — register driver (§3).
//!
//! Ported from DFRobot's MicroPython library (`DFRobot_AS3935_Lib.py` +
//! `deep_demo.py`), which §8 made the behavioural spec.
//!
//! **Those files have been deleted, and this module is now the record.** The
//! port is complete and verified on hardware — the antenna self-test reads
//! 499–500 kHz through the same registers, and disturber decoding drives the
//! §4.2 auto-tune. Everything the reference knew is either implemented below or
//! written down here; nothing was left behind in a file that was going to rot.
//! Where this deliberately differs from the reference, the difference is noted
//! at the item.
//!
//! ## The one pattern not carried over
//!
//! The reference does its I2C reads **inside the GPIO interrupt callback**.
//! That is fine in MicroPython and wrong on esp-idf: an ISR runs in interrupt
//! context, where blocking on an I2C transaction — which is what
//! `i2c_master_transmit` does — is not allowed. It also has to sleep ≥3 ms
//! first, which is a thing an ISR must never do.
//!
//! So the split here is: **the ISR only notifies**, and the main task does the
//! wait, the reads, and the batching. See `main.rs`.
//!
//! ## Register map
//!
//! Only the registers this design touches. The AS3935's is small and the
//! datasheet's table numbering is more useful than a full mirror of it.
//!
//! | Reg | Bits used | What |
//! |---|---|---|
//! | `0x00` | `0x01` PWD, `0x3E` AFE_GB | power-down; indoor/outdoor gain |
//! | `0x01` | `0x70` NF_LEV, `0x0F` WDTH | noise floor; watchdog threshold |
//! | `0x02` | `0x30` MIN_NUM_LIGH, `0x0F` SREJ, `0x40` CL_STAT | min strikes; spike rejection; clear stats |
//! | `0x03` | `0x0F` INT, `0x20` MASK_DIST, `0xC0` LCO_FDIV | interrupt reason; disturber mask |
//! | `0x04..0x06` | energy | 20-bit strike energy, LSB first |
//! | `0x07` | `0x3F` DISTANCE | estimated distance to the head of the storm |
//! | `0x08` | `0x0F` TUN_CAP, `0xE0` DISP_* | antenna tuning caps; what appears on IRQ |
//! | `0x3C` | — | direct command: reset (write `0x96`) |
//! | `0x3D` | — | direct command: calibrate RCO (write `0x96`) |

use esp_idf_hal::i2c::I2cDriver;
use esp_idf_hal::sys::EspError;

const REG_CONFIG0: u8 = 0x00;
const REG_CONFIG1: u8 = 0x01;
const REG_CONFIG2: u8 = 0x02;
const REG_INTERRUPT: u8 = 0x03;
const REG_ENERGY_LSB: u8 = 0x04;
const REG_ENERGY_MSB: u8 = 0x05;
const REG_ENERGY_MMSB: u8 = 0x06;
const REG_DISTANCE: u8 = 0x07;
const REG_TUNING: u8 = 0x08;
const CMD_RESET: u8 = 0x3C;
const CMD_CALIB_RCO: u8 = 0x3D;
/// The value both direct commands take. Not a register write — the chip
/// interprets it as "do the thing".
const DIRECT_COMMAND: u8 = 0x96;

const MASK_POWER_DOWN: u8 = 0x01;
const MASK_AFE_GAIN: u8 = 0x3E;
const MASK_NOISE_FLOOR: u8 = 0x70;
const MASK_WATCHDOG: u8 = 0x0F;
const MASK_SPIKE_REJECT: u8 = 0x0F;
const MASK_MIN_STRIKES: u8 = 0x30;
const MASK_CLEAR_STATS: u8 = 0x40;
const MASK_DISTURBER: u8 = 0x20;
const MASK_INTERRUPT: u8 = 0x0F;
const MASK_DISPLAY_SRCO: u8 = 0x20;
const MASK_DISPLAY_LCO: u8 = 0x80;
const MASK_LCO_FDIV: u8 = 0xC0;
const MASK_IRQ_DISPLAY: u8 = 0xE0;
const MASK_TUNING_CAPS: u8 = 0x0F;
const MASK_DISTANCE: u8 = 0x3F;
const MASK_ENERGY_MMSB: u8 = 0x1F;

/// AFE gain for a sensor indoors. Roughly 4× the outdoor gain, because indoors
/// the signal arrives through a building.
const AFE_INDOOR: u8 = 0x24;
/// AFE gain for a sensor outdoors.
const AFE_OUTDOOR: u8 = 0x1C;

/// I2C transaction timeout, milliseconds.
const TIMEOUT_MS: u32 = 100;

/// What the antenna should resonate at, in kilohertz, and the tolerance the
/// datasheet allows.
pub const ANTENNA_NOMINAL_KHZ: u32 = 500;
pub const ANTENNA_TOLERANCE_PERCENT: u32 = 35; // tenths of a percent: 3.5 %

/// What `DISP_LCO` divides the antenna frequency by before putting it on the
/// pin, with `LCO_FDIV` at its default.
pub const LCO_DIVISOR: u32 = 16;

/// Minimum wait between the IRQ edge and reading the reason register.
///
/// **Datasheet page 22, and it is not optional.** The chip needs this long to
/// settle the interrupt bits after asserting the pin; read sooner and the
/// reason nibble reads as 0, which decodes as `Unknown` and loses the strike.
pub const IRQ_SETTLE_MS: u32 = 3;

/// Where the sensor is, which sets the AFE gain (§4.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Location {
    Indoor,
    Outdoor,
}

impl Location {
    fn afe_gain(self) -> u8 {
        match self {
            Location::Indoor => AFE_INDOOR,
            Location::Outdoor => AFE_OUTDOOR,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Location::Indoor => "indoor",
            Location::Outdoor => "outdoor",
        }
    }
}

/// Why the sensor raised its IRQ.
///
/// **The reference returns 1/2/3 for these; the datasheet register holds
/// 0x08/0x04/0x01.** Both numbering schemes appear in the spec, and mixing them
/// silently turns lightning into "unknown" — which is why this is an enum and
/// neither number escapes the module.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Interrupt {
    Lightning,
    Disturber,
    NoiseTooHigh,
    /// The register held something else. Usually means it was read too soon
    /// after the edge — see [`IRQ_SETTLE_MS`].
    Unknown(u8),
}

/// How far away the storm is.
///
/// The reference returns the raw 6-bit field and lets the caller treat it as
/// kilometres. **Two of its 64 values are not distances**, and conflating them
/// with real ones is how a storm directly overhead gets charted at 1 km
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

/// The sensor, and the bus address it answered on.
pub struct As3935 {
    address: u8,
}

impl As3935 {
    /// Addresses the AS3935 can be strapped to.
    ///
    /// All three are inside I2C's reserved 0x00–0x07 block, which is why the
    /// bus scan starts below 0x08.
    pub const ADDRESSES: [u8; 3] = [0x01, 0x02, 0x03];

    /// Find the sensor by resetting each candidate address until one answers.
    ///
    /// A reset rather than a bare probe: the reference does the same, and it is
    /// the stronger test — a device that merely acknowledges its address could
    /// be anything, while one that accepts a reset command and then reads back
    /// a plausible register is an AS3935.
    pub fn find(i2c: &mut I2cDriver<'_>) -> Option<Self> {
        for address in Self::ADDRESSES {
            let sensor = As3935 { address };
            if sensor.reset(i2c).is_ok() {
                return Some(sensor);
            }
        }
        None
    }

    pub fn address(&self) -> u8 {
        self.address
    }

    fn read(&self, i2c: &mut I2cDriver<'_>, register: u8) -> Result<u8, EspError> {
        let mut value = [0u8; 1];
        // `write_read` issues a repeated START between the two phases rather
        // than a STOP. The AS3935 requires that: a STOP would end the
        // transaction and drop the register pointer it was just given.
        i2c.write_read(self.address, &[register], &mut value, TIMEOUT_MS)?;
        Ok(value[0])
    }

    fn write(&self, i2c: &mut I2cDriver<'_>, register: u8, value: u8) -> Result<(), EspError> {
        i2c.write(self.address, &[register, value], TIMEOUT_MS)
    }

    /// Read-modify-write the bits under `mask`.
    ///
    /// `value` must already be shifted into position — the mask says *where*,
    /// not *how far*. That is the reference's convention and it is kept, because
    /// every caller here is writing a datasheet constant that is already in
    /// place.
    fn modify(
        &self,
        i2c: &mut I2cDriver<'_>,
        register: u8,
        mask: u8,
        value: u8,
    ) -> Result<(), EspError> {
        let current = self.read(i2c, register)?;
        self.write(i2c, register, (current & !mask) | (value & mask))
    }

    pub fn reset(&self, i2c: &mut I2cDriver<'_>) -> Result<(), EspError> {
        self.write(i2c, CMD_RESET, DIRECT_COMMAND)?;
        esp_idf_hal::delay::FreeRtos::delay_ms(2);
        Ok(())
    }

    /// Bring the AFE out of power-down and calibrate the internal oscillators.
    ///
    /// The RCO calibration is not optional after a reset: the chip's timing —
    /// and therefore its whole notion of what a lightning waveform looks like —
    /// depends on it. The `DISP_SRCO` toggle at the end is what the datasheet
    /// asks for to latch the calibration; it briefly routes SRCO to the IRQ pin,
    /// which is harmless because nothing is listening yet.
    pub fn power_up(&self, i2c: &mut I2cDriver<'_>) -> Result<(), EspError> {
        self.modify(i2c, REG_CONFIG0, MASK_POWER_DOWN, 0x00)?;
        self.write(i2c, CMD_CALIB_RCO, DIRECT_COMMAND)?;
        esp_idf_hal::delay::FreeRtos::delay_ms(2);
        self.modify(i2c, REG_TUNING, MASK_DISPLAY_SRCO, MASK_DISPLAY_SRCO)?;
        esp_idf_hal::delay::FreeRtos::delay_ms(2);
        self.modify(i2c, REG_TUNING, MASK_DISPLAY_SRCO, 0x00)
    }

    pub fn set_location(&self, i2c: &mut I2cDriver<'_>, location: Location) -> Result<(), EspError> {
        self.modify(i2c, REG_CONFIG0, MASK_AFE_GAIN, location.afe_gain())
    }

    /// Let disturber events raise the IRQ, or suppress them.
    ///
    /// Kept **enabled** by default, which looks wrong for a lightning detector
    /// and is not: §4.2's noise-floor auto-tune is driven by disturber events.
    /// Mask them and the sensor goes quiet, the auto-tune never raises the
    /// floor, and the first real storm arrives into a mis-tuned receiver.
    pub fn set_disturber_enabled(
        &self,
        i2c: &mut I2cDriver<'_>,
        enabled: bool,
    ) -> Result<(), EspError> {
        let bits = if enabled { 0x00 } else { MASK_DISTURBER };
        self.modify(i2c, REG_INTERRUPT, MASK_DISTURBER, bits)
    }

    /// Route the antenna's resonant oscillator (LCO) to the IRQ pin.
    ///
    /// This is the AS3935's built-in self-test, and it is worth more here than
    /// the datasheet lets on. With `DISP_LCO` set the pin carries **LCO ÷ 16**
    /// — a ~31 kHz square wave — instead of events. Two things fall out of
    /// that, and this design wants both:
    ///
    /// 1. **It proves the IRQ wire.** A pin that sees no edges while the sensor
    ///    is deliberately driving it at 31 kHz is not connected to the sensor.
    ///    Without this, "no strikes indoors" and "the IRQ is on the wrong pad"
    ///    look identical, and both look like a working detector on a quiet day.
    /// 2. **It replaces the scope in §3 step 5.** The antenna should resonate at
    ///    500 kHz ±3.5 %; measuring the pin and multiplying by 16 gives that
    ///    number in software, so tuning `TUN_CAP` no longer needs an instrument.
    ///
    /// Must be cleared again before the pin is useful as an interrupt.
    pub fn set_irq_display_lco(&self, i2c: &mut I2cDriver<'_>) -> Result<(), EspError> {
        // Also force LCO_FDIV to 0, i.e. divide by 16. It defaults there, but
        // the divider is in the same register as the interrupt mask and a
        // future edit could disturb it -- and a wrong divisor turns a correct
        // antenna into a failing one by a factor of four.
        self.modify(i2c, REG_INTERRUPT, MASK_LCO_FDIV, 0x00)?;
        self.modify(i2c, REG_TUNING, MASK_IRQ_DISPLAY, MASK_DISPLAY_LCO)
    }

    /// Stop routing internal oscillators to the IRQ pin.
    ///
    /// Must be cleared before the pin is useful as an interrupt: with any of
    /// `DISP_LCO/SRCO/TRCO` set, the pin carries a clock rather than an event,
    /// and every edge of it looks like a strike. This is also the tuning path
    /// the reference leaves commented out (§3 step 5).
    pub fn clear_irq_display(&self, i2c: &mut I2cDriver<'_>) -> Result<(), EspError> {
        self.modify(i2c, REG_TUNING, MASK_IRQ_DISPLAY, 0x00)
    }

    /// Antenna tuning capacitance, in picofarads.
    ///
    /// The chip takes 0–120 pF in steps of 8, so the value is divided by 8 and
    /// clamped. 120 pF is the reference's default and the SEN0290's factory
    /// setting; changing it needs a scope on the IRQ pin (§3 step 5), so it is
    /// exposed but should not be guessed at.
    pub fn set_tuning_caps(&self, i2c: &mut I2cDriver<'_>, picofarads: u8) -> Result<(), EspError> {
        let code = if picofarads > 120 { 0x0F } else { picofarads >> 3 };
        self.modify(i2c, REG_TUNING, MASK_TUNING_CAPS, code)
    }

    /// Noise floor level, 0–7 (§4.2). Higher rejects more, detects less.
    pub fn set_noise_floor(&self, i2c: &mut I2cDriver<'_>, level: u8) -> Result<(), EspError> {
        self.modify(i2c, REG_CONFIG1, MASK_NOISE_FLOOR, (level & 0x07) << 4)
    }

    pub fn noise_floor(&self, i2c: &mut I2cDriver<'_>) -> Result<u8, EspError> {
        Ok((self.read(i2c, REG_CONFIG1)? & MASK_NOISE_FLOOR) >> 4)
    }

    /// Watchdog threshold, 0–15. Higher is more robust to disturbers and less
    /// sensitive to real strikes.
    pub fn set_watchdog_threshold(&self, i2c: &mut I2cDriver<'_>, level: u8) -> Result<(), EspError> {
        self.modify(i2c, REG_CONFIG1, MASK_WATCHDOG, level & 0x0F)
    }

    /// Spike rejection, 0–15. Same trade as the watchdog, on a different stage.
    pub fn set_spike_rejection(&self, i2c: &mut I2cDriver<'_>, level: u8) -> Result<(), EspError> {
        self.modify(i2c, REG_CONFIG2, MASK_SPIKE_REJECT, level & 0x0F)
    }

    /// How many strikes must be detected before the sensor raises an interrupt.
    ///
    /// The chip offers 1, 5, 9 or 16 and rounds down to the nearest, so the
    /// value actually set is returned rather than assumed.
    ///
    /// **A disturber-rejection tool that costs latency, not sensitivity**, which
    /// makes it different in kind from the §4.2 ladder. At 1 the first strike
    /// reports immediately; at 5 the sensor waits for a pattern before saying
    /// anything, which a storm produces and a passing motor usually does not.
    /// The cost is that the first four strikes of a real storm are silent —
    /// unacceptable for a device whose job is early warning, which is why this
    /// is exposed but left at 1.
    pub fn set_min_strikes(&self, i2c: &mut I2cDriver<'_>, strikes: u8) -> Result<u8, EspError> {
        let (bits, actual) = match strikes {
            0..=4 => (0x00, 1),
            5..=8 => (0x10, 5),
            9..=15 => (0x20, 9),
            _ => (0x30, 16),
        };
        self.modify(i2c, REG_CONFIG2, MASK_MIN_STRIKES, bits)?;
        Ok(actual)
    }

    /// Discard the accumulated distance estimate.
    ///
    /// Not on the wake path yet: it belongs to §4.3's storm-end detection,
    /// which is unbuilt. Kept rather than deleted because it is a complete,
    /// correct operation waiting for a caller — not a stub standing in for one.
    #[allow(dead_code)]
    ///
    /// The AS3935 estimates distance from statistics gathered over a *storm*,
    /// not from a single strike — so the figure is only meaningful while the
    /// strikes it was built from belong to the same weather. When a storm has
    /// clearly ended, those statistics describe weather that is no longer there
    /// and will bias the first strike of the next one.
    ///
    /// Cleared by toggling `CL_STAT` high–low–high, which is the datasheet's
    /// sequence and not a mistake in the reference: the bit is edge-triggered,
    /// so writing it once does nothing.
    pub fn clear_statistics(&self, i2c: &mut I2cDriver<'_>) -> Result<(), EspError> {
        self.modify(i2c, REG_CONFIG2, MASK_CLEAR_STATS, MASK_CLEAR_STATS)?;
        self.modify(i2c, REG_CONFIG2, MASK_CLEAR_STATS, 0x00)?;
        self.modify(i2c, REG_CONFIG2, MASK_CLEAR_STATS, MASK_CLEAR_STATS)
    }

    /// Put the analogue front end to sleep.
    ///
    /// Kept for §7's battery mode. Note the wake path is not symmetric:
    /// [`As3935::power_up`] re-runs the RCO calibration, and skipping that
    /// leaves the chip timing-uncalibrated and its idea of a strike waveform
    /// wrong.
    #[allow(dead_code)]
    pub fn power_down(&self, i2c: &mut I2cDriver<'_>) -> Result<(), EspError> {
        self.modify(i2c, REG_CONFIG0, MASK_POWER_DOWN, MASK_POWER_DOWN)
    }

    /// Why the IRQ fired. **Reading this clears it**, which is what re-arms the
    /// pin — so it must be called exactly once per edge.
    ///
    /// The caller is responsible for having waited [`IRQ_SETTLE_MS`] first.
    /// That wait is not done here because this is called from the main task,
    /// which may have other things to do in those 3 ms, and burying a blocking
    /// delay inside a register read is how it ends up being paid twice.
    pub fn interrupt_reason(&self, i2c: &mut I2cDriver<'_>) -> Result<Interrupt, EspError> {
        let reason = self.read(i2c, REG_INTERRUPT)? & MASK_INTERRUPT;
        Ok(match reason {
            0x08 => Interrupt::Lightning,
            0x04 => Interrupt::Disturber,
            0x01 => Interrupt::NoiseTooHigh,
            other => Interrupt::Unknown(other),
        })
    }

    /// Distance and energy for the strike that just fired the IRQ.
    ///
    /// Read together because they describe one event and the chip overwrites
    /// both on the next one.
    pub fn strike(&self, i2c: &mut I2cDriver<'_>) -> Result<Strike, EspError> {
        let raw_distance = self.read(i2c, REG_DISTANCE)? & MASK_DISTANCE;
        let distance = match raw_distance {
            0x3F => Distance::OutOfRange,
            0x01 => Distance::Overhead,
            km => Distance::Km(km),
        };

        // 20 bits across three registers, most significant first. The MMSB
        // carries only its low 5 bits.
        let mmsb = (self.read(i2c, REG_ENERGY_MMSB)? & MASK_ENERGY_MMSB) as u32;
        let msb = self.read(i2c, REG_ENERGY_MSB)? as u32;
        let lsb = self.read(i2c, REG_ENERGY_LSB)? as u32;

        Ok(Strike {
            distance,
            energy_raw: (mmsb << 16) | (msb << 8) | lsb,
        })
    }
}
