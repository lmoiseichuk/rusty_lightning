//! The strike log: a CSV file on LittleFS, surviving power cuts (§5).
//!
//! Lines are appended as strikes happen, buffered in RAM, and `fsync`ed once a
//! minute. The file's length is the cursor, the filesystem checks its own
//! integrity, and the stored form is already the output form — so there is no
//! end-of-log search, no per-record checksum, and nothing to render on the way
//! out.
//!
//! **See §5 for why**: LittleFS over SPIFFS given this board's ON/OFF switch,
//! why that choice is what makes minute-scale batching safe, and the two format
//! decisions (`overhead`/`far` as words, an unset clock writing `0`).

use std::fmt::Write as _;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};

use esp_idf_hal::sys;

use crate::as3935::{Distance, Strike};

const MOUNT_POINT: &str = "/lfs";
const PARTITION_LABEL: &str = "storage";
const PATH: &str = "/lfs/strikes.csv";

/// The header row, which is also how an empty log is recognised.
///
/// `simulated` is `1` for a strike injected by the `strike` console command and
/// `0` for one the sensor decoded. Without it the two are **indistinguishable
/// on disk**, and the question the log exists to answer — has this device ever
/// detected real lightning? — cannot be asked of it. A digit rather than a word
/// because this column is for filtering, where `distance_km`'s `overhead` and
/// `far` are for reading.
/// `strokes` is how many return strokes the merge window folded into this row
/// (§4.3). One flash is normally three to four of them, so without this column a
/// merged record is indistinguishable from a single strike — the same ambiguity
/// `simulated` was added to remove, and `energy_raw` is a *sum* across them.
/// `kind` is what the chip said this event was: `lightning`, `disturber` or
/// `noise`.
///
/// **Only lightning was ever logged, and that is why the central question about
/// this device cannot be answered.** On 2026-08-13 a visible overhead cell
/// produced 103,000 disturbers and 27 records. The repo's own prose says "the
/// 103,000 disturbers were the storm" — that real flashes were arriving and
/// failing the chip's waveform validation — but nobody could ever check it,
/// because the file held no disturber timestamps to line up against the flashes.
/// One column makes a two-week-old argument into a measurement.
///
/// `millis` is the sub-second part of the arrival, and it settles a separate
/// anomaly. Of 1040 real records in `storm-2026-08-12.csv`, **not one shares a
/// second with another**; under the observed rate ~46 same-second pairs were
/// expected, so the odds of that happening by chance are about 1e-20. Nothing in
/// the code implements a one-second floor — `append` does no deduplication and
/// the reader had no rate limit — so either the servicing path has a dead time
/// that costs events when a storm is heaviest, or the epoch column is simply
/// coarse. Sub-second arrival times tell those two apart in one storm.
const HEADER: &str = "timestamp,millis,kind,nf,iso_local,distance_km,energy_raw,\
                      intensity_milli,score_milli,simulated,strokes";

/// What the chip classified an event as.
///
/// A column rather than three files: the whole point is to compare arrival times
/// across kinds, and that is a sort, not a join.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Lightning,
    Disturber,
    Noise,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Lightning => "lightning",
            Kind::Disturber => "disturber",
            Kind::Noise => "noise",
        }
    }
}

/// How often buffered lines are flushed and synced.
pub const SYNC_INTERVAL_MS: u32 = 60_000;

pub struct Log {
    /// Records written, counted at open from the file itself.
    records: u32,
    bytes: u32,
    /// How many more non-lightning events may be logged. Zero means off.
    ///
    /// **Event logging is a bounded measurement, not a mode.** At the rate this
    /// device sees indoors — 4.4 noise and 4.2 disturbers a second, measured —
    /// logging every event writes about 31,000 rows an hour, and there is no
    /// rotation and no size bound in this file: the log grows until the
    /// filesystem is full and then writes fail. 1844 KB of free flash is about
    /// an hour at that rate.
    ///
    /// So arming it costs a budget, and when the budget runs out it stops and
    /// says so. That makes it safe to leave running through a storm nobody is
    /// watching, which is exactly when the measurement is wanted.
    ///
    /// **Not persisted**, matching `sensitive on`: a device that came back from
    /// a power cut silently filling its flash would be the same kind of trap.
    event_budget: u32,
    /// Lines appended since the last sync. Kept in memory so a storm does not
    /// become one flash write per strike.
    pending: Vec<String>,
}

