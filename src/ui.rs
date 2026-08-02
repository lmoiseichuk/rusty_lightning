//! Screen layout (§6).
//!
//! Draws into a plain framebuffer and never touches SPI — `display` owns the
//! hardware. That split is what lets a layout be reasoned about, and eventually
//! tested, without a panel anywhere near it.
//!
//! ## What the panel size changes
//!
//! 800×480 is nineteen times the area of the moisture project's 200×200, and
//! the temptation is to fill it. The constraint that stops that is unchanged:
//! **every refresh is full**, and a full refresh on a UC8179 takes seconds. So
//! the layout is built to be read at a glance from across a room, not to be
//! dense — the extra area buys *size*, not *more*.

use embedded_graphics::mono_font::ascii::{FONT_10X20, FONT_6X10, FONT_9X15};
use embedded_graphics::mono_font::MonoTextStyle;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::{Line, PrimitiveStyle, Rectangle};
use embedded_graphics::text::{Alignment, Baseline, Text};
use epd_waveshare::epd7in5_v2::Display7in5;
use u8g2_fonts::types::{FontColor, VerticalPosition};
use u8g2_fonts::{fonts, FontRenderer};

use crate::as3935::{Distance, Location};
use crate::display::{HEIGHT, INK, PAPER, WIDTH};
use crate::system::Health;

/// The number meant to be read from across the room.
const HEADLINE: FontRenderer = FontRenderer::new::<fonts::u8g2_font_fub42_tr>();
/// Section headings and gauge labels.
const LABEL: FontRenderer = FontRenderer::new::<fonts::u8g2_font_fub14_tr>();

type Text16 = heapless::String<16>;
type Text32 = heapless::String<32>;
type Text64 = heapless::String<64>;

/// Everything the status screen shows.
pub struct Status<'a> {
    pub location: Location,
    /// Measured antenna resonance, kHz — from the LCO self-test.
    pub antenna_khz: u32,
    pub irq_confirmed: bool,
    /// §4.2's defence level and its ceiling.
    pub defence_level: u8,
    pub defence_max: u8,
    pub defence_rung: &'a str,
    pub strikes_total: u32,
    /// The most recent strike, if there has been one.
    pub last_strike: Option<(Distance, u32)>,
    /// Disturbers counted since boot — the honest measure of how hostile the
    /// location is, and the number that decides whether a quiet screen means
    /// "no storms" or "this spot is unusable".
    pub disturbers_total: u32,
    /// Device health, drawn as the status line.
    pub health: Health,
    /// `None` when the gauge is absent or did not answer.
    pub battery: Option<crate::battery::Reading>,
    /// Minutes since boot, for the uptime readout.
    pub uptime_minutes: u32,
}

