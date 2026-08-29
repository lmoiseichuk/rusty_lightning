//! Reading a log row back, whatever shape the file is.
//!
//! **Free of ESP-IDF so it can be host-tested**, like `defence` and `verdict`.
//! `log.rs` cannot be: it registers a littlefs partition. This module is the
//! part that has to be right and is easy to get wrong.
//!
//! ## Why by name and not by position
//!
//! The replay path read fields 0..3 as epoch, iso, distance, energy. Adding
//! three columns in front of `iso` — `millis`, `kind`, `nf` — silently broke it:
//! `distance` landed on the string `lightning`, which parses as neither a
//! kilometre count nor a sentinel, so **every new record would have been skipped
//! on replay** and the charts would have quietly stopped filling.
//!
//! Positional parsing makes a column addition a breaking change to a file
//! format that is explicitly append-only. Reading the header once and looking
//! columns up by name makes it not.
//!
//! ## Two shapes, both valid
//!
//! Old files have eight columns and no `kind`. Every row in one is a strike,
//! because that is all the firmware logged. New files carry `kind` and hold
//! disturbers and noise as well — and those must never be replayed as strikes,
//! which is the second thing positional parsing would have got wrong.

/// Where each column this reader cares about sits, by name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Columns {
    pub timestamp: usize,
    pub distance: usize,
    pub energy: usize,
    /// `None` in a file written before event logging existed.
    pub kind: Option<usize>,
    /// `None` in a file written before merged flashes recorded their count.
    pub strokes: Option<usize>,
    pub width: usize,
}

impl Columns {
    /// Work out the layout from a header line.
    ///
    /// Returns `None` if a column this reader needs is missing, which is the
    /// honest answer for a file this build cannot read — better than guessing
    /// at offsets and replaying nonsense into the charts.
    pub fn from_header(header: &str) -> Option<Columns> {
        let names: Vec<&str> = header.split(',').map(str::trim).collect();
        let at = |want: &str| names.iter().position(|n| *n == want);
        Some(Columns {
            timestamp: at("timestamp")?,
            distance: at("distance_km")?,
            energy: at("energy_raw")?,
            kind: at("kind"),
            strokes: at("strokes"),
            width: names.len(),
        })
    }
}

/// What one row turned out to be.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Row {
    /// A strike: epoch, distance field, energy, and how many return strokes
    /// the merge window folded into it.
    ///
    /// `strokes` is 1 for a file written before the column existed, which is
    /// the truthful reading: those records are single strikes as far as
    /// anything can now tell.
    Strike { epoch: u64, energy_raw: u32, distance: Distance, strokes: u32 },
    /// A disturber or noise event — logged, and never a strike.
    Event,
    /// Unreadable, or a row this build does not understand.
    Skip,
}

/// The distance the chip reported, as the file spells it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Distance {
    Overhead,
    OutOfRange,
    Km(u8),
}

/// Parse one row against a known layout.
pub fn parse_row(line: &str, columns: &Columns) -> Row {
    let fields: Vec<&str> = line.split(',').collect();
    // A short row is a truncated write, which a power cut can leave behind.
    if fields.len() < columns.width {
        return Row::Skip;
    }

    // **The kind gate comes first.** A disturber row has empty distance and
    // energy columns; without this it would fall through to the parse below and
    // be skipped for the wrong reason today, and replayed as a strike the moment
    // somebody made those columns tolerant.
    if let Some(kind) = columns.kind {
        match fields[kind].trim() {
            "lightning" => {}
            "" => {}
            _ => return Row::Event,
        }
    }

    let Ok(epoch) = fields[columns.timestamp].trim().parse::<u64>() else {
        return Row::Skip;
    };
    // Zero means the strike happened while the clock was unset. It is a real
    // record, but it cannot be placed on a time axis.
    if epoch == 0 {
        return Row::Skip;
    }
    let Ok(energy_raw) = fields[columns.energy].trim().parse::<u32>() else {
        return Row::Skip;
    };
    let distance = match fields[columns.distance].trim() {
        "overhead" => Distance::Overhead,
        "far" => Distance::OutOfRange,
        km => match km.parse::<u8>() {
            Ok(km) => Distance::Km(km),
            Err(_) => return Row::Skip,
        },
    };
    // **Read, not assumed.** The replay used to hardcode 1 here, with a comment
    // saying the count was "not recoverable from the columns" -- but `strokes`
    // has been the eleventh column since merged flashes started recording it,
    // and `log::append` writes it. So a replayed storm showed every multi-stroke
    // flash as a single strike, and the comment sent the next reader looking in
    // the wrong place. A file older than the column still reads 1, which is what
    // the old comment was right about and is now the only case it covers.
    let strokes = match columns.strokes {
        Some(at) => fields[at].trim().parse::<u32>().unwrap_or(1).max(1),
        None => 1,
    };

    Row::Strike { epoch, energy_raw, distance, strokes }
}
