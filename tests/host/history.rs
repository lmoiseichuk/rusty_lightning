//! Host checks for the strike history rings.
//!
//! A copy of `src/history.rs`'s logic — see README.md.
//!
//! Worth testing because a ring's failures are all quiet ones. A gap that is
//! not cleared shows last week's storm as today's; an off-by-one in `series`
//! draws the chart reversed; a wrong score formula produces plausible numbers
//! that rank storms wrongly. None of it errors, and all of it needs days of
//! weather to notice on hardware.
//! **This compiles the real `src/history.rs` and `src/strike.rs`**, included by
//! path — not copies of them. `history` names its dependency as
//! `crate::strike`, so the module has to be declared with that name at this
//! binary's root for the path to resolve. (The `strike()` helper below is a
//! function, and functions and modules live in different namespaces, so the two
//! names coexist.)

#[path = "../../src/strike.rs"]
mod strike;
#[path = "../../src/history.rs"]
mod history;
use history::*;
use strike::*;

static mut PASS: u32 = 0;
static mut FAIL: u32 = 0;
fn check(name: &str, ok: bool) {
    unsafe {
        if ok { PASS += 1; println!("  ok   {name}"); }
        else { FAIL += 1; println!("  FAIL {name}"); }
    }
}

/// A strike whose intensity_milli is `intensity`, at `distance`.
fn strike(distance: Distance, intensity: u32) -> Strike {
    Strike { distance, energy_raw: intensity * 16777 / 1000 }
}

fn main() {
    println!("history:");

    // --- the score formula, against §4.3's observed figures ---------------
    // "distance ~6 km, intensity ~3-7, score ~0.5-1.2"
    let s = strike(Distance::Km(6), 3000);
    check("6 km at intensity 3 scores ~0.5", (490..=510).contains(&score_milli(&s).unwrap()));
    let s = strike(Distance::Km(6), 7000);
    check("6 km at intensity 7 scores ~1.16", (1140..=1180).contains(&score_milli(&s).unwrap()));

    check("overhead scores 10x a 1 km strike",
        score_milli(&strike(Distance::Overhead, 3000)).unwrap()
            == 10 * score_milli(&strike(Distance::Km(1), 3000)).unwrap());
    check("out of range has no score", score_milli(&strike(Distance::OutOfRange, 3000)).is_none());

    // --- bucketing ---------------------------------------------------------
    let mut ring: Ring<96> = Ring::new(15);
    ring.record(0, &strike(Distance::Km(6), 3000));
    ring.record(5, &strike(Distance::Km(6), 3000));
    ring.record(14, &strike(Distance::Km(4), 3000));
    check("three strikes inside one 15-min bucket land together", ring.recent(1).strikes == 3);
    ring.record(15, &strike(Distance::Km(6), 3000));
    check("minute 15 opens a new bucket", ring.recent(1).strikes == 1);
    check("...and the previous one is still there", ring.recent(2).strikes == 4);

    // --- the gap clear, which is the bug this file exists for ---------------
    let mut ring: Ring<96> = Ring::new(15);
    ring.record(0, &strike(Distance::Km(6), 3000));
    // One full lap of the ring (96 x 15 min = 24 h) plus a bit.
    ring.tick(15 * 96 + 30);
    check("a full lap clears every stale bucket", ring.recent(96).strikes == 0);

    let mut ring: Ring<96> = Ring::new(15);
    ring.record(0, &strike(Distance::Km(6), 3000));
    ring.tick(60);   // four buckets later, no strikes between
    check("a short gap clears only the skipped buckets", ring.recent(4).strikes == 0);
    check("...and leaves the older one intact", ring.recent(5).strikes == 1);

    // --- distance samples are not strike counts ----------------------------
    let mut ring: Ring<96> = Ring::new(15);
    ring.record(0, &strike(Distance::Km(10), 3000));
    ring.record(1, &strike(Distance::Overhead, 3000));
    ring.record(2, &strike(Distance::OutOfRange, 3000));
    let b = ring.recent(1);
    check("all three count as strikes", b.strikes == 3);
    check("only the one with a distance is averaged", b.distance_samples == 1);
    check("mean distance is that one, not a third of it", b.mean_distance_km() == Some(10));
    check("closest is tracked", b.distance_km_min == 10);

    // --- the sentinel, which derive(Default) got wrong ---------------------
    check("a default bucket has NO closest, not 0 km",
        Bucket::default().distance_km_min == u8::MAX);

    // The failure this guards: a window whose other buckets are empty must not
    // report the closest strike as 0 km. Seen on hardware -- three strikes at
    // 20, 9 and 3 km reported "closest 0 km" because every cleared bucket
    // carried a derived default of zero and won every min().
    let mut ring: Ring<96> = Ring::new(15);
    ring.record(0, &strike(Distance::Km(20), 3000));
    ring.record(1, &strike(Distance::Km(9), 3000));
    ring.record(2, &strike(Distance::Km(3), 3000));
    check("closest over a mostly-empty window is the real closest",
        ring.recent(4).distance_km_min == 3);
    check("...and an entirely empty window has no closest",
        Ring::<96>::new(15).recent(4).distance_km_min == u8::MAX);

    // --- series ordering ---------------------------------------------------
    let mut ring: Ring<4> = Ring::new(15);
    ring.record(0, &strike(Distance::Km(6), 3000));
    ring.record(45, &strike(Distance::Km(6), 3000));
    ring.record(45, &strike(Distance::Km(6), 3000));
    let mut out = [Bucket::default(); 4];
    ring.series(&mut out);
    check("series is oldest-first, newest last", out[3].strikes == 2 && out[0].strikes == 1);

    // --- live length: charts must fill from the left ----------------------
    let mut ring: Ring<96> = Ring::new(15);
    check("an untouched ring has no live buckets", ring.live_len() == 0);
    ring.record(0, &strike(Distance::Km(6), 3000));
    check("one bucket after the first strike", ring.live_len() == 1);
    ring.tick(45);
    check("four buckets after 45 minutes", ring.live_len() == 4);
    ring.tick(15 * 200);
    check("never exceeds the ring length", ring.live_len() == 96);

    let mut ring: Ring<4> = Ring::new(15);
    ring.record(0, &strike(Distance::Km(6), 3000));
    ring.record(30, &strike(Distance::Km(2), 9000));
    let (mut c, mut s2) = ([0u16; 4], [0u32; 4]);
    let live = series_of(&ring, &mut c, &mut s2);
    check("series_of reports the live count", live == 3);
    check("...oldest first, anchored at index 0", c[0] == 1 && c[2] == 1 && c[1] == 0);

    // --- mean score --------------------------------------------------------
    let mut ring: Ring<96> = Ring::new(15);
    ring.record(0, &strike(Distance::Km(6), 3000));   // 500
    ring.record(1, &strike(Distance::Km(6), 7000));   // ~1160
    let mean = ring.recent(1).mean_score_milli().unwrap();
    check("mean score averages the two", (820..=840).contains(&mean));
    check("an empty bucket has no mean", Bucket::default().mean_score_milli().is_none());

    unsafe {
        println!("\n{PASS} passed, {FAIL} failed");
        std::process::exit(if FAIL == 0 { 0 } else { 1 });
    }
}
