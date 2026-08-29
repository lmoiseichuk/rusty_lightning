//! Host checks for the web UI's query parsing and escaping.
//!
//! **This compiles the real `src/query.rs`**, included by path.
//!
//! This is the only code in the firmware that takes input from outside the
//! device. Everything else is either the operator at a console or the sensor on
//! a bus. The access point is open for sixty seconds at a time on a password
//! somebody read off the panel, so the threat is small — but "small" is a
//! statement about who is on the network, not about whether the parser is
//! correct, and the parser is where a mistake would let a query reach the
//! console with something the console did not expect.
//!
//! ```sh
//! cd tests/host && rustc --edition 2024 -A dead_code -o /tmp/query query.rs && /tmp/query
//! ```

use std::sync::atomic::{AtomicU32, Ordering};

#[path = "../../src/query.rs"]
mod query;
use query::*;

static PASS: AtomicU32 = AtomicU32::new(0);
static FAIL: AtomicU32 = AtomicU32::new(0);

fn check(name: &str, ok: bool) {
    let counter = if ok { &PASS } else { &FAIL };
    counter.fetch_add(1, Ordering::Relaxed);
    println!("  {:<4} {name}", if ok { "ok" } else { "FAIL" });
}

fn main() {
    println!("== query ==");

    println!("\n  the commands the page can send");
    check("a bare command passes", command_from_query("cmd=indoor").as_deref() == Some("indoor"));
    check(
        "a command with an argument is composed",
        command_from_query("cmd=defence&v=5").as_deref() == Some("defence 5"),
    );
    check(
        "the argument order does not matter",
        command_from_query("v=5&cmd=defence").as_deref() == Some("defence 5"),
    );
    check(
        "sensitive takes on or off",
        command_from_query("cmd=sensitive&v=on").as_deref() == Some("sensitive on"),
    );

    println!("\n  and what it refuses");
    // The allow-list is the whole security argument: a command the page does
    // not know must not reach the console's parser at all.
    check("an unknown command is refused", command_from_query("cmd=erase").is_none());
    check("an empty query is refused", command_from_query("").is_none());
    check(
        "a command needing an argument is refused without one",
        command_from_query("cmd=defence").is_none(),
    );
    check(
        "an argument to a command that takes none is ignored, not appended",
        command_from_query("cmd=indoor&v=nonsense").as_deref() == Some("indoor"),
    );
    check(
        "sensitive refuses anything but on and off",
        command_from_query("cmd=sensitive&v=maybe").is_none(),
    );

    // **The one that matters.** A space is the console's argument separator, so
    // an argument containing one would arrive as two arguments -- which is how
    // a single field turns into a different command with a different meaning.
    check(
        "an argument containing a space is refused",
        command_from_query("cmd=defence&v=5+reboot").is_none(),
    );
    check(
        "...including an encoded one",
        command_from_query("cmd=defence&v=5%20reboot").is_none(),
    );
    check(
        "...and a tab",
        command_from_query("cmd=defence&v=5%09reboot").is_none(),
    );
    check(
        "...and a newline",
        command_from_query("cmd=defence&v=5%0Areboot").is_none(),
    );

    println!("\n  percent decoding");
    check("plain text is unchanged", percent_decode("hello") == "hello");
    check("a plus is a space", percent_decode("a+b") == "a b");
    check("an escape decodes", percent_decode("a%2Db") == "a-b");
    check("a lowercase escape decodes", percent_decode("a%2db") == "a-b");
    // Forgiving rather than strict: a stray `%` in a password is a character,
    // not an error, and turning it into something else silently is worse than
    // leaving it.
    check("a truncated escape is left alone", percent_decode("100%") == "100%");
    check("a non-hex escape is left alone", percent_decode("%zz") == "%zz");

    println!("\n  escaping what goes back out");
    check("plain text is unchanged", escape("hello") == "hello");
    check("a tag is escaped", escape("<b>") == "&lt;b&gt;");
    check("an ampersand is escaped", escape("a&b") == "a&amp;b");
    check("quotes are escaped", escape("\"x\"") == "&quot;x&quot;");
    check("apostrophes are escaped", escape("it's") == "it&#39;s");
    // The attribute case: a password is rendered inside single quotes on the
    // settings form, so an apostrophe in one must not close the attribute.
    check(
        "an apostrophe cannot break out of an attribute",
        !escape("a' onload='x").contains('\''),
    );

    println!("\n  durations");
    check("minutes only", duration(90) == "1m");
    check("hours and minutes", duration(3660) == "1h 01m");
    check("days too", duration(90_000) == "1d 1h 00m");
    check("zero", duration(0) == "0m");

    let passed = PASS.load(Ordering::Relaxed);
    let failed = FAIL.load(Ordering::Relaxed);
    println!("\n{passed} passed, {failed} failed");
    if failed > 0 {
        std::process::exit(1);
    }
}
