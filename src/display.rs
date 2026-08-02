//! The 7.5" panel — hardware only (§6).
//!
//! UC8179 controller, 800×480, monochrome, on Seeed's XIAO ePaper driver board.
//! Nothing here decides *what* to draw; that is `ui`.
//!
//! ## Three things this board does differently
//!
//! **1. BUSY is NOT inverted, whatever §6 says.** The spec warns to "mind the
//! board's inverted BUSY", and that warning is wrong for this unit — measured,
//! not assumed. Reading the pad either side of initialisation:
//!
//! ```text
//! epd:  BUSY pad (GPIO4) is LOW    <- immediately after init: panel busy
//! epd:  BUSY before draw: HIGH     <- a few ms later: panel idle
//! ```
//!
//! LOW while working, HIGH when done, which is stock Waveshare polarity and
//! exactly what `epd-waveshare`'s hard-coded `IS_BUSY_LOW = true` expects. An
//! inverting adapter was written first, on the strength of the spec, and it hung
//! every refresh: the driver read "idle" as "busy" and waited forever.
//!
//! [`BUSY_INVERTED`] keeps the choice explicit rather than deleting the
//! question, because a different revision of this board could well need it —
//! and the two failure modes are hard to tell apart from the outside. Wrong one
//! way, refreshes block forever; wrong the other, they return in milliseconds
//! having drawn nothing.
//!
//! **2. The pads must be un-muxed before use.** `PinDriver` calls
//! `gpio_set_direction`, not `gpio_reset_pin` — and it is the latter that
//! returns a pad's IO_MUX selection to plain GPIO. This bit us on GPIO21 with
//! the sensor IRQ, and it is worse here: several display pins are ADC or
//! strapping pads with peripheral functions selected at reset.
//!
//! **3. The framebuffer does not fit on the stack.** 800 × 480 × 1bpp is
//! **48 000 bytes**, against a main task stack of 32 KB. It has to live on the
//! heap, and it has to get there without ever being built as a temporary — see
//! [`Panel::frame`].


use epd_waveshare::color::Color;
use epd_waveshare::epd7in5_v2::{Display7in5, Epd7in5};
use epd_waveshare::prelude::*;
// `Delay`, not `Ets`. `Ets` is a busy-wait — it spins on `ets_delay_us` without
// ever yielding, and a UC8179 full refresh takes **seconds**. Spinning that long
// starves the idle task, the task watchdog fires, and with
// CONFIG_ESP_TASK_WDT_PANIC the device reboots mid-refresh. That presents as a
// boot loop right after "panel up", with the draw never completing.
//
// `Delay` busy-waits only below its threshold and hands longer waits to
// `vTaskDelay`, so the scheduler keeps running while the panel does its work.
use esp_idf_hal::delay::Delay;
use esp_idf_hal::gpio::{AnyIOPin, Gpio10, Gpio2, Gpio3, Gpio4, Gpio5, Gpio8, Input, Output, PinDriver, Pull};
use esp_idf_hal::spi::config::Config as SpiConfig;
use esp_idf_hal::spi::{SpiDeviceDriver, SpiDriver, SpiDriverConfig, SPI2};
use esp_idf_hal::sys::EspError;
use esp_idf_hal::units::Hertz;

pub const WIDTH: u32 = 800;
pub const HEIGHT: u32 = 480;

/// Ink and paper, with this panel's inversion already applied.
///
/// **Measured, and the opposite of what the crate's names suggest.** Drawing
/// `Color::Black` on a `Color::White` background produced white text on a black
/// screen — legible, and completely inverted. On this board the crate's `White`
/// renders as ink and its `Black` renders as paper.
///
/// `epd-waveshare` is not wrong: it sends `Color::White` as `0xFF`, correct for
/// a stock Waveshare 7.5" V2. Seeed's panel reads those bits the other way.
///
/// Naming them by **role** is what keeps this to one place. Every drawing call
/// in `ui` asks for INK or PAPER and never mentions black or white, so a board
/// that does not invert becomes a two-line change here rather than a hunt
/// through the layout — and nobody has to remember mid-layout that "black"
/// means white.
pub const INK: Color = Color::White;
pub const PAPER: Color = Color::Black;

