//! The page the access point serves.
//!
//! The parsing and escaping live in [`crate::query`], which is free of
//! ESP-IDF so it can be host-tested; this half is the rendering, which is a
//! long string and nothing more.
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

use crate::portal::Snapshot;
use crate::query::{duration, escape};
pub use crate::query::command_from_query;

// --- the page ---------------------------------------------------------------

/// Style and structure in one string.
///
/// **No external anything.** The device is a captive portal with no route to
/// the internet, so a stylesheet, a font or a script loaded from a CDN is a
/// request that hangs and then fails — leaving an unstyled page and a spinner.
/// Everything the page needs travels with it.
const STYLE: &str = "\
<style>
:root{color-scheme:light dark;--fg:#111;--bg:#fff;--dim:#666;--line:#ddd;--accent:#b23}
@media(prefers-color-scheme:dark){:root{--fg:#eee;--bg:#151515;--dim:#999;--line:#333}}
*{box-sizing:border-box}
body{margin:0;padding:1rem;font:16px/1.5 system-ui,sans-serif;color:var(--fg);background:var(--bg);max-width:44rem;margin-inline:auto}
h1{font-size:1.3rem;margin:0 0 .2rem}
h2{font-size:1rem;margin:1.6rem 0 .4rem;color:var(--dim);text-transform:uppercase;letter-spacing:.05em}
.sub{color:var(--dim);margin:0 0 1rem}
table{width:100%;border-collapse:collapse}
td,th{padding:.35rem .4rem;border-bottom:1px solid var(--line);text-align:left}
th{color:var(--dim);font-weight:normal}
td.n{text-align:right;font-variant-numeric:tabular-nums}
.grid{display:grid;grid-template-columns:repeat(auto-fit,minmax(9rem,1fr));gap:.6rem}
.card{border:1px solid var(--line);border-radius:.5rem;padding:.6rem .7rem}
.card .k{color:var(--dim);font-size:.8rem}
.card .v{font-size:1.35rem;font-variant-numeric:tabular-nums}
form{display:flex;gap:.4rem;align-items:center;margin:.35rem 0;flex-wrap:wrap}
label{min-width:9rem;color:var(--dim)}
input{flex:1;min-width:5rem;padding:.4rem;border:1px solid var(--line);border-radius:.35rem;background:var(--bg);color:var(--fg)}
button,a.btn{padding:.4rem .8rem;border:1px solid var(--line);border-radius:.35rem;background:var(--bg);color:var(--fg);cursor:pointer;text-decoration:none;display:inline-block}
button:hover,a.btn:hover{border-color:var(--accent);color:var(--accent)}
.warn{border-left:3px solid var(--accent);padding-left:.7rem;margin:1rem 0}
.scroll{overflow-x:auto}
code{background:rgba(128,128,128,.15);padding:.1rem .3rem;border-radius:.2rem}
</style>";

/// One statistic card.
fn card(key: &str, value: &str) -> String {
    format!(
        "<div class=card><div class=k>{}</div><div class=v>{}</div></div>",
        escape(key),
        escape(value)
    )
}

/// A one-button form that queues a console command.
///
/// `GET` rather than `POST` because the whole page is one screen of a captive
/// portal and a GET survives the back button; the redirect in the handler is
/// what stops a reload repeating the action.
fn action(command: &str, label: &str) -> String {
    format!(
        "<form action=/do><input type=hidden name=cmd value='{}'><button>{}</button></form>",
        escape(command),
        escape(label)
    )
}

/// A labelled field with a submit button.
fn field(command: &str, label: &str, current: &str, hint: &str) -> String {
    format!(
        "<form action=/do><label for=f{cmd}>{label}</label>\
         <input type=hidden name=cmd value='{cmd}'>\
         <input id=f{cmd} name=v value='{current}' placeholder='{hint}'>\
         <button>set</button></form>",
        cmd = escape(command),
        label = escape(label),
        current = escape(current),
        hint = escape(hint),
    )
}

pub fn page(state: &Snapshot) -> String {
    let mut out = String::with_capacity(8192);
    out.push_str("<!doctype html><meta charset=utf-8>");
    out.push_str("<meta name=viewport content='width=device-width,initial-scale=1'>");
    out.push_str("<title>lightning</title>");
    out.push_str(STYLE);

    out.push_str("<h1>lightning monitor</h1>");
    out.push_str(&format!(
        "<p class=sub>{} &middot; up {} &middot; {}</p>",
        escape(state.location),
        escape(&duration(state.uptime_s)),
        escape(if state.time_local.is_empty() { "clock not set" } else { &state.time_local }),
    ));

    if state.time_local.is_empty() {
        out.push_str(
            "<div class=warn><b>The clock is not set.</b> Strikes are still recorded, \
             with an empty time column — the record is kept and the timestamp is not \
             invented. Set it below and later strikes will carry the time.</div>",
        );
    }
    if state.frozen {
        out.push_str(
            "<div class=warn><b>The auto-tune is frozen</b> (<code>sensitive on</code>). \
             The noise floor is at zero and will not climb, so expect disturbers. \
             Turn it off below to hand the device back to itself.</div>",
        );
    }

    // --- what it has heard --------------------------------------------------
    out.push_str("<h2>Strikes</h2><div class=grid>");
    out.push_str(&card("last hour", &state.strikes_hour.to_string()));
    out.push_str(&card("last 24 hours", &state.strikes_day.to_string()));
    out.push_str(&card("since boot", &state.strikes_total.to_string()));
    out.push_str(&card(
        "nearest",
        &match state.nearest_km {
            Some(0) => "overhead".to_string(),
            Some(km) => format!("{km} km"),
            None => "--".to_string(),
        },
    ));
    out.push_str("</div>");
    if let Some(last) = &state.last_strike {
        out.push_str(&format!("<p class=sub>most recent: {}</p>", escape(last)));
    }

    // --- the receiver -------------------------------------------------------
    out.push_str("<h2>Receiver</h2><div class=grid>");
    out.push_str(&card(
        "noise floor",
        &format!("{}/7 ({}%)", state.defence_raw, state.defence_percent),
    ));
    out.push_str(&card("noise", &format!("{}/min", state.noise_per_min)));
    out.push_str(&card("disturbers", &format!("{}/min", state.disturbers_per_min)));
    out.push_str(&card("quiet threshold", &format!("{}/min", state.quiet_per_min)));
    out.push_str("</div>");

    // --- the board ----------------------------------------------------------
    out.push_str("<h2>Board</h2><div class=grid>");
    out.push_str(&card(
        "battery",
        &format!("{}% ({} mV)", state.battery_percent, state.battery_mv),
    ));
    out.push_str(&card("free memory", &format!("{} KB", state.free_ram_kb)));
    out.push_str(&card("log", &format!("{} records", state.log_records)));
    out.push_str(&card("log size", &format!("{} KB", state.log_bytes / 1024)));
    out.push_str("</div>");

    // --- the log ------------------------------------------------------------
    out.push_str("<h2>Recent strikes</h2>");
    if state.recent.is_empty() {
        out.push_str("<p class=sub>Nothing yet. The table fills as strikes arrive, and \
                      survives a reboot — it is replayed from the log at boot.</p>");
    } else {
        out.push_str("<div class=scroll><table><tr><th>when<th>distance<th class=n>energy\
                      <th class=n>score<th class=n>strokes</tr>");
        for row in &state.recent {
            out.push_str(&format!(
                "<tr><td>{}<td>{}<td class=n>{}<td class=n>{}<td class=n>{}</tr>",
                escape(&row.when),
                escape(&row.distance),
                row.energy,
                row.score,
                row.strokes,
            ));
        }
        out.push_str("</table></div>");
    }
    out.push_str(
        "<p><a class=btn href=/log.csv download>download the whole log (CSV)</a></p>",
    );

    // --- settings -----------------------------------------------------------
    out.push_str("<h2>Settings</h2>");
    out.push_str(&field("tz", "time zone (minutes)", &state.tz_minutes.to_string(), "-300"));
    out.push_str(&field("quiet", "quiet threshold /min", &state.quiet_per_min.to_string(), "60"));
    out.push_str(&field("defence", "noise floor 0-7", &state.defence_raw.to_string(), "3"));
    out.push_str(&field("srej", "spike rejection 0-15", "", "2"));
    out.push_str(&field("wdth", "watchdog 0-15", "", "2"));
    out.push_str(&field("merge", "merge window ms", "", "1000"));
    out.push_str(&field("events", "log N noise/disturber rows", "", "500"));

    out.push_str("<h2>Actions</h2><div class=grid>");
    for (command, label) in [
        ("indoor", "indoor gain"),
        ("outdoor", "outdoor gain"),
        ("calibrate", "calibrate the noise floor"),
        ("sync", "flush the log to flash"),
        ("clearstats", "clear the distance estimate"),
        ("reboot", "reboot"),
    ] {
        out.push_str(&action(command, label));
    }
    out.push_str(&format!(
        "<form action=/do><input type=hidden name=cmd value=sensitive>\
         <input type=hidden name=v value='{}'><button>{}</button></form>",
        if state.frozen { "off" } else { "on" },
        if state.frozen { "release the auto-tune" } else { "open up and freeze" },
    ));
    out.push_str("</div>");

    // --- the network itself -------------------------------------------------
    if let Some((ssid, password, generated)) = &state.credentials {
        out.push_str("<h2>This network</h2>");
        out.push_str(&format!(
            "<p class=sub>name <code>{}</code> &middot; password <code>{}</code></p>",
            escape(ssid),
            escape(password),
        ));
        if *generated {
            out.push_str(
                "<div class=warn><b>This password was made up for this session and \
                 has not been saved.</b> It will be different next time unless it is \
                 stored — which is deliberate: a password nobody has read should not \
                 quietly become permanent. Use the console's <code>ap</code> command \
                 to set one you choose.</div>",
            );
        }
    }

    out.push_str(&format!(
        "<h2>Console</h2><p class=sub>Everything here is a console command; the page \
         only types them for you. The full table is in <code>help</code> over USB. \
         The access point drops itself after {} seconds with nobody connected.</p>",
        crate::portal::WINDOW_S,
    ));

    out
}
