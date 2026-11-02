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

/// Every command the page may send, and how many arguments it takes.
///
/// **This is the drift hazard, so it is one list.** The first version was
/// written from memory rather than from `console`'s command table and five of
/// its seven action buttons named words the console does not know -- `indoor`,
/// `outdoor`, `reboot`, `sync`, `quiet`. Each composed a line, the console
/// parsed it to `Unknown`, and the page silently did nothing. The comment above
/// this module claimed the single command table prevents exactly that.
///
/// It cannot be derived from `console::parse`, which is a `match` over string
/// literals with no list to read. So it is checked instead: `verify` runs every
/// entry through the real parser at boot and reports any the console rejects.
///
/// **That check verified this list and nothing else.** The page's own buttons
/// were string literals in `webui`, and four of them -- `indoor`, `outdoor`,
/// `sync`, `reboot` -- plus a `quiet` field went on naming words that are not
/// here, so the allow-list dropped them before the parser ever saw them: the
/// same five dead buttons this comment records being fixed, back again in the
/// one place nothing checked. The page is now built from [`ACTIONS`],
/// [`FIELDS`] and [`TOGGLES`] below, and the crate refuses to compile if any of
/// them is not something this list lets through -- see `page_is_allowed`.
pub const COMMANDS: &[(&str, Arity)] = &[
    ("mode indoor", Arity::None),
    ("mode outdoor", Arity::None),
    ("calibrate", Arity::None),
    ("clearstats", Arity::None),
    ("dump", Arity::None),
    ("golden", Arity::None),
    ("status", Arity::None),
    ("health", Arity::None),
    ("regs", Arity::None),
    ("defence", Arity::One),
    ("srej", Arity::One),
    ("wdth", Arity::One),
    ("merge", Arity::One),
    ("tz", Arity::One),
    ("events", Arity::One),
    ("strike", Arity::One),
    // **`off` only.** `sensitive on` freezes the noise floor at 0, which is the
    // swamped end of the ladder, not the sensitive one -- measured here at
    // 300-480 noise a minute and no strikes indoors. The page offered it as
    // "open up and freeze", one tap from a phone. It stays a console command,
    // where somebody has read `help` first; from the network the only thing
    // the page may do with it is let go.
    ("sensitive", Arity::Choice(&["off"])),
    ("mask", Arity::Choice(&["on", "off"])),
    ("panel sleep", Arity::Choice(&["on", "off"])),
    ("scope", Arity::Choice(&["day", "week", "month"])),
];

// --- what the page offers ------------------------------------------------------

/// A button that sends one fixed command line.
pub struct Action {
    pub command: &'static str,
    /// The argument, or empty for a command that takes none.
    pub value: &'static str,
    pub label: &'static str,
}

/// A text box whose contents become the command's one argument.
pub struct Field {
    pub command: &'static str,
    pub label: &'static str,
    pub hint: &'static str,
}

/// An on/off setting, shown as one button that says where it is and offers the
/// other way.
pub struct Toggle {
    pub command: &'static str,
    /// The button while the setting is on; pressing it sends `off`.
    pub while_on: &'static str,
    /// The button while it is off; pressing it sends `on`.
    pub while_off: &'static str,
}

/// The page's action buttons.
pub const ACTIONS: &[Action] = &[
    Action { command: "mode indoor", value: "", label: "indoor gain" },
    Action { command: "mode outdoor", value: "", label: "outdoor gain" },
    Action { command: "calibrate", value: "", label: "calibrate the noise floor" },
    Action { command: "clearstats", value: "", label: "clear the distance estimate" },
];

/// Shown only while `sensitive on` is in force, which only the console can do.
pub const RELEASE: Action = Action {
    command: "sensitive",
    value: "off",
    label: "release the auto-tune",
};

/// The page's settings. `quiet` is gone: there is no such command -- the
/// threshold is `calibrate`'s second argument -- so the field never did
/// anything. `tz` takes **hours**, which is what the console's `tz` takes;
/// the field said minutes and offered `-300`, which the console refuses.
pub const FIELDS: &[Field] = &[
    Field { command: "tz", label: "time zone (hours)", hint: "-5" },
    Field { command: "defence", label: "noise floor 0-7", hint: "3" },
    Field { command: "srej", label: "spike rejection 0-15", hint: "2" },
    Field { command: "wdth", label: "watchdog 0-15", hint: "0" },
    Field { command: "merge", label: "merge window ms", hint: "1000" },
    Field { command: "events", label: "log N noise/disturber rows", hint: "5000" },
];

/// The two experiments, as toggles: the page is the only way to change them on
/// a board running from its cell, where connecting a cable restarts it.
pub const TOGGLES: &[Toggle] = &[
    Toggle {
        command: "mask",
        while_on: "disturbers masked -- unmask them",
        while_off: "disturbers UNMASKED -- mask them",
    },
    Toggle {
        command: "panel sleep",
        while_on: "panel sleeps between refreshes -- keep it powered",
        while_off: "panel left powered -- power it down",
    },
];

