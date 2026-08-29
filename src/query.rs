//! Reading a query string, and escaping what goes back out.
//!
//! Two halves, and the split is deliberate. [`command_from_query`] and the
//! escaping helpers are **pure** — no ESP-IDF, no globals — so they are
//! host-tested like the rest of this crate's decision logic. [`page`] renders,
//! which is a long string and nothing more.
//!
//! ## Everything goes through the console
//!
//! The page composes console command lines and hands them to the main loop; it
//! never reaches for the sensor, the log or NVS itself. That is what keeps the
//! two interfaces from drifting: there is one command table, one parser and one
//! set of range checks, and the web UI is a second way of typing into it rather
//! than a second implementation of it.
//!
//! It also decides the security model, such as it is. Anything reachable here
//! is reachable from the console by somebody holding the board, and the access
//! point is up for sixty seconds after a five-second press. There is no
//! authentication because the network's password is the authentication.

/// Turn `/do?cmd=...` into a console line.
///
/// Returns `None` for anything not recognised, which the caller logs and
/// discards. **An allow-list, not an escape-list**: a query naming a command
/// this does not know is refused rather than passed through, so a mistyped or
/// hostile URL cannot reach the console's parser with something unexpected.
pub fn command_from_query(query: &str) -> Option<String> {
    let mut command = None;
    let mut value = None;
    for pair in query.split('&') {
        let (key, raw) = pair.split_once('=')?;
        match key {
            "cmd" => command = Some(percent_decode(raw)),
            "v" => value = Some(percent_decode(raw)),
            _ => {}
        }
    }

    let command = command?;
    let value = value.unwrap_or_default();
    let value = value.trim();

    // The allow-list. Each arm names how many arguments the command takes,
    // because "reject an argument the command does not want" is cheaper here
    // than explaining a surprise later.
    match command.as_str() {
        // No argument.
        "indoor" | "outdoor" | "calibrate" | "clearstats" | "reboot" | "sync" => {
            Some(command)
        }
        // One argument, which the console's own parser range-checks.
        "defence" | "srej" | "wdth" | "quiet" | "merge" | "tz" | "events" | "strike" => {
            if value.is_empty() {
                return None;
            }
            // A space is the console's separator, so an argument containing one
            // would silently become two arguments.
            if value.contains(char::is_whitespace) {
                return None;
            }
            Some(format!("{command} {value}"))
        }
        // On or off.
        "sensitive" => match value {
            "on" | "off" => Some(format!("sensitive {value}")),
            _ => None,
        },
        _ => None,
    }
}

/// Decode `%20` and `+`.
///
/// Written out rather than pulled in: this needs one direction, no allocation
/// beyond the result, and no character set beyond ASCII. An invalid escape is
/// left as written, which is the forgiving choice — a stray `%` in a password
/// should not silently become something else.
pub fn percent_decode(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = String::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => {
                out.push(' ');
                index += 1;
            }
            b'%' if index + 2 < bytes.len() => {
                let hex = &raw[index + 1..index + 3];
                match u8::from_str_radix(hex, 16) {
                    Ok(byte) => {
                        out.push(byte as char);
                        index += 3;
                    }
                    Err(_) => {
                        out.push('%');
                        index += 1;
                    }
                }
            }
            byte => {
                out.push(byte as char);
                index += 1;
            }
        }
    }
    out
}

/// Escape the five characters that would otherwise end the element.
///
/// Every value on the page goes through this. Most of them are numbers the
/// firmware produced, but the SSID and the password are whatever somebody
/// typed, and a device name containing `<` should show a device name
/// containing `<`.
pub fn escape(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for character in raw.chars() {
        match character {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            other => out.push(other),
        }
    }
    out
}

/// `1h 02m` from seconds, for the uptime line.
pub fn duration(seconds: u32) -> String {
    let days = seconds / 86_400;
    let hours = (seconds % 86_400) / 3600;
    let minutes = (seconds % 3600) / 60;
    match (days, hours) {
        (0, 0) => format!("{minutes}m"),
        (0, _) => format!("{hours}h {minutes:02}m"),
        _ => format!("{days}d {hours}h {minutes:02}m"),
    }
}