/// Draw the status screen.
pub fn status(frame: &mut Display7in5, s: &Status<'_>) {
    let _ = frame.clear(PAPER);
    let black = PrimitiveStyle::with_stroke(INK, 1);

    // --- status line ------------------------------------------------------
    //
    // The device talking about itself: clock, die temperature, memory, battery.
    // One line, at the top, small. In a box that runs unattended for weeks this
    // is the only way to ask whether the machine under the readings is healthy —
    // and none of it is about the weather, which is why it is kept out of the
    // way of everything that is.
    status_line(frame, s);

    let _ = Line::new(Point::new(16, 30), Point::new(WIDTH as i32 - 16, 30))
        .into_styled(black)
        .draw(frame);

    // --- header -----------------------------------------------------------
    let _ = LABEL.render(
        "LIGHTNING TERMINAL",
        Point::new(16, 62),
        VerticalPosition::Baseline,
        FontColor::Transparent(INK),
        frame,
    );
    let mut version = Text32::new();
    let _ = version.push_str("fw ");
    let _ = version.push_str(env!("CARGO_PKG_VERSION"));
    let _ = version.push_str(" · ");
    let _ = version.push_str(s.location.label());
    let _ = Text::with_alignment(
        &version,
        Point::new(WIDTH as i32 - 16, 50),
        MonoTextStyle::new(&FONT_9X15, INK),
        Alignment::Right,
    )
    .draw(frame);

    let _ = Line::new(Point::new(16, 74), Point::new(WIDTH as i32 - 16, 74))
        .into_styled(black)
        .draw(frame);

    // --- the headline: total strikes --------------------------------------
    //
    // The one number the device exists to report. Everything else on this
    // screen is context for it.
    let mut count = Text16::new();
    let _ = write_u32(&mut count, s.strikes_total);
    let _ = HEADLINE.render(
        count.as_str(),
        Point::new(16, 158),
        VerticalPosition::Baseline,
        FontColor::Transparent(INK),
        frame,
    );
    let _ = LABEL.render(
        if s.strikes_total == 1 { "strike" } else { "strikes" },
        Point::new(16, 190),
        VerticalPosition::Baseline,
        FontColor::Transparent(INK),
        frame,
    );

    // --- last strike ------------------------------------------------------
    let mut last = Text32::new();
    match s.last_strike {
        Some((distance, intensity_milli)) => {
            let _ = last.push_str(match distance {
                Distance::Overhead => "overhead",
                Distance::OutOfRange => "out of range",
                Distance::Km(_) => "",
            });
            if let Distance::Km(km) = distance {
                let _ = write_u32(&mut last, km as u32);
                let _ = last.push_str(" km");
            }
            let _ = last.push_str("  ·  intensity ");
            let _ = write_u32(&mut last, intensity_milli / 1000);
        }
        None => {
            let _ = last.push_str("no strikes yet");
        }
    }
    let _ = Text::with_baseline(
        &last,
        Point::new(320, 138),
        MonoTextStyle::new(&FONT_10X20, INK),
        Baseline::Top,
    )
    .draw(frame);

    // --- gauges -----------------------------------------------------------
    gauge(
        frame,
        Point::new(16, 248),
        "noise defence",
        s.defence_level as u32,
        s.defence_max as u32,
        s.defence_rung,
    );

    // --- the bring-up facts, small, at the foot ---------------------------
    //
    // Small because they matter once — but they matter a lot then, and a sealed
    // device with no console has nowhere else to say them.
    let mut antenna = Text32::new();
    let _ = antenna.push_str("antenna ");
    let _ = write_u32(&mut antenna, s.antenna_khz);
    let _ = antenna.push_str(" kHz · IRQ ");
    let _ = antenna.push_str(if s.irq_confirmed { "OK" } else { "NOT CONFIRMED" });
    let _ = antenna.push_str(" · disturbers ");
    let _ = write_u32(&mut antenna, s.disturbers_total);

    let _ = Line::new(
        Point::new(16, HEIGHT as i32 - 40),
        Point::new(WIDTH as i32 - 16, HEIGHT as i32 - 40),
    )
    .into_styled(black)
    .draw(frame);
    let _ = Text::with_baseline(
        &antenna,
        Point::new(16, HEIGHT as i32 - 32),
        MonoTextStyle::new(&FONT_6X10, INK),
        Baseline::Top,
    )
    .draw(frame);
}

/// A labelled horizontal bar.
///
/// Deliberately a bar rather than a number: the question these answer is "how
/// close to the limit", which a filled proportion says at a glance and a
/// figure makes you compute.
fn gauge(
    frame: &mut Display7in5,
    at: Point,
    label: &str,
    value: u32,
    max: u32,
    detail: &str,
) {
    const BAR_WIDTH: u32 = 360;
    const BAR_HEIGHT: u32 = 28;

    let _ = LABEL.render(
        label,
        Point::new(at.x, at.y),
        VerticalPosition::Baseline,
        FontColor::Transparent(INK),
        frame,
    );

    let bar = Rectangle::new(
        Point::new(at.x, at.y + 12),
        Size::new(BAR_WIDTH, BAR_HEIGHT),
    );
    let _ = bar
        .into_styled(PrimitiveStyle::with_stroke(INK, 2))
        .draw(frame);

    if max > 0 && value > 0 {
        // Inset by the stroke so the fill does not sit on the border.
        let filled = (BAR_WIDTH - 4) * value.min(max) / max;
        let _ = Rectangle::new(
            Point::new(at.x + 2, at.y + 14),
            Size::new(filled, BAR_HEIGHT - 4),
        )
        .into_styled(PrimitiveStyle::with_fill(INK))
        .draw(frame);
    }

    let mut caption = Text32::new();
    let _ = write_u32(&mut caption, value);
    let _ = caption.push('/');
    let _ = write_u32(&mut caption, max);
    let _ = caption.push_str("  ");
    let _ = caption.push_str(detail);
    let _ = Text::with_baseline(
        &caption,
        Point::new(at.x + BAR_WIDTH as i32 + 12, at.y + 14),
        MonoTextStyle::new(&FONT_9X15, INK),
        Baseline::Top,
    )
    .draw(frame);
}

