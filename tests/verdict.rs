//! Host checks for the tuning window's verdict.
//!
//! **This compiles the real `src/verdict.rs`**, included by path — not a copy.
//! Keeping that module free of ESP-IDF imports is what makes this possible, and
//! it is why the module exists at all: `tuning` drives an I²C sensor and cannot
//! be built on a workstation, so the one decision worth checking was unreachable
//! until it was moved out.
//!
//! The claim being defended is a negative one, which is exactly the kind that
//! rots quietly: **a disturber must never reach the quiet verdict.** Nothing in
//! the type system says so. Folding `noise + disturbers` compiles, runs, and
//! looks reasonable — it was what shipped, and it made the tuner climb `NF_LEV`
//! against events that knob cannot touch.
//!
//! ```sh
//! cd tests && rustc --edition 2024 -A dead_code -o /tmp/verdict verdict.rs && /tmp/verdict
//! ```

use std::sync::atomic::{AtomicU32, Ordering};

// `verdict` splits a lightning interrupt by its distance with
// `golden::is_evidence`, and times the rescue's silence with `uptime`; `golden`
// in turn names `merger` for the flash it folds into its record. All of them
// at the root, where their `crate::` paths resolve.
#[path = "../src/strike.rs"]
mod strike;
#[path = "../src/uptime.rs"]
mod uptime;
#[path = "../src/merger.rs"]
mod merger;
#[path = "../src/golden.rs"]
mod golden;
#[path = "../src/verdict.rs"]
mod verdict;
use strike::{Distance, Strike};
use verdict::*;

/// A detail read that came back, as `As3935::strike` returns one.
fn read(distance: Distance) -> Result<Strike, ()> {
    Ok(Strike { distance, energy_raw: 100_000 })
}

/// A detail read that failed.
fn unreadable() -> Result<Strike, ()> {
    Err(())
}

static PASS: AtomicU32 = AtomicU32::new(0);
static FAIL: AtomicU32 = AtomicU32::new(0);

fn check(name: &str, ok: bool) {
    let counter = if ok { &PASS } else { &FAIL };
    counter.fetch_add(1, Ordering::Relaxed);
    println!("  {:<4} {name}", if ok { "ok" } else { "FAIL" });
}

