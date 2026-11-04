//! lightning project, Copyright (c) 2026 Leonid Moiseichuk under MIT License
//!
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

use std::cell::Cell;
use std::rc::Rc;

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

/// The longest any one of the driver's own BUSY waits may last.
///
/// **`epd-waveshare`'s waits have no timeout.** Its `wait_until_idle` polls
/// BUSY every 10 ms for as long as the controller holds it, and the driver runs
/// one before every frame, one after `PowerOn` in every wake, and one either
/// side of `PowerOff` in every sleep. A controller that never released BUSY --
/// a booster that cannot reach regulation on a sagging cell is the obvious
/// way -- would hold the main loop there for ever: the sensor unread, the
/// console dead, the last screen on the glass. The task watchdog would not
/// notice, because the poll yields and the watchdog only watches the idle
/// task.
///
/// Until 0.21.0 the `PowerOn` wait ran once, at boot. With the panel powered
/// down between refreshes it runs on every redraw, so it is bounded here.
///
/// **Two seconds against waits that should take tens of milliseconds** --
/// the same generosity as `wait_for_refresh`'s 500 ms start bound, which was
/// set against a measured "well under 100 ms". Nothing here has measured
/// `PowerOn`; the console prints every wake's time, which is the figure to
/// revisit this against.
const DRIVER_WAIT_LIMIT_MS: u32 = 2_000;

/// Whether the driver's BUSY waits are being timed, and how that went.
///
/// Shared between [`Panel`], which starts and ends the timing around each
/// driver call, and [`BusyPin`], which the driver owns and reads -- an `Rc`
/// because the pin is moved into the driver and there is no way back to it.
/// Single-threaded by construction: the panel lives in the main task.
#[derive(Clone, Copy)]
enum Watch {
    /// No bound: BUSY reads as the pad says.
    Off,
    /// A driver call is running, started at `started_ms` on `crate::now_ms`.
    Until { started_ms: u32 },
    /// The bound ran out during this call. BUSY now reads idle for the rest of
    /// it, so the waits after the one that gave up do not each wait again.
    GaveUp,
}

/// The BUSY pin, presented to the driver with [`BUSY_INVERTED`] applied.
///
/// Implemented over `embedded_hal::digital::InputPin`, which is the trait
/// `epd-waveshare` actually consumes — so the driver cannot tell the difference
/// and nothing downstream has to remember the polarity.
///
/// **And the timeout the driver does not have.** While a call is being timed
/// ([`Watch::Until`]), a BUSY still held past [`DRIVER_WAIT_LIMIT_MS`] reads as
/// idle, which is the only way to end the driver's wait without changing the
/// driver: its loop is `while busy`, so a pin that stops saying busy stops it.
pub struct BusyPin<'d> {
    pin: PinDriver<'d, Input>,
    watch: Rc<Cell<Watch>>,
}

impl BusyPin<'_> {
    /// Whether the panel is busy as the driver should see it.
    fn busy(&mut self) -> bool {
        let level_busy = if BUSY_INVERTED { self.pin.is_high() } else { self.pin.is_low() };
        if !level_busy {
            return false;
        }
        match self.watch.get() {
            Watch::Off => true,
            Watch::GaveUp => false,
            Watch::Until { started_ms } => {
                if crate::uptime::due(crate::now_ms(), started_ms, DRIVER_WAIT_LIMIT_MS) {
                    self.watch.set(Watch::GaveUp);
                    return false;
                }
                true
            }
        }
    }
}

impl embedded_hal::digital::ErrorType for BusyPin<'_> {
    type Error = core::convert::Infallible;
}

impl embedded_hal::digital::InputPin for BusyPin<'_> {
    // The driver asks `is_low` (its `IS_BUSY_LOW` is true), so "busy" is low
    // and "idle" is high, after the inversion and the bound above.
    fn is_high(&mut self) -> Result<bool, Self::Error> {
        Ok(!self.busy())
    }

    fn is_low(&mut self) -> Result<bool, Self::Error> {
        Ok(self.busy())
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
    /// Whether the controller has been sent to deep sleep since its last
    /// init, so the next [`Panel::show`] has to wake it first.
    ///
    /// Set after any attempt to sleep, successful or not: a power-down that
    /// failed half way leaves the controller in a state nothing here can name,
    /// and the wake is a hardware reset followed by the boot-time init, which
    /// is correct from any of them.
    asleep: bool,
    /// The other end of [`BusyPin`]'s bound -- see [`Panel::bounded`].
    watch: Rc<Cell<Watch>>,
}

