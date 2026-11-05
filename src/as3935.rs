//! lightning project, Copyright (c) 2026 Leonid Moiseichuk under MIT License
//! Reworked from DFRobot's MicroPython driver for the SEN0290, `MicroPython/DFRobot_AS3935_Lib.py`
//! and its example `MicroPython/example/DFRobot_AS3935_detailed.py`
//! (https://github.com/DFRobot/DFRobot_AS3935), Copyright (c) 2020 DFRobot (MIT License; the
//! example's own header says Copyright DFRobot 2018, GNU Lesser General Public License).
//! DFRobot's licence, as the repository's `LICENSE` gives it:
//!
//! > MIT License
//! >
//! > Copyright (c) 2020 DFRobot
//! >
//! > Permission is hereby granted, free of charge, to any person obtaining a copy
//! > of this software and associated documentation files (the "Software"), to deal
//! > in the Software without restriction, including without limitation the rights
//! > to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
//! > copies of the Software, and to permit persons to whom the Software is
//! > furnished to do so, subject to the following conditions:
//! >
//! > The above copyright notice and this permission notice shall be included in all
//! > copies or substantial portions of the Software.
//! >
//! > THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
//! > IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
//! > FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
//! > AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
//! > LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
//! > OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
//! > SOFTWARE.
//!
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
//! ## Why not the `as3935-generic` crate
//!
//! Evaluated and rejected, recorded here so it is not re-litigated. It covers
//! the ordinary register set competently, but it has no `DISP_LCO` — and the
//! antenna self-test built on that (`set_irq_display_lco` below) is the single
//! highest-value piece of code in this project: it found two wiring bugs in
//! seconds and measured the resonant frequency at 499–500 kHz, which the
//! datasheet's own procedure says needs an oscilloscope. Losing it to save a
//! few hundred lines of register plumbing that is already written and verified
//! on hardware is the wrong trade.
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
//! | `0x08` | `0x0F` TUN_CAP, `0xE0` DISP_* (`0x40` DISP_SRCO pulsed by the calibration) | antenna tuning caps; what appears on IRQ |
//! | `0x3A` | `0x80` TRCO_CALIB_DONE, `0x40` TRCO_CALIB_NOK | did the timer RCO calibrate |
//! | `0x3B` | `0x80` SRCO_CALIB_DONE, `0x40` SRCO_CALIB_NOK | did the system RCO calibrate |
//! | `0x3C` | — | direct command: reset (write `0x96`) |
//! | `0x3D` | — | direct command: calibrate RCO (write `0x96`) |

use esp_idf_hal::i2c::I2cDriver;

// Re-exported so callers still say `as3935::Distance`: these describe a strike
// this driver decoded, and moving them out was about *compilation*, not about
// where they belong conceptually. See `strike.rs`.
pub use crate::strike::{Distance, Strike};
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
const REG_CALIB_TRCO: u8 = 0x3A;
const REG_CALIB_SRCO: u8 = 0x3B;
const CMD_RESET: u8 = 0x3C;
const CMD_CALIB_RCO: u8 = 0x3D;
/// The value both direct commands take. Not a register write — the chip
/// interprets it as "do the thing".
const DIRECT_COMMAND: u8 = 0x96;