fn main() {
    println!("== verdict ==");

    // **The regression this file exists for.** Storm of 2026-08-26: a window of
    // pure disturbers read as a noisy band, and the tuner answered with the one
    // knob that cannot remove them.
    let mut storm = Window::default();
    storm.fold(0, 8, 0);
    check(
        "a window of disturbers alone is quiet",
        storm.quiet(1, 60),
    );
    check(
        "and its disturbers are still counted",
        storm.disturbers_per_min(60) == 8,
    );
    check(
        "and contribute nothing to the noise rate",
        storm.noise_per_min(60) == 0,
    );

    let mut noisy = Window::default();
    noisy.fold(11, 0, 0);
    check("noise alone still says noisy", !noisy.quiet(1, 60));

    // A rate, not a count: the same evidence in half the window is twice the
    // rate, and the verdict has to follow.
    let mut half = Window::default();
    half.fold(2, 0, 0);
    check("2 in 30 s is 4/min", half.noise_per_min(30) == 4);
    check("2 in 60 s is 2/min", half.noise_per_min(60) == 2);
    check("and 4/min fails a 3/min bar", !half.quiet(3, 30));
    check("where 2/min passes it", half.quiet(3, 60));

    // Multiply before dividing, or a window that is not a whole divisor of 60
    // collapses.
    check("7 in 45 s is 9/min, not 0", Window::per_min(7, 45) == 9);
    check("a zero window does not divide by zero", Window::per_min(5, 0) == 300);

    // Strikes veto a climb elsewhere; the window only has to carry them.
    let mut weather = Window::default();
    weather.fold(0, 6, 2);
    check("strikes are kept separately", weather.strikes == 2);
    check("and do not make the band noisy", weather.quiet(1, 60));

    let mut used = Window::default();
    used.fold(3, 4, 5);
    used.unconfirmed = 6;
    used.clear();
    check("clear empties every counter", used == Window::default());

    println!("\n  a lightning interrupt is not always a strike the tuner may act on");
    let mut batch = Batch::default();
    batch.lightning(&read(Distance::Overhead));
    check("overhead is still a strike to the console", batch.strikes == 1);
    check("...but not a confirmed one", batch.confirmed() == 0 && batch.unconfirmed == 1);
    batch.lightning(&read(Distance::Km(9)));
    check("a kilometre reading is confirmed", batch.confirmed() == 1);
    batch.lightning(&read(Distance::OutOfRange));
    check("so is out of range", batch.confirmed() == 2);
    // The detail read failed: nothing says what the chip measured, so it may
    // not vouch for anything.
    batch.lightning(&unreadable());
    check("an unreadable one is unconfirmed", batch.unconfirmed == 2 && batch.confirmed() == 2);
    check("and every one of them is still counted", batch.strikes == 4);

    // **The dip and the rescue read `Window::strikes`**, so this is the line
    // between an overhead and a decision that commits the device to anything.
    let mut overheads = Batch::default();
    for _ in 0..3 {
        overheads.lightning(&read(Distance::Overhead));
    }
    overheads.noise = 70;
    let mut window = Window::default();
    window.fold_batch(&overheads);
    check("a window of overhead alone heard no lightning that is evidence", !window.heard_lightning());
    check("...carries no strike a dip or the rescue could read", window.strikes == 0);
    check("...still says how many it had", window.unconfirmed == 3);
    check("...and its band is noisy", !window.quiet(60, 60));
    // **But it holds.** A hold only refuses to step, and the band a near
    // strike leaves behind is not the room's.
    check("...and still holds the walk rather than climbing on it", window.walk(60, 60) == Walk::Hold);
    let mut located = Batch::default();
    located.lightning(&read(Distance::Km(12)));
    window.fold_batch(&located);
    check("one strike with a distance and it has heard lightning", window.heard_lightning());
    check("and holds exactly that one", window.strikes == 1 && window.unconfirmed == 3);

    println!("\n  the walk: any lightning holds, then the noise decides");
    let walk_of = |batch: Batch| {
        let mut window = Window::default();
        window.fold_batch(&batch);
        window.walk(60, 60)
    };
    check("a quiet window with no lightning relaxes", walk_of(Batch { noise: 10, ..Batch::default() }) == Walk::Relax);
    check("a noisy one climbs", walk_of(Batch { noise: 200, ..Batch::default() }) == Walk::Climb);
    check("...and disturbers alone do not make it climb", walk_of(Batch { disturbers: 500, ..Batch::default() }) == Walk::Relax);
    let mut located = Batch { noise: 200, ..Batch::default() };
    located.lightning(&read(Distance::Km(8)));
    check("a noisy window with a located strike holds", walk_of(located) == Walk::Hold);
    let mut quiet_strike = Batch::default();
    quiet_strike.lightning(&read(Distance::Km(8)));
    check("...and so does a quiet one, rather than relaxing mid-storm", walk_of(quiet_strike) == Walk::Hold);
    let mut failed_read = Batch { noise: 200, ..Batch::default() };
    failed_read.lightning(&unreadable());
    check("a strike whose distance could not be read holds too", walk_of(failed_read) == Walk::Hold);
    // **The night this rule is for**, 2026-08-12 as a repeat would look now:
    // the band at 208-299 noise/min, the strikes all in the nearest bin. A
    // window with eight of them at 210/min must hold -- 0.22.0 climbed
    // 210/60 = 3 notches here, and reached nf 7 in the second window.
    let mut overhead_storm = Window::default();
    for second in 0..60 {
        let mut each = Batch { noise: if second < 30 { 4 } else { 3 }, ..Batch::default() };
        if second % 8 == 0 {
            each.lightning(&read(Distance::Overhead));
        }
        overhead_storm.fold_batch(&each);
    }
    check("the storm overhead runs at 210 noise/min", overhead_storm.noise_per_min(60) == 210);
    check("...hears no lightning that is evidence", !overhead_storm.heard_lightning());
    check("...and holds the walk all the same", overhead_storm.walk(60, 60) == Walk::Hold);

    // September's stream, one overhead every 8 s for a minute, sixty minutes.
    // It holds every window -- which costs a walk that did not step -- and
    // reads as lightning in none, so nothing stops the dip looking every ten
    // minutes and nothing keeps one open.
    let mut september_heard = false;
    let mut september_held = true;
    for _ in 0..60 {
        let mut minute = Window::default();
        for second in 0..60 {
            let mut each = Batch::default();
            if second % 8 == 0 {
                each.lightning(&read(Distance::Overhead));
            }
            minute.fold_batch(&each);
        }
        september_heard |= minute.heard_lightning();
        september_held &= minute.walk(60, 60) == Walk::Hold;
    }
    check("an hour of September's overhead never once reads as lightning", !september_heard);
    check("...and holds the walk in every window of it", september_held);

    println!("\n  the dip and the rescue read only strikes with a distance");
    let mut overhead_only = Batch::default();
    overhead_only.lightning(&read(Distance::Overhead));
    let mut found = Batch::default();
    found.lightning(&read(Distance::Km(14)));
    let mut failed = Batch::default();
    failed.lightning(&unreadable());
    let mut dip = Window::default();
    dip.fold_batch(&overhead_only);
    check("a dip that heard only overhead does not stay open", !dip.dip_stays_open());
    check("...and an overhead does not stop the next dip starting", dip.dip_may_start());
    dip.fold_batch(&found);
    check("one strike with a distance keeps it open", dip.dip_stays_open());
    check("...and makes the next dip unnecessary", !dip.dip_may_start());
    let mut lost = Window::default();
    lost.fold_batch(&failed);
    check("an unreadable strike keeps no dip open", !lost.dip_stays_open() && lost.dip_may_start());
    check("an empty window lets a dip start", Window::default().dip_may_start());

    // The rescue's clock. **September's stream must not keep restarting it**:
    // that is what kept 0.21's rescue from ever running while it lasted.
    let minute = 60_000u32;
    let mut silence = Silence::default();
    check("no strike yet, no silence to measure", silence.minutes(100 * minute).is_none());
    silence.observe(&overhead_only, minute);
    check("an overhead does not start the clock", silence.minutes(100 * minute).is_none());
    silence.observe(&failed, minute);
    check("nor does an unreadable strike", silence.minutes(100 * minute).is_none());
    silence.observe(&found, minute);
    check("a located strike does", silence.minutes(26 * minute) == Some(25));
    for second in 0..3600u32 {
        if second % 8 == 0 {
            silence.observe(&overhead_only, minute + second * 1000);
        }
        silence.observe(&Batch { noise: 11, disturbers: 8, ..Batch::default() }, minute + second * 1000);
    }
    check(
        "an hour of overhead, noise and disturbers does not restart it",
        silence.minutes(61 * minute) == Some(60),
    );
    silence.observe(&found, 61 * minute);
    check("the next located strike does", silence.minutes(61 * minute) == Some(0));
    let mut wrapped = Silence::default();
    wrapped.observe(&found, u32::MAX - minute + 1);
    check("...and it counts across the millisecond wrap", wrapped.minutes(minute) == Some(2));

    println!("\n  the raw count is the verdict, the batches are for comparing with 0.20");
    // A band drowning at the chip's re-raise rate, ~90 ms: 11 a batch for a
    // minute. The level interrupt counts all of them, and so did 0.20 awake.
    let mut drowning = Window::default();
    for _ in 0..60 {
        drowning.fold_batch(&Batch { noise: 11, ..Batch::default() });
    }
    check("eleven a second is 660/min", drowning.noise_per_min(60) == 660);
    check("...noisy against 60/min", !drowning.quiet(60, 60));
    check("...in every one of its 60 batches", drowning.noisy_batches == 60 && drowning.batches == 60);
    // **What an unattended 0.20 board saw of the same band**: one read a
    // batch, so 60/min -- exactly the quiet threshold, which `quiet` passes.
    // The verdict must stay on the raw count, or it inherits that blindness.
    check(
        "one a batch for a minute is 60/min, which the threshold calls quiet",
        Window::per_min(drowning.noisy_batches, 60) == 60 && Window::per_min(60, 60) <= 60,
    );
    let mut one_each = Window::default();
    let mut two_each = Window::default();
    for _ in 0..60 {
        one_each.fold_batch(&Batch { noise: 1, ..Batch::default() });
        two_each.fold_batch(&Batch { noise: 2, ..Batch::default() });
    }
    check("one noise in every batch is quiet at 60/min", one_each.quiet(60, 60));
    check(
        "two in every batch is not, though the same batches had noise",
        !two_each.quiet(60, 60) && two_each.noisy_batches == one_each.noisy_batches,
    );
    // The other way round: a burst in one second is a noisy minute by rate,
    // and one noisy batch out of sixty.
    let mut burst = Window::default();
    burst.fold_batch(&Batch { noise: 70, ..Batch::default() });
    for _ in 1..60 {
        burst.fold_batch(&Batch::default());
    }
    check("70 in one batch is noisy at 60/min", !burst.quiet(60, 60));
    check("...from one noisy batch of sixty", burst.noisy_batches == 1 && burst.batches == 60);
    let mut emptied = burst;
    emptied.clear();
    check("clear empties the batch counts too", emptied.batches == 0 && emptied.noisy_batches == 0);

    let (pass, fail) = (PASS.load(Ordering::Relaxed), FAIL.load(Ordering::Relaxed));
    println!("\n{pass} passed, {fail} failed");
    if fail > 0 {
        std::process::exit(1);
    }
}
