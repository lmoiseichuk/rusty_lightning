//! Commands in over USB, and the awake signal that comes with them.
//!
//! ## Why the console decides the power policy
//!
//! Three schemes for "is this device on USB" were tried and each failed in a
//! case that happens (see `power`). This one cannot fail the same way, because
//! it is not detecting a *cable* — it is detecting a **person**. Somebody typing
//! is the clearest possible evidence that the console matters right now, and
//! when nobody has typed for ten minutes it evidently does not.
//!
//! ## Reading without blocking
//!
//! The wake loop must not stall waiting for input that will usually never come,
//! so stdin is put into non-blocking mode once at startup and polled. A blocking
//! read here would freeze the sensor, the screen and everything else behind a
//! console nobody is using.

use std::io::Read;

/// What the console can be asked to do.
///
/// Deliberately a small, flat vocabulary: one word, at most two arguments, no
/// modes and no state between lines. A device console is read by a person at a
/// terminal *and* piped from a host script, and anything cleverer than this
/// serves neither well.
///
/// The read side is non-blocking (see [`Console::new`]), so an idle console
/// costs one failed `read` per loop and never stalls the sensor or the screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    Help,
    /// `date` shows the clock; `date <unix-epoch>` sets it (§5).
    Date(Option<u64>),
    /// `tz <hours>` — local offset, negative west of UTC.
    SetTz(i32),
    /// `scope day|week|month` — which span the charts cover.
    Scope(u8),
    /// `health` — the device's own vitals, i.e. the top line of the screen.
    Health,
    /// `status` — what the sensor is doing, i.e. the second line.
    Status,
    /// `dump` — the strike log as CSV.
    Dump,
    /// `clear` — erase the strike log.
    Clear,
    /// `strike [km] [intensity]` — inject a synthetic strike, because a real
    /// one cannot be provoked; see the handler.
    Simulate(u8, u32),
    /// Typed, but not understood. Still counts as activity.
    Unknown,
}

/// Longest line accepted. Long enough for `time 1785280179`, and a bound is
/// wanted anyway: input on an embedded device should never be able to grow a
/// buffer without limit.
const MAX_LINE: usize = 64;

pub struct Console {
    buffer: heapless::Vec<u8, MAX_LINE>,
}

impl Console {
    /// Make stdin readable, then non-blocking.
    ///
    /// **The driver install is the part that is easy to miss.** With
    /// `CONFIG_ESP_CONSOLE_USB_SERIAL_JTAG` the console is write-only by
    /// default: `println!` works, and every read from stdin returns nothing
    /// forever. It is not an error and nothing reports it — commands simply
    /// vanish, which is exactly how the first version of this failed.
    ///
    /// Two calls fix it: install the USB-Serial/JTAG driver, then point VFS at
    /// it so `stdin` routes through the driver rather than the primitive
    /// write-only path.
    pub fn new() -> Self {
        // SAFETY: plain IDF setup calls, made once before anything reads stdin.
        unsafe {
            let config = esp_idf_hal::sys::usb_serial_jtag_driver_config_t {
                tx_buffer_size: 256,
                rx_buffer_size: 256,
            };
            let err = esp_idf_hal::sys::usb_serial_jtag_driver_install(
                &config as *const _ as *mut _,
            );
            if err == esp_idf_hal::sys::ESP_OK {
                esp_idf_hal::sys::esp_vfs_usb_serial_jtag_use_driver();
            } else {
                println!("con:  USB-Serial/JTAG driver would not install -- input disabled");
            }

            // Non-blocking, so the wake loop never stalls waiting for input that
            // will usually never come. A blocking read here would freeze the
            // sensor, the screen and everything else behind an idle console.
            let flags = esp_idf_hal::sys::fcntl(0, esp_idf_hal::sys::F_GETFL as i32, 0);
            esp_idf_hal::sys::fcntl(
                0,
                esp_idf_hal::sys::F_SETFL as i32,
                flags | esp_idf_hal::sys::O_NONBLOCK as i32,
            );
        }
        Self {
            buffer: heapless::Vec::new(),
        }
    }

    /// Collect any input and return a command once a line is complete.
    ///
    /// Returns `None` when nothing has been typed *or* when a line is still
    /// being typed — the caller uses `Some` as the activity signal, so a
    /// half-finished line deliberately does not count until it is sent.
    pub fn poll(&mut self) -> Option<Command> {
        let mut byte = [0u8; 1];
        loop {
            match std::io::stdin().read(&mut byte) {
                Ok(1) => {
                    let c = byte[0];
                    if c == b'\n' || c == b'\r' {
                        if self.buffer.is_empty() {
                            continue;
                        }
                        let line = core::str::from_utf8(&self.buffer)
                            .unwrap_or("")
                            .trim()
                            .to_owned();
                        self.buffer.clear();
                        return Some(parse(&line));
                    }
                    // A line longer than the buffer is not a command. Drop it
                    // rather than truncating into something that might parse.
                    if self.buffer.push(c).is_err() {
                        self.buffer.clear();
                    }
                }
                // Nothing available, or the console is not readable. Either way
                // there is nothing to do this pass.
                _ => return None,
            }
        }
    }
}

fn parse(line: &str) -> Command {
    let mut parts = line.split_whitespace();
    let word = parts.next().unwrap_or("");
    let arg = parts.next();

    match word {
        "help" | "?" => Command::Help,

        // `date` reads, `date <epoch>` writes. One word for one concept beats
        // `date` to show and `time` to set, which is the sort of split nobody
        // remembers the right half of.
        "date" | "time" => match arg {
            None => Command::Date(None),
            Some(value) => match value.parse::<u64>() {
                Ok(epoch) => Command::Date(Some(epoch)),
                Err(_) => Command::Unknown,
            },
        },

        "tz" => match arg.and_then(|v| v.parse::<i32>().ok()) {
            // Hours in, minutes stored -- see `clock::tz_minutes`.
            Some(hours) if (-12..=14).contains(&hours) => Command::SetTz(hours * 60),
            _ => Command::Unknown,
        },

        "scope" | "chart" => match arg {
            Some("day") => Command::Scope(0),
            Some("week") => Command::Scope(1),
            Some("month") => Command::Scope(2),
            _ => Command::Unknown,
        },

        "health" => Command::Health,
        "status" => Command::Status,
        "dump" => Command::Dump,
        "clear" => Command::Clear,

        "strike" => Command::Simulate(
            arg.and_then(|v| v.parse::<u8>().ok()).unwrap_or(8),
            parts.next().and_then(|v| v.parse::<u32>().ok()).unwrap_or(4000),
        ),

        _ => Command::Unknown,
    }
}

/// One screen of help.
pub fn print_help() {
    println!("commands:");
    println!("  help                  this");
    println!("  date [unix-epoch]     show the clock, or set it");
    println!("  tz <hours>            local offset, e.g. tz -4 for US Eastern");
    println!("  scope day|week|month  which span the charts cover");
    println!("  health                device vitals -- the screen's top line");
    println!("  status                sensor state -- the screen's second line");
    println!("  dump                  the strike log as CSV");
    println!("  clear                 erase the strike log (no confirmation)");
    println!("  strike [km] [int]     inject a synthetic strike (default 8 km, 4000)");
    println!();
    println!("Typing anything also keeps the device awake for 10 minutes.");
    println!("Otherwise it light-sleeps and the USB port goes with it.");
}