const MASK_POWER_DOWN: u8 = 0x01;
const MASK_AFE_GAIN: u8 = 0x3E;
const MASK_NOISE_FLOOR: u8 = 0x70;
const MASK_WATCHDOG: u8 = 0x0F;
/// Only written by `set_spike_rejection`, which nothing calls -- see there.
#[allow(dead_code)]
const MASK_SPIKE_REJECT: u8 = 0x0F;
const MASK_MIN_STRIKES: u8 = 0x30;
const MASK_CLEAR_STATS: u8 = 0x40;
const MASK_DISTURBER: u8 = 0x20;
const MASK_INTERRUPT: u8 = 0x0F;
/// `DISP_SRCO`, register `0x08` **bit 6** -- the bit the datasheet's
/// calibration procedure pulses.
///
/// AS3935 datasheet v1-04 (ams, 2016-01-13), "Clock Generation", **page 36**:
///
/// > The output frequency of those oscillators can be displayed on the IRQ pin
/// > with register setting (REG0x08[5] =1 TRCO, while REG0x08[6] = 1 SRCO).
/// > [...] It is recommended to first trim the receiver antenna before the
/// > calibration of both oscillators is done. [...] the TRCO needs to be
/// > recalibrated using the following procedure:
/// > 1. Send Direct command CALIB_RCO
/// > 2. Modify REG0x08[6] = 1
/// > 3. Wait 2ms
/// > 4. Modify REG0x08[6] = 0
///
/// and the register map (pages 18-19) agrees: bit 7 `DISP_LCO`, bit 6
/// `DISP_SRCO`, bit 5 `DISP_TRCO`, bits 3:0 `TUN_CAP`.
///
/// **Until 0.22.0 this pulsed bit 5, `DISP_TRCO`, and a comment here defended
/// it.** The MicroPython reference wrote `0x20` under the comment
/// `#set DISP_SRCO to 1`; this file decided the comment was the mistake and
/// the value right, on the claim that "TRCO is the bit `CALIB_RCO` actually
/// requires" -- a claim with no source, which the procedure above contradicts.
/// The reference's comment named the datasheet's bit and its value did not.
///
/// **What would say this is wrong:** the banner line `as: RCO calibration`,
/// which reads `0x3A`/`0x3B` straight after. On 0.21.2, pulsing bit 5, it read
/// `TRCO done, SRCO done (0x3A=0x9C 0x3B=0x9E)`. If bit 6 reads anything but
/// `done` twice, that is the evidence to go back on, and this constant is the
/// whole of the change.
const MASK_DISPLAY_SRCO: u8 = 0x40;
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

/// Wait between the IRQ edge and reading the reason register.
///
/// **The datasheet says 2 ms minimum, and it is not optional** -- v1-04,
/// "Interrupt Management", page 34; the reference's comment below cites page
/// 22, presumably of an earlier revision. Read sooner and the reason nibble
/// reads as 0, which decodes as `Unknown` and loses the event entirely.
///
/// ## Why 3, and the 30 that was here before it
///
/// **This is the reference's value, checked against the file rather than
/// remembered.** `doc/DFRobot_AS3935_Lib.py`, retired in `a4e0b42bd67c` and
/// still recoverable from it:
///
/// ```python
/// def getInterruptSrc(self) -> int:
///     utime.sleep_ms(3) #wait 3ms before reading (min 2ms per pg 22 of datasheet)
/// ```
///
/// `sleep_ms(3)` is three milliseconds and the comment agrees with the code —
/// **but it is not the whole wait.** `deep_demo.py`'s interrupt handler sleeps
/// again before calling it:
///
/// ```python
/// def callback_handle(channel):
///     utime.sleep(0.005)
///     source = Core.sensor.getInterruptSrc()
/// ```
///
/// 5 ms in the handler plus 3 ms inside the read is **8 ms** between the edge
/// and the register access, and 8 ms is therefore the value that has detected
/// real strikes on this hardware. Reading either sleep on its own gets a
/// different number, which is how this constant has now been wrong twice.
///
/// It was 30 ms between `ff8f3714aad7` and here, on the claim that the
/// reference's comment said 3 ms while its code said `utime.sleep(0.03)`. **No
/// such line exists in the file.** The reasoning built on it — that a longer
/// wait lets the chip finish validating, and that sampling early presents a
/// strike as a disturber — was plausible, never tested against lightning, and
/// rested on a quotation that was not there.
///
/// What is real is the datasheet's **2 ms minimum** (above) for the interrupt
/// bits, which 3 ms clears with margin. If a longer settle is ever wanted
/// again, it needs evidence from a storm rather than from a misread reference:
/// §9 item 6 is still open precisely because nothing below
/// [`Interrupt::Lightning`] has been confirmed by one.
///
/// **A constant is not a wait.** Until 0.21.1 this was handed to
/// `FreeRtos::delay_ms`, which never waited 8 ms at all -- see [`settle`],
/// which is now the only thing that honours it.
pub const IRQ_SETTLE_MS: u32 = 8;