/// Whether a line `cmd value` would get through [`command_from_query`].
///
/// A `const fn`, so the answer is known when the firmware is compiled. Written
/// with `while` loops and byte comparisons because those are what a constant
/// context allows -- iterators and `==` on `str` are not available there.
const fn allowed(command: &str, value: &str) -> bool {
    let mut index = 0;
    while index < COMMANDS.len() {
        let (name, arity) = COMMANDS[index];
        if same(name, command) {
            return match arity {
                Arity::None => value.is_empty(),
                Arity::One => !value.is_empty() && !has_whitespace(value),
                Arity::Choice(choices) => {
                    let mut choice = 0;
                    while choice < choices.len() {
                        if same(choices[choice], value) {
                            return true;
                        }
                        choice += 1;
                    }
                    false
                }
            };
        }
        index += 1;
    }
    false
}

/// `a == b` for a constant context.
const fn same(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut index = 0;
    while index < a.len() {
        if a[index] != b[index] {
            return false;
        }
        index += 1;
    }
    true
}

const fn has_whitespace(text: &str) -> bool {
    let bytes = text.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index].is_ascii_whitespace() {
            return true;
        }
        index += 1;
    }
    false
}

/// Every button, field and toggle on the page, checked against [`COMMANDS`].
///
/// A field is checked with a stand-in argument, since its real one is typed.
const fn page_is_allowed() -> bool {
    let mut index = 0;
    while index < ACTIONS.len() {
        if !allowed(ACTIONS[index].command, ACTIONS[index].value) {
            return false;
        }
        index += 1;
    }
    let mut index = 0;
    while index < FIELDS.len() {
        if !allowed(FIELDS[index].command, "1") {
            return false;
        }
        index += 1;
    }
    let mut index = 0;
    while index < TOGGLES.len() {
        if !allowed(TOGGLES[index].command, "on") || !allowed(TOGGLES[index].command, "off") {
            return false;
        }
        index += 1;
    }
    allowed(RELEASE.command, RELEASE.value)
}

// **A dead button is a compile error.** Evaluated when the crate is built --
// for the firmware and for the host checks alike -- so a page entry naming a
// word `COMMANDS` does not let through stops the build rather than shipping a
// button that silently does nothing. `const _` rather than a named constant
// because an unnamed one is always evaluated, used or not.
//
// What it cannot see is whether the *console* knows the word: `console::parse`
// is a `match`, not a list a constant can read. `console::verify_web_commands`
// checks that at boot, against the real parser, for every line `page_lines`
// can produce.
const _: () = assert!(
    page_is_allowed(),
    "a web page button, field or toggle names a command query::COMMANDS does not allow"
);

/// How many arguments a command takes, and of what shape.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Arity {
    /// Takes none; an argument in the query is ignored rather than appended.
    None,
    /// Takes exactly one, which `console` range-checks.
    One,
    /// Takes one of a fixed set of words.
    ///
    /// **Separate from [`Arity::One`] because a number is not a word.** The
    /// boot check fed `scope 1` to the parser and the parser rejected it:
    /// `scope` takes `day`, `week` or `month`, and a sample built as "the name
    /// plus 1" is not a command. That was the sixth dead entry in this list,
    /// found by the check written for the first five.
    Choice(&'static [&'static str]),
}

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

    let (name, arity) = COMMANDS.iter().find(|(name, _)| *name == command)?;

    match arity {
        Arity::None => Some((*name).to_string()),
        Arity::One => {
            if value.is_empty() {
                return None;
            }
            // A space is the console's separator, so an argument containing one
            // would silently become two arguments.
            if value.contains(char::is_whitespace) {
                return None;
            }
            Some(format!("{name} {value}"))
        }
        Arity::Choice(allowed) => {
            if allowed.contains(&value) {
                Some(format!("{name} {value}"))
            } else {
                None
            }
        }
    }
}

/// A representative line for each entry, for the boot check.
///
/// Arguments are chosen to be in range for every command that takes one, since
/// the point is to test that the console *knows the word*, not that it accepts
/// a particular value.
pub fn samples() -> impl Iterator<Item = String> {
    COMMANDS.iter().map(|(name, arity)| match arity {
        Arity::None => (*name).to_string(),
        Arity::One => format!("{name} 1"),
        // The first choice, which is a real value by construction.
        Arity::Choice(allowed) => format!("{name} {}", allowed[0]),
    })
}

/// Every console line the page itself can send, for the boot check.
///
/// The same lines the buttons compose, so the check runs what a person can
/// actually press rather than a sample of what the allow-list would accept.
/// Fields carry the stand-in argument `1`, which every one of them accepts.
pub fn page_lines() -> impl Iterator<Item = String> {
    let actions = ACTIONS.iter().chain(core::iter::once(&RELEASE)).map(|action| {
        match action.value {
            "" => action.command.to_string(),
            value => format!("{} {value}", action.command),
        }
    });
    let fields = FIELDS.iter().map(|field| format!("{} 1", field.command));
    let toggles = TOGGLES.iter().flat_map(|toggle| {
        [format!("{} on", toggle.command), format!("{} off", toggle.command)]
    });
    actions.chain(fields).chain(toggles)
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