/// SPI clock to the panel.
///
/// The UC8179 tolerates far more, but 800×480 is 48 000 bytes per frame and the
/// refresh itself takes seconds — the transfer is not the bottleneck, so there
/// is nothing to buy by pushing the clock over ribbon cable and headers.
const SPI_HZ: u32 = 4_000_000;

/// Display pins, fixed by Seeed's driver board (§2).
pub struct Pins<'d> {
    pub spi: SPI2<'d>,
    pub sclk: Gpio8<'d>,
    pub mosi: Gpio10<'d>,
    pub cs: Gpio3<'d>,
    pub dc: Gpio5<'d>,
    pub rst: Gpio2<'d>,
    pub busy: Gpio4<'d>,
}

/// Whether this board inverts BUSY on its way to the MCU.
///
/// **False, measured** — see the module comment. Kept as a constant so the
/// question stays visible and a different board revision is a one-line change.
const BUSY_INVERTED: bool = false;

/// The BUSY pin, presented to the driver with [`BUSY_INVERTED`] applied.
///
/// Implemented over `embedded_hal::digital::InputPin`, which is the trait
/// `epd-waveshare` actually consumes — so the driver cannot tell the difference
/// and nothing downstream has to remember the polarity.
pub struct BusyPin<'d> {
    pin: PinDriver<'d, Input>,
}

impl embedded_hal::digital::ErrorType for BusyPin<'_> {
    type Error = core::convert::Infallible;
}

impl embedded_hal::digital::InputPin for BusyPin<'_> {
    fn is_high(&mut self) -> Result<bool, Self::Error> {
        Ok(if BUSY_INVERTED { self.pin.is_low() } else { self.pin.is_high() })
    }

    fn is_low(&mut self) -> Result<bool, Self::Error> {
        Ok(if BUSY_INVERTED { self.pin.is_high() } else { self.pin.is_low() })
    }
}

type PanelDriver<'d> = Epd7in5<
    SpiDeviceDriver<'d, SpiDriver<'d>>,
    BusyPin<'d>,
    PinDriver<'d, Output>,
    PinDriver<'d, Output>,
    Delay,
>;

pub struct Panel<'d> {
    epd: PanelDriver<'d>,
    spi: SpiDeviceDriver<'d, SpiDriver<'d>>,
}

impl<'d> Panel<'d> {
    /// Bring the panel up: un-mux the pads, start SPI, reset and initialise.
    pub fn new(pins: Pins<'d>) -> Result<Self, EspError> {
        // Un-mux every display pad before any driver touches it. See the module
        // comment — `PinDriver` alone does not do this, and a pad still owned by
        // a peripheral ignores everything written to it.
        //
        // SAFETY: plain IDF calls on pads nothing else owns yet.
        unsafe {
            for pad in [2, 3, 4, 5, 8, 10] {
                esp_idf_hal::sys::gpio_reset_pin(pad);
            }
        }

        let driver = SpiDriver::new(
            pins.spi,
            pins.sclk,
            pins.mosi,
            // The panel is write-only: there is no MISO to read back from.
            None::<AnyIOPin>,
            &SpiDriverConfig::new(),
        )?;
        let mut spi = SpiDeviceDriver::new(
            driver,
            Some(pins.cs),
            &SpiConfig::new().baudrate(Hertz(SPI_HZ)),
        )?;

        let busy = BusyPin {
            // Pulled toward the IDLE level, so a disconnected BUSY fails fast --
            // a refresh that returns immediately having drawn nothing -- rather
            // than hanging the device forever on a pin nobody is driving.
            pin: PinDriver::input(
                pins.busy,
                if BUSY_INVERTED { Pull::Down } else { Pull::Up },
            )?,
        };
        let dc = PinDriver::output(pins.dc)?;
        let rst = PinDriver::output(pins.rst)?;

        let mut delay = Delay::new_default();
        let epd = Epd7in5::new(&mut spi, busy, dc, rst, &mut delay, None)
            // The driver reports SPI errors; there is nothing more specific to
            // map them to, and the caller only needs "the panel did not come up".
            .map_err(|_| EspError::from_infallible::<{ esp_idf_hal::sys::ESP_FAIL }>())?;

        Ok(Self { epd, spi })
    }