/// The LC oscillator's start-up time, and how long the RC oscillators take to
/// calibrate: **2 ms each**, datasheet v1-04, electrical characteristics,
/// page 9 (`LCOSUT`, and `TRCOCAL` -- "the calibration of the RC oscillators
/// starts after the LCO settles"). The calibration procedure's own "Wait 2ms"
/// (page 36) is the second.
///
/// Waited with [`wait_at_least_ms`], never with `FreeRtos::delay_ms(2)`: at
/// 100 Hz that is one tick (esp-idf-hal 0.46.2 rounds 2 ms up to one), which
/// returns at the next tick interrupt -- anything from a few microseconds to
/// 10 ms, depending on where in the tick it was called.
///
/// **What that did until 0.22.0, exactly**, because it matters if the banner
/// ever changes: the code had two such delays, and only the first was short
/// by chance. It ran between `CALIB_RCO` and the bit going up -- a gap the
/// procedure does not time -- from microseconds to 10 ms. The second, the
/// procedure's step 3, began a fraction of a millisecond after the first
/// returned on a tick, so it lasted to the next tick: about 9.7 ms on every
/// boot. The step-3 wait is 7.5-10 ms now for the same reason, with the 2 ms
/// guaranteed rather than an accident of where the previous wait ended. So of
/// 0.22.0's three changes to the calibration, the bit and the order changed
/// what the chip saw; this one did not.
pub const LCO_STARTUP_MS: u32 = 2;
pub const RCO_CALIBRATION_MS: u32 = 2;

/// Wait at least [`IRQ_SETTLE_MS`] before the reason register is read, yielding
/// to the scheduler on the way.
///
/// ## What `FreeRtos::delay_ms(8)` actually did
///
/// esp-idf-hal turns milliseconds into scheduler ticks, rounding up, and this
/// build ticks at `CONFIG_FREERTOS_HZ` = 100 -- so 8 ms became **one tick**.
/// `vTaskDelay(1)` does not wait a tick: it returns at the *next* tick
/// interrupt, which may be 10 ms away or a few microseconds. The settle was
/// spread evenly over 0-10 ms, and about one event in five was read inside the
/// datasheet's 2 ms -- v1-04, "Interrupt Management": *"After the signal IRQ
/// goes high the external unit should wait 2ms before reading the interrupt
/// register."* Read that early, the reason nibble can still be 0, or distance
/// and energy can be the previous event's; and the read itself lowers INT
/// (same section), so nothing raises it again. The event is simply gone,
/// counted as `Unknown`.
///
/// That was true on USB all along. On the cell it hardly arose, because an
/// event that came in while the chip light-slept was read only at the next
/// 1 s timer wake, long after INT rose. Since 0.21.0 the IRQ wakes the chip,
/// so every event on the cell is read straight after its interrupt -- the
/// path this function now guards.
///
/// ## The shape, and why both halves
///
/// **One tick first, always.** The main task has to block, however briefly, so
/// the idle task runs: a level interrupt that re-fires the moment it is
/// re-armed would otherwise have this task spinning from one pass to the next
/// until the task watchdog reset the board. A pure busy-wait would be exact and
/// would lose that.
///
/// **Then a busy-wait for whatever of the 8 ms the tick did not cover**, timed
/// on the microsecond clock rather than assumed. So the read lands 8-10 ms
/// after the task woke -- never under the reference's 8, at most one tick over
/// -- and the spin is only ever the shortfall: about 3 ms on average, none when
/// the tick itself took 8 ms or more. Timed from here rather than from the
/// interrupt, which only makes it longer, never shorter.
///
/// Rounding the tick count up by one instead (`vTaskDelay(2)`) would also
/// guarantee the minimum, at 10-20 ms -- twice as long awake on every event
/// on the cell, and further from the 8 ms that has heard lightning here.
pub fn settle() {
    wait_at_least_ms(IRQ_SETTLE_MS);
}