/// What one [`Panel::show`] did, for the console line that reports it.
pub struct Refresh {
    /// How long the panel was busy refreshing. Zero means BUSY never fell:
    /// nothing was drawn.
    pub busy_ms: u32,
    /// How long waking it took first, when it had been put to sleep.
    pub woke_ms: Option<u32>,
    /// How long powering it down took afterwards, when it was. Timed for the
    /// same reason as the wake: both are loop time the sensor is not read in.
    pub slept_ms: Option<u32>,
    /// Whether it was powered down again afterwards.
    pub asleep: bool,
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

        let watch = Rc::new(Cell::new(Watch::Off));
        let busy = BusyPin {
            // Pulled toward the IDLE level, so a disconnected BUSY fails fast --
            // a refresh that returns immediately having drawn nothing -- rather
            // than hanging the device forever on a pin nobody is driving.
            pin: PinDriver::input(
                pins.busy,
                if BUSY_INVERTED { Pull::Down } else { Pull::Up },
            )?,
            watch: Rc::clone(&watch),
        };
        let dc = PinDriver::output(pins.dc)?;
        let rst = PinDriver::output(pins.rst)?;

        // The init is the same `PowerOn` and wait a wake runs, so it is bounded
        // the same way. Done by hand rather than through `bounded`, because
        // there is no `Panel` yet to call it on.
        let mut delay = Delay::new_default();
        watch.set(Watch::Until { started_ms: crate::now_ms() });
        let made = Epd7in5::new(&mut spi, busy, dc, rst, &mut delay, None);
        let gave_up = matches!(watch.get(), Watch::GaveUp);
        watch.set(Watch::Off);
        if gave_up {
            println!("epd:  ⚠ init: BUSY held past {DRIVER_WAIT_LIMIT_MS} ms -- carried on without it");
        }
        // The driver reports SPI errors; there is nothing more specific to map
        // them to, and the caller only needs "the panel did not come up".
        let epd = made.map_err(|_| EspError::from_infallible::<{ esp_idf_hal::sys::ESP_FAIL }>())?;