impl Log {
    /// Mount the filesystem and prepare the log.
    pub fn open() -> Option<Self> {
        let mount = std::ffi::CString::new(MOUNT_POINT).ok()?;
        let label = std::ffi::CString::new(PARTITION_LABEL).ok()?;

        let mut config = sys::littlefs::esp_vfs_littlefs_conf_t {
            base_path: mount.as_ptr(),
            partition_label: label.as_ptr(),
            ..Default::default()
        };
        // The flags are a bindgen bitfield rather than plain bools, so they are
        // set through generated accessors rather than in the initialiser.
        config.set_read_only(0);
        config.set_dont_mount(0);

        // **Try WITHOUT formatting first, and say so if that fails.**
        //
        // This used to be one call with `format_if_mount_failed` set, and
        // nothing reported when it fired — so a mount failure silently erased
        // every strike this device had ever recorded and came back looking like
        // a device that had simply never seen a storm. The owner reported
        // exactly that: "reboot cleans them".
        //
        // The old comment argued the cost was nothing, because "in that
        // situation the log is already unreadable". That is an assumption, and
        // it is the wrong way round for littlefs specifically: it is designed to
        // survive a power cut, so a mount failure after one is more often
        // transient than terminal. This board has been browning out — the
        // e-paper refresh on USB power alone — which is precisely the way to
        // reach an unclean mount without any real corruption.
        //
        // So the format still happens, because a device that cannot log at all
        // is worse. But it happens as a *decision*, with a line saying what it
        // cost, instead of as a side effect nobody could see.
        config.set_format_if_mount_failed(0);
        // SAFETY: a plain IDF call; both CStrings outlive it.
        let first = unsafe { sys::littlefs::esp_vfs_littlefs_register(&config) };

        if first != sys::ESP_OK {
            println!("log:  ⚠ the log filesystem would not mount ({first})");
            let had = crate::settings::logged_records().unwrap_or(0);
            match had {
                0 => println!("log:    nothing was recorded on it, so nothing is lost"),
                n => println!("log:    {n} record(s) were on it and are about to be ERASED"),
            }
            println!("log:    formatting, because a device that cannot log is worse");
            config.set_format_if_mount_failed(1);
            // SAFETY: as above. The failed registration left nothing mounted.
            let second = unsafe { sys::littlefs::esp_vfs_littlefs_register(&config) };
            if second != sys::ESP_OK {
                println!("log:  ⚠ and the format failed too ({second}) -- no log this boot");
                return None;
            }
            let _ = crate::settings::store_logged_records(0);
        }

        let mut log = Log {
            records: 0,
            bytes: 0,
            event_budget: 0,
            pending: Vec::new(),
        };
        log.ensure_header();
        log.recount();

        // **The second way a log goes missing, and the one the first check
        // cannot see.** If the filesystem was formatted by something other than
        // the branch above -- a previous boot's format, an erase during
        // flashing, a partition change -- then the mount SUCCEEDS and the file
        // is simply empty. Nothing is wrong from here; the records are just
        // gone.
        //
        // NVS is a different partition and survives all of that, so comparing
        // the two is what turns a silent disappearance into a reported one. The
        // owner's complaint was precisely this shape: "reboot cleans them".
        let remembered = crate::settings::logged_records().unwrap_or(0);
        if remembered > log.records {
            println!(
                "log:  ⚠ {} record(s) were remembered, {} are on disk -- {} LOST",
                remembered,
                log.records,
                remembered - log.records
            );
            println!("log:    the log filesystem was reformatted or replaced since the last sync");
            // Re-anchor, or every subsequent boot repeats a loss that has
            // already been reported and cannot be undone.
            let _ = crate::settings::store_logged_records(log.records);
        }

        Some(log)
    }

    /// Write the header if the file is new or empty, and warn if it is stale.
    ///
    /// A header tells a reader — a person, a spreadsheet, or a browser — what
    /// the columns are, and its presence distinguishes "this log exists and
    /// holds nothing" from "there is no log".
    ///
    /// **It is only written to an empty file**, which means a format change
    /// never reaches an existing log — caught in testing, where a renamed
    /// column left old rows described by the old header. Rewriting the file to
    /// fix it would be worse: that discards records to correct a label. So the
    /// mismatch is reported and left alone, and `clear` is the deliberate way
    /// to start again.
    fn ensure_header(&mut self) {
        let empty = std::fs::metadata(PATH).map(|m| m.len() == 0).unwrap_or(true);
        if !empty {
            if let Ok(file) = File::open(PATH) {
                if let Some(Ok(first)) = BufReader::new(file).lines().next() {
                    if first != HEADER {
                        println!("log:  ⚠ header is from an older format:");
                        println!("log:    on disk: {first}");
                        println!("log:    current: {HEADER}");
                        println!("log:    rows still parse; use `clear` to start fresh");
                    }
                }
            }
            return;
        }
        if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(PATH) {
            let _ = writeln!(file, "{HEADER}");
            let _ = file.flush();
            let _ = file.sync_all();
        }
    }