/// Wait **at least** `ms`, yielding one tick first: the shape [`settle`]
/// explains, for every datasheet wait in this driver.
///
/// One yielded tick, so the idle task runs whatever the caller is doing, and
/// then a busy-wait of whatever the tick did not cover, timed on the
/// microsecond clock rather than assumed. The wait ends between `ms` and one
/// tick past it -- never short, which is the property every caller needs: a
/// datasheet minimum met "on average" is a minimum missed some of the time.
///
/// Meant for the few milliseconds the chip asks for. The spin is at most `ms`,
/// so a caller asking for long waits should block instead.
pub fn wait_at_least_ms(ms: u32) {
    let wanted_us = i64::from(ms) * 1000;
    // SAFETY: the IDF's monotonic microsecond clock; a read with no side
    // effects. `vTaskDelay` is a plain FreeRTOS call from task context.
    let started_us = unsafe { esp_idf_hal::sys::esp_timer_get_time() };
    unsafe { esp_idf_hal::sys::vTaskDelay(1) };
    let elapsed_us = unsafe { esp_idf_hal::sys::esp_timer_get_time() } - started_us;
    if elapsed_us < wanted_us {
        // `Ets` is the ROM's busy-wait. Never more than `ms` here -- 8 ms at
        // the most this driver asks for -- so it cannot starve anything: the
        // tick above already let the idle task in.
        esp_idf_hal::delay::Ets::delay_us((wanted_us - elapsed_us) as u32);
    }
}

/// Bit 7 of `0x3A`/`0x3B`: the oscillator calibrated. Datasheet v1-04, the
/// register table and "Clock Generation".
const MASK_CALIB_DONE: u8 = 0x80;
/// Bit 6 of the same registers: the calibration ran and failed.
const MASK_CALIB_NOK: u8 = 0x40;

/// What one RC oscillator's calibration status register says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Calibration {
    /// `DONE` set, `NOK` clear: calibrated.
    Done,
    /// `NOK` set: the chip tried and failed.
    Failed,
    /// Neither bit: the calibration never finished, or never ran.
    NotDone,
    /// Both bits, which the datasheet does not describe. Reported raw.
    Contradictory,
}

impl Calibration {
    fn from_register(value: u8) -> Calibration {
        match (value & MASK_CALIB_DONE != 0, value & MASK_CALIB_NOK != 0) {
            (true, false) => Calibration::Done,
            (false, true) => Calibration::Failed,
            (false, false) => Calibration::NotDone,
            (true, true) => Calibration::Contradictory,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Calibration::Done => "done",
            Calibration::Failed => "FAILED",
            Calibration::NotDone => "NOT DONE",
            Calibration::Contradictory => "done AND failed",
        }
    }
}

