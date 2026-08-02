//! Host checks for §4.2's noise-rejection ladder.
//!
//! A copy of `src/defence.rs`'s logic — see README.md.
//!
//! Worth testing because the ladder is three ranges welded end to end, and the
//! joins are where an off-by-one lives: a level that skips a rung, repeats one,
//! or writes a register value past its field width would all look plausible on
//! a console and would only misbehave under sustained noise.

/// Where each rung starts, from §3 step 6.
pub const NOISE_FLOOR_BASE: u8 = 0;
pub const WATCHDOG_BASE: u8 = 2;
pub const SPIKE_REJECT_BASE: u8 = 0;

/// Where each rung stops.
///
/// `NF_LEV` and `WDTH` are 3- and 4-bit fields and use their full range.
/// `SREJ` is capped below its 4-bit maximum on purpose: the datasheet's own
/// curves flatten past ~11, and the last few settings reject so aggressively
/// that a genuine nearby strike can be discarded. A detector that hears nothing
/// is worse than one that hears noise, because the noise is at least visible.
pub const NOISE_FLOOR_MAX: u8 = 7;
pub const WATCHDOG_MAX: u8 = 15;
pub const SPIKE_REJECT_MAX: u8 = 11;

/// Total rungs: every step of all three knobs.
pub const MAX_LEVEL: u8 = (NOISE_FLOOR_MAX - NOISE_FLOOR_BASE)
    + (WATCHDOG_MAX - WATCHDOG_BASE)
    + (SPIKE_REJECT_MAX - SPIKE_REJECT_BASE);

/// The three register values a defence level maps to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Settings {
    pub noise_floor: u8,
    pub watchdog: u8,
    pub spike_reject: u8,
}

/// Translate a level into register values.
///
/// Pure, total, and the only place the ladder's shape is written down — so
/// changing the order of the rungs is a one-function edit rather than a hunt
/// through the event loop.
pub fn settings(level: u8) -> Settings {
    let level = level.min(MAX_LEVEL);

    let noise_span = NOISE_FLOOR_MAX - NOISE_FLOOR_BASE;
    let watchdog_span = WATCHDOG_MAX - WATCHDOG_BASE;

    if level <= noise_span {
        return Settings {
            noise_floor: NOISE_FLOOR_BASE + level,
            watchdog: WATCHDOG_BASE,
            spike_reject: SPIKE_REJECT_BASE,
        };
    }

    let past_noise = level - noise_span;
    if past_noise <= watchdog_span {
        return Settings {
            noise_floor: NOISE_FLOOR_MAX,
            watchdog: WATCHDOG_BASE + past_noise,
            spike_reject: SPIKE_REJECT_BASE,
        };
    }

    Settings {
        noise_floor: NOISE_FLOOR_MAX,
        watchdog: WATCHDOG_MAX,
        spike_reject: SPIKE_REJECT_BASE + (past_noise - watchdog_span),
    }
}

/// A short name for which knob a level is currently working on, for the console
/// and later for the screen.
pub fn rung(level: u8) -> &'static str {
    let noise_span = NOISE_FLOOR_MAX - NOISE_FLOOR_BASE;
    let watchdog_span = WATCHDOG_MAX - WATCHDOG_BASE;
    match level {
        l if l <= noise_span => "noise floor",
        l if l - noise_span <= watchdog_span => "watchdog",
        _ => "spike rejection",
    }
}

static mut PASS: u32 = 0;
static mut FAIL: u32 = 0;

fn check(name: &str, ok: bool) {
    unsafe {
        if ok { PASS += 1; println!("  ok   {name}"); }
        else { FAIL += 1; println!("  FAIL {name}"); }
    }
}

fn main() {
    println!("defence:");

    check("level 0 is every knob at its base", settings(0) == Settings {
        noise_floor: NOISE_FLOOR_BASE, watchdog: WATCHDOG_BASE, spike_reject: SPIKE_REJECT_BASE });

    // --- the first rung: noise floor only ---------------------------------
    check("level 7 tops out the noise floor", settings(7).noise_floor == NOISE_FLOOR_MAX);
    check("...without touching the watchdog", settings(7).watchdog == WATCHDOG_BASE);
    check("...or spike rejection", settings(7).spike_reject == SPIKE_REJECT_BASE);

    // --- the joins, which is what this file is for ------------------------
    check("level 8 starts the watchdog, noise floor pinned", settings(8) == Settings {
        noise_floor: NOISE_FLOOR_MAX, watchdog: WATCHDOG_BASE + 1, spike_reject: SPIKE_REJECT_BASE });
    check("level 20 tops out the watchdog", settings(20).watchdog == WATCHDOG_MAX);
    check("...spike rejection still untouched", settings(20).spike_reject == SPIKE_REJECT_BASE);
    check("level 21 starts spike rejection", settings(21) == Settings {
        noise_floor: NOISE_FLOOR_MAX, watchdog: WATCHDOG_MAX, spike_reject: SPIKE_REJECT_BASE + 1 });

    // --- the top ----------------------------------------------------------
    check("MAX_LEVEL is 31", MAX_LEVEL == 31);
    check("MAX_LEVEL maxes every knob", settings(MAX_LEVEL) == Settings {
        noise_floor: NOISE_FLOOR_MAX, watchdog: WATCHDOG_MAX, spike_reject: SPIKE_REJECT_MAX });
    check("past the top saturates rather than wrapping", settings(255) == settings(MAX_LEVEL));

    // --- properties that must hold across the whole ladder ----------------
    let mut monotonic = true;
    let mut in_range = true;
    for level in 0..=MAX_LEVEL {
        let s = settings(level);
        if level > 0 {
            let previous = settings(level - 1);
            // Exactly one knob moves per rung, and only ever upward.
            let moved = (s.noise_floor != previous.noise_floor) as u8
                + (s.watchdog != previous.watchdog) as u8
                + (s.spike_reject != previous.spike_reject) as u8;
            if moved != 1
                || s.noise_floor < previous.noise_floor
                || s.watchdog < previous.watchdog
                || s.spike_reject < previous.spike_reject
            {
                monotonic = false;
            }
        }
        // Field widths: NF_LEV is 3 bits, WDTH and SREJ are 4.
        if s.noise_floor > 7 || s.watchdog > 15 || s.spike_reject > 15 {
            in_range = false;
        }
    }
    check("every rung moves exactly one knob, upward only", monotonic);
    check("no setting ever exceeds its register field", in_range);

    // --- the rung labels --------------------------------------------------
    check("rung names follow the ladder", rung(0) == "noise floor"
        && rung(7) == "noise floor" && rung(8) == "watchdog"
        && rung(20) == "watchdog" && rung(21) == "spike rejection"
        && rung(MAX_LEVEL) == "spike rejection");

    unsafe {
        println!("\n{PASS} passed, {FAIL} failed");
        std::process::exit(if FAIL == 0 { 0 } else { 1 });
    }
}