    /// Count what is already on disk.
    ///
    /// Derived from the file rather than kept in a separate counter, so the two
    /// cannot disagree after a crash. Done once at open.
    fn recount(&mut self) {
        self.bytes = std::fs::metadata(PATH).map(|m| m.len() as u32).unwrap_or(0);
        self.records = File::open(PATH)
            .map(|file| {
                BufReader::new(file)
                    .lines()
                    .skip(1) // the header
                    .filter(|line| line.as_ref().map(|l| !l.trim().is_empty()).unwrap_or(false))
                    .count() as u32
            })
            .unwrap_or(0);
    }

    pub fn len(&self) -> u32 {
        self.records + self.pending.len() as u32
    }

    pub fn used_bytes(&self) -> u32 {
        self.bytes
    }

    /// Free space on the partition, in bytes.
    pub fn free_bytes(&self) -> u32 {
        let Ok(label) = std::ffi::CString::new(PARTITION_LABEL) else {
            return 0;
        };
        let (mut total, mut used) = (0usize, 0usize);
        // SAFETY: a read-only query against a mounted partition.
        let err =
            unsafe { sys::littlefs::esp_littlefs_info(label.as_ptr(), &mut total, &mut used) };
        if err != sys::ESP_OK {
            return 0;
        }
        total.saturating_sub(used) as u32
    }

    /// Record a strike. Buffered; see [`Log::sync`].
    /// Record an event the chip did not call lightning.
    ///
    /// **Deliberately cheap and deliberately not a `Strike`.** A disturber
    /// carries no distance and no energy — the chip rejected the waveform before
    /// measuring it — so every reading column is empty. What it carries is the
    /// only thing being asked of it: *when*.
    /// Arm event logging for `rows` more non-lightning events.
    ///
    /// Returns the budget actually set. Zero turns it off.
    pub fn arm_events(&mut self, rows: u32) -> u32 {
        self.event_budget = rows;
        rows
    }

    pub fn event_budget(&self) -> u32 {
        self.event_budget
    }

    pub fn append_event(&mut self, epoch: u64, millis: u32, nf: u8, kind: Kind) {
        match self.event_budget {
            0 => return,
            1 => {
                self.event_budget = 0;
                println!("log:  event budget spent -- disturbers and noise are no longer logged");
                println!("log:  `events <rows>` arms another window; `dump` to read what was caught");
                return;
            }
            budget => self.event_budget = budget - 1,
        }

        let mut line = String::with_capacity(64);
        let iso = match epoch {
            0 => String::new(),
            epoch => crate::clock::format_local(epoch).to_string(),
        };
        let _ = write!(line, "{epoch},{millis},{},{nf},{iso},,,,,0,0", kind.as_str());
        self.pending.push(line);
    }

    pub fn append(&mut self, epoch: u64, millis: u32, nf: u8, strike: &Strike, simulated: bool, strokes: u32) {
        let mut line = String::with_capacity(96);

        // Words rather than numbers for the two sentinels. "Overhead" and "out
        // of range" are not distances, and a consumer averaging this column
        // must not silently take them for 1 km and 63 km.
        let distance = match strike.distance {
            Distance::Overhead => "overhead".to_string(),
            Distance::OutOfRange => "far".to_string(),
            Distance::Km(km) => km.to_string(),
        };
        let score = crate::history::score_milli(strike)
            .map(|v| v.to_string())
            .unwrap_or_default();
        // An unset clock writes 0 and an empty ISO column rather than a
        // plausible 1970 date. The strike happened; what is unknown is when,
        // and saying so is recoverable where inventing it is not.
        // **Local time in the ISO column, UTC in the epoch column.** The epoch
        // is the machine-readable, unambiguous one; the ISO string is for a
        // person reading the file, and a person in Florida wants Florida time.
        // Keeping both means neither reader has to know about the other.
        let iso = if epoch == 0 {
            String::new()
        } else {
            crate::clock::format_local(epoch).to_string()
        };

        let _ = write!(
            line,
            "{},{},{},{},{},{},{},{},{},{},{}",
            epoch,
            millis,
            Kind::Lightning.as_str(),
            nf,
            iso,
            distance,
            strike.energy_raw,
            strike.intensity_milli(),
            score,
            simulated as u8,
            strokes
        );
        self.pending.push(line);
    }