/// One oscillator's calibration, decoded, with the raw register beside it.
pub type CalibrationReading = (Calibration, u8);

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

    /// Every register the design touches, straight off the chip.
    ///
    /// A diagnostic, and the reason it exists: when the sensor reports nothing,
    /// the question "is it configured the way we think it is" cannot be answered
    /// from our own state. Every other status line prints what we *believe* we
    /// wrote. This prints what the part actually holds — the two disagreeing is
    /// the whole class of bug that is otherwise invisible.
    pub fn dump_registers(&self, i2c: &mut I2cDriver<'_>) -> Result<[u8; 9], EspError> {
        let mut out = [0u8; 9];
        for (register, slot) in out.iter_mut().enumerate() {
            *slot = self.read(i2c, register as u8)?;
        }
        Ok(out)
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
        // The reference's 2 ms, as a real 2 ms -- see [`wait_at_least_ms`].
        wait_at_least_ms(2);
        Ok(())
    }

    /// Bring the AFE out of power-down, trim the antenna, and calibrate the
    /// internal oscillators -- **in the datasheet's order**.
    ///
    /// The RCO calibration is not optional after a reset: the chip's timing —
    /// and therefore its whole notion of what a lightning waveform looks like —
    /// depends on it. Page 36 of datasheet v1-04 gives the order, quoted at
    /// [`MASK_DISPLAY_SRCO`]: trim the antenna first, then `CALIB_RCO`, then
    /// `REG0x08[6]` up for 2 ms and down again.
    ///
    /// **Until 0.22.0 this ran before `TUN_CAP` was written**, so the
    /// oscillators were calibrated against the LC oscillator at 0 pF -- off its
    /// 500 kHz -- and the 120 pF that puts it there arrived half a second
    /// later; and it pulsed bit 5, `DISP_TRCO`, not the procedure's bit 6. Both
    /// are the datasheet's now. Its two 2 ms waits were one scheduler tick
    /// each, but the one the procedure times, step 3, came out at about 10 ms
    /// every boot (see [`LCO_STARTUP_MS`]), so it is now guaranteed rather than
    /// changed. `boot::configure` reads `0x3A`/`0x3B` straight after, and
    /// `boot::antenna_self_test` measures the trimmed antenna after that, as
    /// before.
    ///
    /// The pulse briefly routes SRCO (~1.1 MHz) to the IRQ pin, which is
    /// harmless because nothing is listening yet: `main` sets the pin's
    /// interrupt type only after this returns.
    pub fn power_up(&self, i2c: &mut I2cDriver<'_>, tuning_caps_pf: u8) -> Result<(), EspError> {
        self.modify(i2c, REG_CONFIG0, MASK_POWER_DOWN, 0x00)?;
        // Antenna first, and the LC oscillator given its start-up time at the
        // new capacitance -- the calibration measures against it, and starts
        // only once it has settled (page 9).
        self.set_tuning_caps(i2c, tuning_caps_pf)?;
        wait_at_least_ms(LCO_STARTUP_MS);
        // 1. CALIB_RCO.
        self.write(i2c, CMD_CALIB_RCO, DIRECT_COMMAND)?;
        // 2-4. REG0x08[6] = 1, wait 2 ms, REG0x08[6] = 0.
        self.modify(i2c, REG_TUNING, MASK_DISPLAY_SRCO, MASK_DISPLAY_SRCO)?;
        wait_at_least_ms(RCO_CALIBRATION_MS);
        self.modify(i2c, REG_TUNING, MASK_DISPLAY_SRCO, 0x00)
    }

    /// Whether the two RC oscillators calibrated, from `0x3A` and `0x3B`.
    ///
    /// **A report, and the evidence for the change 0.22.0 made.** The chip's
    /// timing -- and so its whole idea of what a lightning waveform looks like
    /// -- runs on these two oscillators. Until 0.22.0 [`Self::power_up`]
    /// calibrated them by pulsing `DISP_TRCO` (bit 5), as the reference did,
    /// before the antenna was trimmed; it now follows the datasheet's procedure
    /// (v1-04, "Clock Generation", page 36), which pulses `REG0x08[6]`,
    /// `DISP_SRCO`, after it. This reads what the chip says came of it, so a
    /// boot answers the question instead of an argument: 0.21.2 read
    /// `0x3A=0x9C 0x3B=0x9E`, both done.
    ///
    /// Returns `(timer RCO, system RCO)` with the raw bytes, which are printed
    /// beside the decoding for the same reason `regs` prints raw registers.
    pub fn calibration_status(
        &self,
        i2c: &mut I2cDriver<'_>,
    ) -> Result<(CalibrationReading, CalibrationReading), EspError> {
        let trco = self.read(i2c, REG_CALIB_TRCO)?;
        let srco = self.read(i2c, REG_CALIB_SRCO)?;
        Ok((
            (Calibration::from_register(trco), trco),
            (Calibration::from_register(srco), srco),
        ))
    }

    pub fn set_location(&self, i2c: &mut I2cDriver<'_>, location: Location) -> Result<(), EspError> {
        self.modify(i2c, REG_CONFIG0, MASK_AFE_GAIN, location.afe_gain())
    }

    /// Let disturber events raise the IRQ, or suppress them.
    ///
    /// **Masked by default** -- see the measurement in `boot::configure`. The
    /// bit is `MASK_DIST`, and it gates `INT_D` and nothing else: `INT_NH` and
    /// `INT_L` still reach the pin, so the noise-floor auto-tune keeps the only
    /// input it actually uses (`Tuning::observe` folds noise alone).
    ///
    /// This doc said the opposite until 2026-09-10, on the strength of a tuner
    /// that no longer exists. Kept as a note because the claim is plausible,
    /// was believed for weeks, and is the sort of thing that gets re-invented.
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

    /// The chip's current noise floor.
    ///
    /// **Uncalled since the ladder began enforcing its own floor at boot**, so
    /// the firmware always knows the value without asking. Kept because it is
    /// the only way to check the chip against what we believe about it, and a
    /// read-modify-write over a marginal bus is exactly the thing that can leave
    /// the two disagreeing.
    #[allow(dead_code)]
    pub fn noise_floor(&self, i2c: &mut I2cDriver<'_>) -> Result<u8, EspError> {
        Ok((self.read(i2c, REG_CONFIG1)? & MASK_NOISE_FLOOR) >> 4)
    }

    /// Watchdog threshold, 0–15. Higher is more robust to disturbers and less
    /// sensitive to real strikes.
    pub fn set_watchdog_threshold(&self, i2c: &mut I2cDriver<'_>, level: u8) -> Result<(), EspError> {
        self.modify(i2c, REG_CONFIG1, MASK_WATCHDOG, level & 0x0F)
    }

    /// Spike rejection, 0–15. Same trade as the watchdog, on a different stage.
    ///
    /// **Called at every boot** — `boot.rs` reads the stored level and writes it,
    /// and the `srej` console command sets it. This comment used to say the
    /// opposite: "deliberately uncalled", and that "the reference never writes
    /// SREJ". Both were false, and the second was checkable — the reference
    /// implementation writes it too.
    ///
    /// The correction matters because of what the old comment invited. Somebody
    /// trusting it would conclude this register is untouched, and the obvious
    /// next step from there — "so let us start writing it" — is exactly the
    /// change that once left the chip unable to validate a strike at all. The
    /// warning was real; the claim it rested on was not.
    ///
    /// What survives, and is the part worth keeping: **forcing this from its
    /// power-on default cost strike validation once.** The default is what a
    /// storm has been seen to work at. Move it only with evidence from a storm.
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
    /// **Uncalled**: the chip powers up at one strike, so writing it asserts a
    /// preference the datasheet already guarantees. `min_strikes` reads it back
    /// instead. Kept for the day something wants the pattern-matching behaviour.
    #[allow(dead_code)]
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

    /// How many strikes the chip will wait for before reporting, read back.
    ///
    /// A reader rather than a writer, because the reference never writes this
    /// and the power-on value is already 1 — so the honest thing is to report
    /// what the chip is doing rather than to assert a preference it already
    /// holds. See the init in `main` for why the write was removed.
    pub fn min_strikes(&self, i2c: &mut I2cDriver<'_>) -> Result<u8, EspError> {
        let bits = self.read(i2c, REG_CONFIG2)? & MASK_MIN_STRIKES;
        Ok(match bits {
            0x00 => 1,
            0x10 => 5,
            0x20 => 9,
            _ => 16,
        })
    }

    /// Discard the accumulated distance estimate.
    ///
    /// The AS3935 estimates distance from statistics gathered over a *storm*,
    /// not from a single strike — so the figure is only meaningful while the
    /// strikes it was built from belong to the same weather. When a storm has
    /// clearly ended, those statistics describe weather that is no longer there
    /// and will bias the first strike of the next one.
    ///
    /// **Called from `crate::session::StormWatch`, and it was not called until
    /// 0.6.0.** A device left running reported the nearest bin for every strike
    /// of a whole storm — 909 of them, with no variation as the cell approached,
    /// sat overhead and departed — while the same sensor driven from short-lived
    /// scripts produced a normal spread of kilometres. Short-lived scripts get a
    /// fresh estimator on every launch; this firmware runs for days.
    ///
    /// Note [`As3935::reset`] is **not** a substitute. `PRESET_DEFAULT` restores
    /// registers, and the datasheet treats clearing the statistics as a separate
    /// operation — so rebooting the board does not do this, and nothing else
    /// ever did.
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
    /// [`As3935::power_up`] re-runs the RCO calibration -- the datasheet's
    /// procedure on page 36 is written for exactly this, a chip back from
    /// power-down -- and skipping that leaves the chip timing-uncalibrated and
    /// its idea of a strike waveform wrong.
    #[allow(dead_code)]
    pub fn power_down(&self, i2c: &mut I2cDriver<'_>) -> Result<(), EspError> {
        self.modify(i2c, REG_CONFIG0, MASK_POWER_DOWN, MASK_POWER_DOWN)
    }

    /// Why the IRQ fired. **Reading this clears it**, which is what re-arms the
    /// pin — so it must be called exactly once per edge.
    ///
    /// The caller is responsible for having waited first -- [`settle`] -- when
    /// an interrupt announced the event. That wait is not done here because the
    /// poll reads this register with no interrupt to settle from, and burying a
    /// blocking delay inside a register read is how it ends up being paid twice.
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
