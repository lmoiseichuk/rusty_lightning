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
const HEADER: &str = "timestamp,iso_local,distance_km,energy_raw,\
                      intensity_milli,score_milli,simulated,strokes";

/// How often buffered lines are flushed and synced.
pub const SYNC_INTERVAL_MS: u32 = 60_000;

pub struct Log {
    /// Records written, counted at open from the file itself.
    records: u32,
    bytes: u32,
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
        //
        // Format on a failed mount, so a virgin device — or one whose
        // filesystem did not survive something — comes up working rather than
        // dead. The cost of a spurious format is the log, and in that situation
        // the log is already unreadable.
        config.set_format_if_mount_failed(1);
        config.set_read_only(0);
        config.set_dont_mount(0);

        // SAFETY: a plain IDF call; both CStrings outlive it.
        let err = unsafe { sys::littlefs::esp_vfs_littlefs_register(&config) };
        if err != sys::ESP_OK {
            return None;
        }

        let mut log = Log {
            records: 0,
            bytes: 0,
            pending: Vec::new(),
        };
        log.ensure_header();
        log.recount();
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
    pub fn append(&mut self, epoch: u64, strike: &Strike, simulated: bool, strokes: u32) {
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
            "{},{},{},{},{},{},{},{}",
            epoch,
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
        let mut written = 0;
        for line in self.pending.drain(..) {
            writeln!(file, "{line}")?;
            self.bytes += line.len() as u32 + 1;
            self.records += 1;
            written += 1;
        }
        file.flush()?;
        // The call that makes the difference: without it the data sits in the
        // filesystem's cache and a power cut takes it.
        file.sync_all()?;
        Ok(written)
    }

    /// Erase the log and start again.
    #[allow(dead_code)]
    pub fn clear(&mut self) -> Result<(), std::io::Error> {
        self.pending.clear();
        std::fs::remove_file(PATH)?;
        self.ensure_header();
        self.recount();
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
    for line in BufReader::new(file).lines().map_while(Result::ok).skip(1) {
        let mut fields = line.split(',');
        let (Some(epoch), Some(_iso), Some(distance), Some(energy)) =
            (fields.next(), fields.next(), fields.next(), fields.next())
        else {
            continue;
        };

        let Ok(epoch) = epoch.trim().parse::<u64>() else {
            continue;
        };
        if epoch == 0 {
            continue;
        }
        let Ok(energy_raw) = energy.trim().parse::<u32>() else {
            continue;
        };
        let distance = match distance.trim() {
            "overhead" => Distance::Overhead,
            "far" => Distance::OutOfRange,
            km => match km.parse::<u8>() {
                Ok(km) => Distance::Km(km),
                Err(_) => continue,
            },
        };

        visit(
            epoch,
            Strike {
                distance,
                energy_raw,
            },
        );
    }
}