    /// A blank framebuffer, on the heap.
    ///
    /// **`Box::new(Display7in5::default())` is not enough.** That builds the
    /// 48 000-byte value as a temporary first and then moves it, and the
    /// temporary is on a 32 KB stack. Debug builds overflow reliably; release
    /// builds sometimes elide the copy, which is worse — it works until an
    /// unrelated change stops it eliding.
    ///
    /// Allocating zeroed memory and initialising in place avoids the temporary
    /// entirely. `Display7in5`'s buffer is white when zeroed, which is also the
    /// blank state we want.
    pub fn frame() -> Box<Display7in5> {
        // `vec![]` of zeroes goes through `alloc_zeroed`, which never touches
        // the stack, and the box conversion is a pointer move.
        let zeroed = vec![0u8; core::mem::size_of::<Display7in5>()].into_boxed_slice();
        let raw = Box::into_raw(zeroed) as *mut Display7in5;
        // SAFETY: `Display7in5` is a plain struct over a byte array with no
        // padding invariants and no Drop, and the allocation is exactly its
        // size and at least its alignment (1). An all-zero bit pattern is a
        // valid, all-white buffer.
        unsafe { Box::from_raw(raw) }
    }

    /// Push a framebuffer to the glass and wait out the refresh.
    ///
    /// Returns how long the panel was actually busy, in milliseconds.
    ///
    /// ## Why this waits again after the driver already claimed to
    ///
    /// `update_and_display_frame` ends in the driver's own `wait_until_idle`,
    /// and **that wait races the panel**. It polls BUSY immediately after
    /// issuing the refresh command, before the UC8179 has asserted it, sees a
    /// still-idle line and returns. Measured here:
    ///
    /// ```text
    /// epd:  drawn in 116 ms          <- what the driver reported
    /// epd:  watching BUSY ... starts LOW
    /// epd:    + 3787 ms  ->  HIGH    <- when the panel actually finished
    /// ```
    ///
    /// 116 ms is just the SPI transfer: 48 000 bytes at 4 MHz. The refresh took
    /// nearly four seconds more, during which the driver believed it was done.
    /// Anything issued in that window — another frame, a sleep command, a deep
    /// sleep — lands on a panel mid-refresh.
    ///
    /// So the sequence here is: send, wait for BUSY to *fall* (the refresh has
    /// begun), then wait for it to *rise* (it has finished). Waiting for the
    /// fall first is what closes the race — without it, this wait would return
    /// instantly for exactly the same reason the driver's does.
    ///
    /// Full refresh every time. §6 leaves partial refresh open for this panel;
    /// until that is settled, full is the one known to produce a correct image.
    pub fn show(&mut self, frame: &Display7in5) -> Result<u32, EspError> {
        let mut delay = Delay::new_default();
        self.epd
            .update_and_display_frame(&mut self.spi, frame.buffer(), &mut delay)
            .map_err(|_| EspError::from_infallible::<{ esp_idf_hal::sys::ESP_FAIL }>())?;

        Ok(Self::wait_for_refresh())
    }