/// Append a decimal `u32` without `format!`.
///
/// The render path runs on a device that is meant to stay up for weeks;
/// `format!` allocates, and the allocation is avoidable here for the sake of
/// four lines.
fn write_u32<const N: usize>(out: &mut heapless::String<N>, mut value: u32) -> Result<(), ()> {
    if value == 0 {
        return out.push('0').map_err(|_| ());
    }
    let mut digits = [0u8; 10];
    let mut used = 0;
    while value > 0 {
        digits[used] = b'0' + (value % 10) as u8;
        value /= 10;
        used += 1;
    }
    for i in (0..used).rev() {
        out.push(digits[i] as char).map_err(|_| ())?;
    }
    Ok(())
}


/// The device's own vital signs, as one line.
fn status_line(frame: &mut Display7in5, s: &Status<'_>) {
    let style = MonoTextStyle::new(&FONT_9X15, INK);
    let mut left = Text64::new();

    // Uptime first: it is the number that says whether anything below it has
    // had time to mean anything.
    let _ = left.push_str("up ");
    let hours = s.uptime_minutes / 60;
    if hours > 0 {
        let _ = write_u32(&mut left, hours);
        let _ = left.push('h');
    }
    let _ = write_u32(&mut left, s.uptime_minutes % 60);
    let _ = left.push_str("m");

    let _ = left.push_str("   ");
    let _ = write_u32(&mut left, s.health.cpu_mhz);
    let _ = left.push_str(" MHz");

    if let Some(tenths) = s.health.die_temp_tenths {
        let _ = left.push_str("   die ");
        if tenths < 0 {
            let _ = left.push('-');
        }
        let magnitude = tenths.unsigned_abs();
        let _ = write_u32(&mut left, magnitude / 10);
        let _ = left.push('.');
        let _ = write_u32(&mut left, magnitude % 10);
        let _ = left.push_str(" C");
    }

    let _ = left.push_str("   heap ");
    let _ = write_u32(&mut left, s.health.free_heap_kb);
    let _ = left.push_str(" KB");

    let _ = Text::with_baseline(&left, Point::new(16, 8), style, Baseline::Top).draw(frame);

    // Battery on the right, because it is the one field a passer-by looks for.
    let mut right = Text64::new();
    match s.battery {
        Some(reading) => {
            let _ = write_u32(&mut right, reading.percent as u32);
            let _ = right.push_str("%  ");
            let _ = write_u32(&mut right, reading.millivolts as u32 / 1000);
            let _ = right.push('.');
            let hundredths = (reading.millivolts % 1000) / 10;
            if hundredths < 10 {
                let _ = right.push('0');
            }
            let _ = write_u32(&mut right, hundredths as u32);
            let _ = right.push_str(" V");

            if reading.is_charging() {
                let _ = right.push_str("  charging");
            } else if let Some(hours) = reading.hours_remaining() {
                let _ = right.push_str("  ");
                if hours >= 48 {
                    let _ = write_u32(&mut right, hours / 24);
                    let _ = right.push_str("d left");
                } else {
                    let _ = write_u32(&mut right, hours);
                    let _ = right.push_str("h left");
                }
            }
            // No "left" figure at all when the rate is too small to divide by.
            // §2.1's CRATE is a real measurement, and a device that has just
            // woken has not discharged measurably yet -- printing a number
            // there would be a division by nearly zero wearing a hat.
        }
        None => {
            let _ = right.push_str("no gauge");
        }
    }
    let _ = Text::with_alignment(
        &right,
        Point::new(WIDTH as i32 - 16, 8),
        style,
        Alignment::Right,
    )
    .draw(frame);
}