        Ok(Self { epd, spi, asleep: false, watch })
    }

    /// Run one driver call with its BUSY waits bounded by
    /// [`DRIVER_WAIT_LIMIT_MS`], and say so if the bound was needed.
    ///
    /// The bound restarts with each call rather than each wait inside it, so a
    /// call is worst-case one limit long: once one wait gives up, BUSY reads
    /// idle for the rest of the call. Carrying on is the point -- a frame sent
    /// to a controller still busy is a frame not drawn, and the console says
    /// which call gave up and how long the redraw took; a loop that never comes
    /// back says nothing at all. A BUSY stuck low for good still costs every
    /// redraw its bounds -- these, and `wait_for_refresh`'s 15 s -- but the loop
    /// runs between them, and the console with it.
    fn bounded(
        &mut self,
        what: &str,
        call: impl FnOnce(
            &mut PanelDriver<'d>,
            &mut SpiDeviceDriver<'d, SpiDriver<'d>>,
        ) -> Result<(), EspError>,
    ) -> Result<(), EspError> {
        self.watch.set(Watch::Until { started_ms: crate::now_ms() });
        let result = call(&mut self.epd, &mut self.spi);
        let gave_up = matches!(self.watch.get(), Watch::GaveUp);
        self.watch.set(Watch::Off);
        if gave_up {
            println!("epd:  ⚠ {what}: BUSY held past {DRIVER_WAIT_LIMIT_MS} ms -- carried on without it");
        }
        result
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
    ///
    /// ## Waking before, and sleeping after
    ///
    /// With `sleep_after` the controller is powered down once the refresh has
    /// finished, and woken again at the start of the next call. See
    /// [`Panel::sleep`] for why, and what the wake costs. Everything between
    /// -- the frame, the refresh, the wait -- is the same either way, so the
    /// two settings can be compared on what they change and nothing else.
    pub fn show(&mut self, frame: &Display7in5, sleep_after: bool) -> Result<Refresh, EspError> {
        let woke_ms = match self.asleep {
            true => Some(self.wake()?),
            false => None,
        };

        // The driver waits for BUSY before it sends the frame -- unbounded in
        // the driver, bounded here like every other driver call.
        self.bounded("frame", |epd, spi| {
            let mut delay = Delay::new_default();
            epd.update_and_display_frame(spi, frame.buffer(), &mut delay)
                .map_err(|_| EspError::from_infallible::<{ esp_idf_hal::sys::ESP_FAIL }>())
        })?;
        let busy_ms = Self::wait_for_refresh();

        let slept_ms = match sleep_after {
            true => {
                let started = crate::now_ms();
                if let Err(e) = self.sleep() {
                    println!("epd:  could not power the panel down -- {e}");
                }
                // Either way: see `asleep` for why a failed power-down still
                // counts.
                self.asleep = true;
                Some(crate::uptime::since(crate::now_ms(), started))
            }
            false => None,
        };

        Ok(Refresh { busy_ms, woke_ms, slept_ms, asleep: self.asleep })
    }

    /// Bring the controller out of deep sleep. Returns how long it took, in ms.
    ///
    /// `epd-waveshare`'s `wake_up` for this panel *is* its `init`: a hardware
    /// reset, then the power settings, `PowerOn`, and a wait for BUSY. So a
    /// wake is exactly what every boot already does before the logo -- the
    /// one path on this board that has drawn correctly from the first day.
    fn wake(&mut self) -> Result<u32, EspError> {
        let started = crate::now_ms();
        self.bounded("wake", |epd, spi| {
            let mut delay = Delay::new_default();
            epd.wake_up(spi, &mut delay)
                .map_err(|_| EspError::from_infallible::<{ esp_idf_hal::sys::ESP_FAIL }>())
        })?;
        self.asleep = false;
        Ok(crate::uptime::since(crate::now_ms(), started))
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

    /// Power the controller down. The image stays on the glass with no power.
    ///
    /// `Epd7in5::sleep` waits for BUSY, sends `PowerOff` -- which stops the
    /// panel's step-up converter -- waits again, then `DeepSleep`.
    ///
    /// ## Why the redraw path now calls it
    ///
    /// **This was deliberately never called**, on the argument that sleeping
    /// between redraws adds a wake-and-init to every refresh of a device that
    /// is usually on USB. What that argument did not weigh is what stays
    /// running instead. Without `PowerOff` the UC8179 leaves its booster on
    /// after the first refresh, and on Seeed's driver board that booster is an
    /// inductor switcher -- L1 (10 µH) and Q1, driven from the controller's GDR
    /// pin -- a few centimetres from a receiver tuned to 500 kHz. The AS3935
    /// datasheet's list of noise sources to avoid starts with "inductor based
    /// DC-DC converters". On 2026-11-01 the receiver indoors heard six to ten
    /// disturbers a second with a floor of 60-80 ms between them -- a
    /// continuous local source -- and in September it logged 9,542 strikes,
    /// 99 % `overhead`, arriving 6-9 s after another strike: just when a
    /// strike's own redraw lands. Neither is proven to be the panel. Both are
    /// what it would look like.
    ///
    /// So `panel sleep on` -- the default -- calls this after every refresh,
    /// and `panel sleep off` restores the old behaviour, so the two can be
    /// compared on the same board in the same weather.
    ///
    /// ## What the wake costs, from the driver rather than a bench
    ///
    /// `wake_up` is `init`: the reset holds RST high 10 ms, low 2 ms, then
    /// waits 200 ms. `Delay` hands anything from a millisecond up to
    /// `vTaskDelay`, which rounds to the 10 ms tick, so that is 190-220 ms of
    /// fixed cost. Then `PowerOn` and a wait for BUSY, polled every 10 ms, for
    /// however long the booster takes to come up -- which nothing here has
    /// measured. The power-down after the refresh is a BUSY wait, `PowerOff`,
    /// another BUSY wait for the booster to stop, and one command byte. Both
    /// are timed and printed with every redraw that paid them (`woke in N ms`,
    /// `powered down in N ms`), and both waits are bounded -- see
    /// [`DRIVER_WAIT_LIMIT_MS`].
    ///
    /// **It is loop time, and the loop is the only thing that reads the
    /// sensor.** Nothing reads `0x03` while the panel is woken, refreshed or
    /// put down; INT stays high and the level interrupt takes the event
    /// afterwards, but the AS3935 keeps only the latest reason, so a second
    /// event in the meantime replaces the first. The refresh itself (3.8 s)
    /// was always that; the wake and the power-down lengthen it by a quarter
    /// of a second or more.
    ///
    /// **How much that matters is the weather's to decide.** At the five-minute
    /// baseline it is a few seconds an hour. In a storm a strike earns a redraw
    /// five seconds after the last one finished (`screen`'s strike gap), so
    /// redraws come about every nine seconds -- some four hundred an hour, the
    /// panel already holding the loop for about 43 % of it -- and the quarter
    /// second each adds is 100 s an hour or more, taking that to about 45 %.
    /// That is the price of the comparison while `panel sleep on` is on trial;
    /// if the panel turns out not to be the noise source, `off` refunds it.
    pub fn sleep(&mut self) -> Result<(), EspError> {
        self.bounded("power-down", |epd, spi| {
            let mut delay = Delay::new_default();
            epd.sleep(spi, &mut delay)
                .map_err(|_| EspError::from_infallible::<{ esp_idf_hal::sys::ESP_FAIL }>())
        })
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