    /// Block until the panel finishes refreshing. Returns the busy time in ms.
    ///
    /// A zero return means BUSY never fell — the panel took the data and did not
    /// start a refresh, which is the failure that otherwise looks like success.
    pub fn wait_for_refresh() -> u32 {
        /// How long to wait for the panel to *start*. Measured at well under
        /// 100 ms; this is generous.
        const START_TIMEOUT_MS: u32 = 500;
        /// How long to allow the refresh itself. Measured ~3.9 s.
        const REFRESH_TIMEOUT_MS: u32 = 15_000;
        /// Poll interval.
        ///
        /// **100 ms, and the number is load-bearing.** Two thresholds sit just
        /// below it, and 20 ms — the obvious choice — fails both:
        ///
        /// * yielding at all, so the idle task runs and the task watchdog stays
        ///   quiet. A busy-wait here rebooted the device mid-refresh;
        /// * clearing FreeRTOS's light-sleep threshold, which is
        ///   `CONFIG_FREERTOS_IDLE_TIME_BEFORE_SLEEP` = 3 ticks at
        ///   `CONFIG_FREERTOS_HZ` = 100 — i.e. **30 ms**. A shorter delay yields
        ///   but never sleeps, so the single longest wait in the system, a 3.8 s
        ///   panel refresh, would hold the core awake at full clock for the
        ///   whole of it.
        ///
        /// The moisture project shipped this bug for weeks at 10 ms. The cost is
        /// up to 100 ms of latency on a refresh that takes 3800.
        const POLL_MS: u32 = 100;

        let mut waited = 0;
        while Self::busy_level() && waited < START_TIMEOUT_MS {
            esp_idf_hal::delay::FreeRtos::delay_ms(POLL_MS);
            waited += POLL_MS;
        }
        if Self::busy_level() {
            // Never went busy: nothing was drawn.
            return 0;
        }

        let mut busy_ms = 0;
        while !Self::busy_level() && busy_ms < REFRESH_TIMEOUT_MS {
            esp_idf_hal::delay::FreeRtos::delay_ms(POLL_MS);
            busy_ms += POLL_MS;
        }
        busy_ms
    }

    /// Put the panel to sleep. The image stays on the glass with no power.
    ///
    /// Not called on the redraw path, deliberately. `Epd7in5::sleep` issues
    /// `DeepSleep`, and coming back from it needs a full re-init — so sleeping
    /// between redraws would add a wake-and-init to every refresh of a device
    /// that is usually USB-powered (§7). It is kept because battery operation
    /// will want it, and because the panel holds its image either way.
    #[allow(dead_code)]
    pub fn sleep(&mut self) -> Result<(), EspError> {
        let mut delay = Delay::new_default();
        self.epd
            .sleep(&mut self.spi, &mut delay)
            .map_err(|_| EspError::from_infallible::<{ esp_idf_hal::sys::ESP_FAIL }>())
    }

    /// The raw level on the BUSY pad, straight from the GPIO register.
    ///
    /// Read by pad number rather than through the driver, because the pin has
    /// been moved into `Epd7in5` and there is no way back out — and this has to
    /// remain answerable precisely when the driver is stuck waiting on it.
    ///
    /// **This is the measurement the inversion question turns on.** `epd-waveshare`
    /// hard-codes `IS_BUSY_LOW = true`; §6 says this board inverts. If the two
    /// disagree, `wait_until_idle` either returns instantly and draws nothing, or
    /// blocks forever — and from the outside those look like "no image" and "a
    /// hang", neither of which points at a polarity bit.
    pub fn busy_level() -> bool {
        // SAFETY: reading a pad level has no side effects and needs no ownership.
        unsafe { esp_idf_hal::sys::gpio_get_level(4) != 0 }
    }

    /// What the BUSY pad says, in terms of what it means for a refresh.
    pub fn busy_verdict() -> &'static str {
        match (Self::busy_level(), BUSY_INVERTED) {
            (true, false) => "HIGH = idle -- a refresh should start",
            (false, false) => "LOW = busy -- the panel is still working",
            (true, true) => "HIGH = busy (inverted) -- the panel is still working",
            (false, true) => "LOW = idle (inverted) -- a refresh should start",
        }
    }
}
