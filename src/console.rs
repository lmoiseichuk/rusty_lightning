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

/// What `freq` was asked to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FreqRequest {
    /// Bare `freq` — report, change nothing.
    Report,
    /// `freq auto` — hand the clock back to §7's policy.
    Auto,
    /// `freq <mhz>` — hold this clock, light sleep off, until `freq auto`.
    Pin(u32),
}

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
    /// `freq` / `freq auto` / `freq <mhz>` — read or pin the CPU clock.
    Freq(FreqRequest),
    /// `battery` — level, voltage, flow direction, and the raw gauge registers.
    Battery,
    /// `sleep on|off` — light sleep alone, leaving the clock where it is.
    ///
    /// Separate from [`Command::Freq`] because they answer different questions:
    /// the clock is about speed, light sleep is about whether the USB port
    /// survives. Only deep sleep is absent, and deliberately — it loses RAM, so
    /// the rings would have to be rebuilt from CSV on every wake, and this
    /// device has no reason to use it.
    Sleep(bool),
    /// `mode indoor|outdoor` — AFE gain, same switch as the BOOT button.
    SetMode(bool),
    /// `regs` — dump the sensor's registers as the chip actually holds them.
    Regs,
    /// `sensitive on|off` — force every rejection knob to its minimum and
    /// freeze the §4.2 auto-tune there.
    /// Search every parameter for the quietest combination (§4.2).
    /// `calibrate [seconds] [events-per-minute]`.
    Calibrate(u32, u32),
    /// `defence` shows the point; `defence <raw>` sets it.
    Defence(Option<u16>),
    Sensitive(bool),
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
    let arg2 = parts.next();

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

        // `calibrate [seconds]` -- a bad or missing number falls back to the
        // default rather than refusing the command, because the argument is a
        // refinement and the sweep is the point.
        // The starting point, by hand. `defence` alone reports where the
        // machine is; `defence <raw>` puts it somewhere, which is how a room
        // with a known answer skips the sweep entirely.
        "defence" | "point" => match arg {
            None => Command::Defence(None),
            Some(value) => match value.parse::<u16>() {
                Ok(raw) => Command::Defence(Some(raw)),
                Err(_) => Command::Unknown,
            },
        },

        // `calibrate [seconds] [events-per-minute]`. Both optional and
        // positional: the window is the one people reach for, the threshold is
        // the one they set once for a room. A bad number is refused rather than
        // silently defaulted -- a sweep is fourteen minutes, and running the
        // wrong one because a typo fell back to a default is worse than being
        // told to try again.
        "calibrate" | "cal" => {
            let seconds = match arg {
                None => Some(crate::session::CALIBRATE_PROBE_S),
                Some(value) => value.parse::<u32>().ok(),
            };
            let quiet = match arg2 {
                None => Some(u32::MAX),
                Some(value) => match value.parse::<u32>() {
                    Ok(rate) if rate <= crate::session::QUIET_PER_MIN_MAX => Some(rate),
                    _ => None,
                },
            };
            match (seconds, quiet) {
                (Some(seconds), Some(quiet)) => Command::Calibrate(seconds, quiet),
                _ => Command::Unknown,
            }
        }
        "sensitive" | "sens" => match arg {
            Some("on") => Command::Sensitive(true),
            Some("off") => Command::Sensitive(false),
            _ => Command::Unknown,
        },
        "freq" => match arg {
            None => Command::Freq(FreqRequest::Report),
            Some("auto") => Command::Freq(FreqRequest::Auto),
            Some(mhz) => match mhz.parse::<u32>() {
                Ok(mhz) => Command::Freq(FreqRequest::Pin(mhz)),
                Err(_) => Command::Unknown,
            },
        },
        "battery" | "batt" => Command::Battery,
        "sleep" => match arg {
            Some("on") => Command::Sleep(true),
            Some("off") => Command::Sleep(false),
            _ => Command::Unknown,
        },
        "mode" => match arg {
            Some("indoor") => Command::SetMode(true),
            Some("outdoor") => Command::SetMode(false),
            _ => Command::Unknown,
        },
        "regs" => Command::Regs,
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
    println!("  mode indoor|outdoor   AFE gain -- outdoor is LOWER gain, for close storms");
    println!();
    println!("diagnostics:");
    println!("  regs                  the sensor's registers, off the chip and decoded");
    println!("  battery               level, voltage, charging/idle/discharging, raw gauge");
    println!(
        "  defence [raw]         show the tuning point, or set it (0-{}, 0 = most sensitive)",
        crate::defence::MAX
    );
    println!("  calibrate [s] [/min]  bisect the whole space for the most sensitive quiet point");
    println!(
        "                        s    seconds per probe ({}-{}, default {}); ~13 probes",
        crate::session::CALIBRATE_PROBE_MIN_S,
        crate::session::CALIBRATE_PROBE_MAX_S,
        crate::session::CALIBRATE_PROBE_S
    );
    println!(
        "                        /min events per minute counting as quiet (0-{}, default {})",
        crate::session::QUIET_PER_MIN_MAX,
        crate::session::QUIET_PER_MIN
    );
    println!("                             stored in NVS; omit to keep the current one");
    println!("  sensitive on|off      every knob wide open, auto-tune frozen");
    println!("  freq [auto|40|80|160] read the clock, or pin it");
    println!("  sleep on|off          light sleep alone -- off is what keeps USB alive");
    println!();
    println!("Typing anything also keeps the device awake for 10 minutes.");
    println!("Otherwise it light-sleeps and the USB port goes with it.");
    println!("`freq 160` holds it open indefinitely; `freq auto` gives it back.");
    println!("Full guide: doc/console.md");
}
