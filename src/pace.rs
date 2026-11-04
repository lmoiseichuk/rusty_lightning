//! lightning project, Copyright (c) 2026 Leonid Moiseichuk under MIT License
//!
//! When a walk over the whole strike log stops for a tick, so the task
//! watchdog cannot fire.
//!
//! **Free of ESP-IDF so the cadence is host-tested**, like `uptime` and
//! `budget`. This owns *when*; the block itself -- one `vTaskDelay` -- is
//! [`crate::log::yield_when_due`], which needs the device.
//!
//! ## What the watchdog watches
//!
//! `CONFIG_ESP_TASK_WDT_TIMEOUT_S` = 5 with `CONFIG_ESP_TASK_WDT_PANIC`, and
//! the one task it watches is the **idle** task, which feeds it every time it
//! runs. The idle task has the lowest priority there is and runs only when
//! every other task is blocked -- so any step of the main loop that goes five
//! seconds without blocking resets the board, however correct the step is.
//! Nothing else catches it: the loop's own blocking, in `notification.wait`,
//! happens only between steps.
//!
//! ## How `dump` reached it
//!
//! Measured 2026-11-01 on 0.21.1, awake at 160 MHz: a dump of 17,727 rows
//! (1,311,568 bytes) finished; ten minutes later, with 18,938 records, the
//! output stopped mid-row after 17,855 rows (1,319,116 bytes) and the board
//! came back saying `boot: TASK WATCHDOG`. Both fit one rate, about 264,000
//! bytes -- 3,570 of that log's rows -- a second, against a 5 s limit: the
//! first dump finished just inside it, the second needed 5.3 s. 0.20.1's
//! dumps earlier that day, up to 1,146,834 bytes, were simply shorter.
//!
//! The loop never blocked because a console write blocks only when the
//! USB-Serial/JTAG driver's 256-byte ring is full -- IDF 5.3.4's
//! `usb_serial_jtag_vfs.c` offers every character without waiting first --
//! and a host that reads as fast as the board writes never lets it fill. So
//! whether a dump survived depended on the reader as much as on the log, and
//! the log only grows.
//!
//! ## The cadence, and what it costs
//!
//! [`YIELD_INTERVAL_MS`] is 500, a tenth of the timeout. The longest the idle
//! task can then be kept out is one interval plus one step of the walk -- a
//! row, 0.28 ms at the measured rate -- so however long the walk, the idle
//! task is never kept out for much more than a tenth of the limit.
//! `tests/pace.rs` reads the timeout out of `sdkconfig.defaults` and refuses
//! an interval above a tenth of it.
//!
//! Each block is [`BLOCK_TICKS`] = **one tick**. `vTaskDelay(1)` returns at
//! the next tick interrupt, at most 10 ms away at `CONFIG_FREERTOS_HZ` = 100,
//! so the cost is at most one tick in fifty: about 2 %. The 18,938-record log
//! is ~1.40 MB, so 5.3 s of writing and at most ten ticks, 0.1 s, of blocking.
//!
//! **Timed rather than counted in rows**, because the rate is not one number:
//! it moves with the clock (`freq`), it follows the reader, and a step costs
//! different things in different walks -- a print in `dump`, a parse in the
//! replay, a network send in the web download. A clock does not care which.
//!
//! ## Who paces
//!
//! Every walk over the whole file: [`crate::log::dump_csv`],
//! [`crate::log::for_each`] (the replay into the charts, at boot), the record
//! count at boot and after `clear`, and the web page's `/log.csv`. The host
//! checks prove the cadence; that each walk calls it is read, not tested.

/// How long a walk may keep the CPU before it blocks for one tick.
///
/// A tenth of the task watchdog's 5 s -- see the module comment for the
/// arithmetic, and `tests/pace.rs` for the check against `sdkconfig.defaults`.
pub const YIELD_INTERVAL_MS: u32 = 500;

/// How long each block is, in FreeRTOS ticks: one.
///
/// **At least one, because zero is not a block.** `vTaskDelay(0)` is a yield,
/// which hands the CPU only to tasks of the caller's priority or higher -- and
/// the idle task is below all of them, so a walk "yielding" with zero would
/// keep it out exactly as before.
///
/// **Under `CONFIG_FREERTOS_IDLE_TIME_BEFORE_SLEEP`** (3), so a block can
/// never turn into a light sleep, which powers the USB PHY down and would take
/// the console away in the middle of a dump. One tick is the whole cost of a
/// block, at most 10 ms; `tests/pace.rs` holds both bounds against
/// `sdkconfig.defaults`.
pub const BLOCK_TICKS: u32 = 1;

/// When a long walk should block, timed from the last time it did.
pub struct Pace {
    /// `now_ms()` when the walk started, or when it was last told to block.
    since_ms: u32,
}

impl Pace {
    /// Start timing a walk that begins now.
    pub fn new(now_ms: u32) -> Self {
        Pace { since_ms: now_ms }
    }

    /// Whether the walk has kept the CPU for [`YIELD_INTERVAL_MS`].
    ///
    /// A `true` restarts the interval, so the caller blocks once and carries
    /// on. **Once, never a catch-up:** a walk that was held up for ten
    /// intervals -- a slow reader filling the console's ring, say -- gets one
    /// `true`, not ten. Being held up meant blocking, which already let the
    /// idle task in; owing it ten more ticks would only slow the walk down.
    ///
    /// Through `uptime::due`, so the 49.7-day wrap of `now_ms()` cannot make a
    /// walk look as though no time has passed.
    pub fn due(&mut self, now_ms: u32) -> bool {
        if !crate::uptime::due(now_ms, self.since_ms, YIELD_INTERVAL_MS) {
            return false;
        }
        self.since_ms = now_ms;
        true
    }
}
