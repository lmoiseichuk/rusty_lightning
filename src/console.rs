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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    /// `time <unix-epoch>` — set the clock (§5).
    SetTime(u64),
    /// `tz <hours>` — minutes east of UTC, given in hours (negative west).
    SetTz(i32),
    /// `strike [km] [intensity]` — inject a synthetic strike to exercise the
    /// path end to end. See the handler for why this exists.
    Simulate(u8, u32),
    /// `dump` — write the strike log as CSV.
    Dump,
    /// `clear` — erase the strike log and start a fresh file.
    Clear,
    /// `help`
    Help,
    /// Something was typed, but it was not a command. Still counts as activity.
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
    match parts.next() {
        Some("time") => match parts.next().and_then(|v| v.parse::<u64>().ok()) {
            Some(epoch) => Command::SetTime(epoch),
            None => Command::Unknown,
        },
        Some("tz") => match parts.next().and_then(|v| v.parse::<i32>().ok()) {
            // Given in hours because that is how people say it; stored in
            // minutes so a half-hour zone needs no format change.
            Some(hours) if (-12..=14).contains(&hours) => Command::SetTz(hours * 60),
            _ => Command::Unknown,
        },
        Some("strike") => {
            let km = parts.next().and_then(|v| v.parse::<u8>().ok()).unwrap_or(8);
            let intensity = parts
                .next()
                .and_then(|v| v.parse::<u32>().ok())
                .unwrap_or(4000);
            Command::Simulate(km, intensity)
        }
        Some("dump") => Command::Dump,
        Some("clear") => Command::Clear,
        Some("help") | Some("?") => Command::Help,
        _ => Command::Unknown,
    }
}

/// One screen of help.
pub fn print_help() {
    println!("commands:");
    println!("  time <unix-epoch>   set the clock, e.g. time 1785280179");
    println!("                      (date +%s on the host gives the number)");
    println!("  tz <hours>          local offset, e.g. tz -4 for US Eastern");
    println!("  strike [km] [int]   inject a synthetic strike (defaults 8 km, 4.0)");
    println!("  dump                write the strike log as CSV");
    println!("  clear               erase the strike log (no confirmation)");
    println!("  help                this");
    println!();
    println!("typing anything also keeps the device awake for 10 minutes --");
    println!("otherwise it light-sleeps and the USB port goes with it.");
}