    /// Flush and sync anything buffered.
    ///
    /// Returns how many records were written, so the caller can say so — a
    /// silent flush is indistinguishable from a lost one.
    pub fn sync(&mut self) -> Result<u32, std::io::Error> {
        if self.pending.is_empty() {
            return Ok(0);
        }
        let mut file = OpenOptions::new().create(true).append(true).open(PATH)?;

        // **Write first, drain only what was written.**
        //
        // This iterated `self.pending.drain(..)`, which removes each line as the
        // iterator advances — so a `writeln!` error returned immediately with
        // every *remaining* buffered line already drained and gone. The module
        // promises it "cannot lose data already recorded", and that was the one
        // path where it did.
        //
        // It is not a remote failure either. The log has no rotation and no size
        // bound, so "filesystem full" is a real end state, and it is reached
        // during a storm — the moment the buffered lines are worth most.
        //
        // Counting first and draining after means a failed sync keeps everything
        // it could not write, and the next sync tries again.
        let mut written = 0;
        for line in &self.pending {
            writeln!(file, "{line}")?;
            written += 1;
        }
        for line in self.pending.drain(..) {
            self.bytes += line.len() as u32 + 1;
            self.records += 1;
        }
        file.flush()?;
        // The call that makes the difference: without it the data sits in the
        // filesystem's cache and a power cut takes it.
        file.sync_all()?;
        // **The count goes to NVS, a different partition.** So if the log's
        // filesystem is ever lost, the device can say how many records went with
        // it rather than coming back looking like it had never seen a storm.
        // Written on the sync rather than the append, so it costs one NVS write
        // a minute at most, matching what is already on disk.
        let _ = crate::settings::store_logged_records(self.records);
        Ok(written)
    }

    /// Erase the log and start again.
    #[allow(dead_code)]
    pub fn clear(&mut self) -> Result<(), std::io::Error> {
        self.pending.clear();
        std::fs::remove_file(PATH)?;
        self.ensure_header();
        self.recount();
        // A deliberate erase is not a loss, so the remembered count follows it
        // down. Otherwise the next boot would report records that somebody
        // asked to be gone.
        let _ = crate::settings::store_logged_records(self.records);
        Ok(())
    }
}

/// Stream the file to the console.
///
/// A copy rather than a re-render: the stored form is already the output form,
/// which is the point of keeping CSV on disk. Anything reading this — a
/// terminal, `netcat`, or later an HTTP handler — gets identical bytes.
pub fn dump_csv(log: &Log) {
    println!("# lightning strike log, {} records", log.len());
    if !log.pending.is_empty() {
        println!("# ({} not yet synced, shown after the file)", log.pending.len());
    }
    match File::open(PATH) {
        Ok(file) => {
            for line in BufReader::new(file).lines().map_while(Result::ok) {
                println!("{line}");
            }
        }
        Err(e) => println!("# could not read {PATH}: {e}"),
    }
    // Buffered lines are shown too. They are real strikes that happened; the
    // only thing not yet true of them is that they are on flash.
    for line in &log.pending {
        println!("{line}");
    }
    println!("# end");
}


/// Walk the stored records, oldest first.
///
/// **This is what makes the charts survive a power cut.** The rings are RAM and
/// die with it; the file does not. Replaying at boot rebuilds the last day,
/// week and month from what actually happened rather than starting every reboot
/// at zero — which, on a device whose whole purpose is a multi-day picture of
/// the weather, is the difference between a chart and a demo.
///
/// Records with no timestamp are skipped: they cannot be placed on a time axis,
/// and guessing where they go would corrupt the very history this rebuilds.
/// They stay in the file, because the strike still happened.
///
/// Malformed lines are skipped rather than fatal. A power cut mid-append leaves
/// a torn final line, and one bad row must not cost the other thousand.
pub fn for_each<F>(mut visit: F)
where
    F: FnMut(u64, Strike),
{
    let Ok(file) = File::open(PATH) else {
        return;
    };
    let mut lines = BufReader::new(file).lines().map_while(Result::ok);

    // **The layout comes from the file's own header, not from this build's.**
    // See `crate::csv` — a file written before event logging has eight columns
    // and no `kind`, and reading it positionally against the current eleven
    // would have silently dropped every record.
    let Some(header) = lines.next() else { return };
    let Some(columns) = crate::csv::Columns::from_header(&header) else {
        println!("log:  ⚠ header has no columns this build understands -- not replaying");
        println!("log:    on disk: {header}");
        return;
    };

    for line in lines {
        match crate::csv::parse_row(&line, &columns) {
            crate::csv::Row::Strike { epoch, energy_raw, distance } => visit(
                epoch,
                Strike {
                    distance: match distance {
                        crate::csv::Distance::Overhead => Distance::Overhead,
                        crate::csv::Distance::OutOfRange => Distance::OutOfRange,
                        crate::csv::Distance::Km(km) => Distance::Km(km),
                    },
                    energy_raw,
                },
            ),
            crate::csv::Row::Event | crate::csv::Row::Skip => continue,
        }
    }
}
