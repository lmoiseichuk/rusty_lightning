//! Host checks for reading a log row back.
//!
//! **This compiles the real `src/csv.rs`**, included by path. It exists because
//! the bug it guards against was written and nearly shipped in the same hour:
//! three columns were added in front of `iso_local`, and the positional reader
//! went on taking field 2 as the distance. Field 2 had become the string
//! `lightning`, which parses as neither a kilometre count nor a sentinel — so
//! every new record would have been silently skipped on replay and the charts
//! would have quietly stopped filling. Nothing would have errored.
//!
//! ```sh
//! cd tests && rustc --edition 2024 -A dead_code -o /tmp/csv csv.rs && /tmp/csv
//! ```

use std::sync::atomic::{AtomicU32, Ordering};

#[path = "../src/csv.rs"]
mod csv;
use csv::*;

static PASS: AtomicU32 = AtomicU32::new(0);
static FAIL: AtomicU32 = AtomicU32::new(0);

fn check(name: &str, ok: bool) {
    let counter = if ok { &PASS } else { &FAIL };
    counter.fetch_add(1, Ordering::Relaxed);
    println!("  {:<4} {name}", if ok { "ok" } else { "FAIL" });
}

const OLD: &str = "timestamp,iso_local,distance_km,energy_raw,intensity_milli,score_milli,simulated,strokes";
const NEW: &str = "timestamp,millis,kind,nf,srej,iso_local,distance_km,energy_raw,intensity_milli,score_milli,simulated,strokes";

fn main() {
    println!("== csv ==");

    let old = Columns::from_header(OLD).expect("old header");
    let new = Columns::from_header(NEW).expect("new header");

    check("an old header has no kind column", old.kind.is_none());
    check("a new header has one", new.kind == Some(2));
    check("and it records the provenance column", new.srej == Some(4));
    check("an old header has no srej", old.srej.is_none());
    check("the distance column moved", old.distance == 2 && new.distance == 6);

    // **The regression.** Both files must replay, and the row is the same strike.
    let old_row = "1786585638,2026-08-12 21:07:18,overhead,283426,16893,168930,0,1";
    let new_row = "1786585638,12345,lightning,3,1,2026-08-12 21:07:18,overhead,283426,16893,168930,0,1";
    let want = Row::Strike { epoch: 1786585638, energy_raw: 283426, distance: Distance::Overhead, strokes: 1 };
    check("an old row still parses", parse_row(old_row, &old) == want);
    check("a new row parses to the same strike", parse_row(new_row, &new) == want);

    // **The second thing positional parsing would have got wrong.** A disturber
    // row has empty distance and energy; it must never reach the charts.
    let disturber = "1786585640,12999,disturber,3,1,2026-08-12 21:07:20,,,,,0,0";
    check("a disturber is not a strike", parse_row(disturber, &new) == Row::Event);
    let noise = "1786585641,13100,noise,0,1,2026-08-12 21:07:21,,,,,0,0";
    check("nor is noise", parse_row(noise, &new) == Row::Event);

    // Distances, including both sentinels.
    let km = "1786585700,1,lightning,3,1,x,7,100,1,1,0,1";
    // **The stroke count is read, not assumed.** The replay hardcoded 1 with a
    // comment saying the count was not recoverable, while `strokes` had been
    // the eleventh column for as long as merged flashes had existed -- so every
    // replayed multi-stroke flash came back as a single strike.
    check("a new header finds the strokes column", new.strokes == Some(11));
    // `strokes` is older than `kind`: it is the last of the eight columns too,
    // just at a different index. Both layouts carry it, which is why hardcoding
    // 1 discarded a real number rather than an unavailable one.
    check("the old header has it too, at its own index", old.strokes == Some(7));
    let merged = "1786585638,12345,lightning,3,1,2026-08-12 21:07:18,overhead,283426,16893,168930,0,4";
    check(
        "a merged flash reports its four strokes",
        matches!(parse_row(merged, &new), Row::Strike { strokes: 4, .. }),
    );
    let old_merged = "1786585638,2026-08-12 21:07:18,overhead,283426,16893,168930,0,4";
    check(
        "an eight-column file reports its strokes as well",
        matches!(parse_row(old_merged, &old), Row::Strike { strokes: 4, .. }),
    );
    // A header predating the column at all -- the reader must not invent zero.
    let ancient = Columns::from_header(
        "timestamp,iso_local,distance_km,energy_raw,intensity_milli,score_milli,simulated",
    )
    .expect("a header with no strokes column is still readable");
    check("a header with no strokes column is accepted", ancient.strokes.is_none());
    check(
        "and its rows read as single strikes",
        matches!(
            parse_row("1786585638,2026-08-12 21:07:18,overhead,283426,16893,168930,0", &ancient),
            Row::Strike { strokes: 1, .. }
        ),
    );
    // A blank or unparseable count must not become zero: every strike is at
    // least one stroke, and a zero would divide badly downstream.
    let blank = "1786585638,12345,lightning,3,1,2026-08-12 21:07:18,overhead,283426,16893,168930,0,";
    check(
        "a blank count still reads one",
        matches!(parse_row(blank, &new), Row::Strike { strokes: 1, .. }),
    );

    check("a kilometre reading parses", matches!(parse_row(km, &new), Row::Strike { distance: Distance::Km(7), .. }));
    let far = "1786585700,1,lightning,3,1,x,far,100,1,1,0,1";
    check("`far` is out of range", matches!(parse_row(far, &new), Row::Strike { distance: Distance::OutOfRange, .. }));

    // A strike logged with no clock cannot be placed on a time axis.
    let unstamped = "0,1,lightning,3,1,,overhead,100,1,1,0,1";
    check("epoch 0 is skipped", parse_row(unstamped, &new) == Row::Skip);

    // A power cut mid-write leaves a short line.
    check("a truncated row is skipped", parse_row("1786585638,12345,light", &new) == Row::Skip);

    // A header this build cannot use is refused rather than guessed at.
    check("a header with no distance is refused", Columns::from_header("timestamp,kind").is_none());

    let (pass, fail) = (PASS.load(Ordering::Relaxed), FAIL.load(Ordering::Relaxed));
    println!("\n{pass} passed, {fail} failed");
    if fail > 0 {
        std::process::exit(1);
    }
}
