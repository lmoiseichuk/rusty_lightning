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
pub const PATH: &str = "/lfs/strikes.csv";

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
const HEADER: &str = "timestamp,millis,kind,nf,srej,iso_local,distance_km,energy_raw,\
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

/// What [`Log::arm_events`] did, for the console to report truthfully.
pub struct Armed {
    /// The budget now in force: what was asked, or less if it did not fit.
    pub rows: u32,
    /// Whether NVS took it. `Err` leaves the save to the next sync.
    pub saved: Result<(), sys::EspError>,
    /// What NVS holds now, so what a restart before that sync would resume.
    pub on_disk: u32,
}

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
    /// **Persisted since 0.21.0, and capped to what fits.** It was RAM-only, on
    /// the argument that a device coming back from a power cut silently filling
    /// its flash would be a trap. Two things changed the answer. Every open of
    /// the console port restarts the board, so an armed window ended the
    /// moment anybody connected to see how it was going -- the measurement
    /// could not be read without being stopped. And the trap was the
    /// *filling*, not the persisting: every budget, typed or restored, now goes
    /// through [`crate::budget::capped`], which keeps a reserve the event rows
    /// can never spend, and the boot banner says when one is armed. Silent and
    /// unbounded were the problem; this is neither.
    event_budget: u32,
    /// The budget as NVS last recorded it, so `sync` writes only on a change.
    saved_budget: u32,
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
            saved_budget: 0,
            pending: Vec::new(),
        };
        log.ensure_header();
        log.recount();

        // **An armed `events` window resumes where the last sync left it.**
        // Re-capped against today's free space, because the rows written before
        // the restart are on the disk now and the reserve still has to stand.
        let stored = crate::settings::event_budget().unwrap_or(0);
        let resumed = crate::budget::capped(stored, log.free_bytes());
        log.event_budget = resumed;
        log.saved_budget = stored;
        if resumed < stored {
            println!(
                "log:  events: {stored} row(s) were left, the log has room for {resumed} -- armed {resumed}"
            );
            // Through the one save path, so a refusal here is retried by the
            // first sync rather than leaving NVS holding the larger count.
            log.save_budget();
        }

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
                        // **"Rows still parse" was wrong, and wrong in the
                        // direction that hides a storm.** The reader takes its
                        // column positions from the header on disk, so in an
                        // old-header file every row written in the new format
                        // is read at the old offsets -- `distance_km` lands on
                        // `kind`, "lightning" fails a `u8` parse, and the row
                        // is skipped. Old rows replay; new ones do not, and the
                        // counts disagree silently. Observed at 2092 records on
                        // disk against 2090 replayed.
                        //
                        // So the warning has to name the consequence and the
                        // number, because "an older format" reads as cosmetic
                        // and this is data that will not come back after a
                        // reboot.
                        let on_disk = first.split(',').count();
                        let current = HEADER.split(',').count();
                        println!("log:  ⚠⚠ this file's header is from an older format");
                        println!("log:    on disk: {on_disk} columns -- {first}");
                        println!("log:    current: {current} columns -- {HEADER}");
                        println!(
                            "log:    ROWS WRITTEN FROM NOW ON WILL NOT BE REPLAYED after a reboot:"
                        );
                        println!(
                            "log:    the reader takes its columns from the header, so a {current}-column"
                        );
                        println!(
                            "log:    row read at {on_disk}-column offsets fails and is skipped."
                        );
                        println!("log:    `dump` to keep what is here, then `clear` to start fresh.");
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
    /// cannot disagree after a crash. Done at open, and after `clear`.
    ///
    /// **Paced, like every walk over the whole file** -- see [`crate::pace`].
    /// This one runs at boot, where a task watchdog reset is not one lost dump
    /// but a board that restarts at the same line every time it starts.
    fn recount(&mut self) {
        self.bytes = std::fs::metadata(PATH).map(|m| m.len() as u32).unwrap_or(0);
        self.records = 0;
        let Ok(file) = File::open(PATH) else {
            return;
        };
        let mut pace = crate::pace::Pace::new(crate::now_ms());
        // `skip(1)` is the header.
        for line in BufReader::new(file).lines().skip(1) {
            match line {
                Ok(text) if text.trim().is_empty() => {}
                Ok(_) => self.records += 1,
                // A line that is not UTF-8 has been read past already, and is
                // skipped -- as the count always did.
                Err(e) if e.kind() == std::io::ErrorKind::InvalidData => {}
                // **Any other error ends the count.** It is the read itself
                // failing, and `lines()` does not end on an error: it returns
                // it and tries the same read again next time, so a failure
                // that persists repeats for ever. The iterator chain this
                // replaced counted until the iterator ended -- for a failing
                // read, never. Unpaced that was a watchdog reset at every
                // boot; paced it would be a boot that never finished. The
                // dump and the replay stop at the first error already.
                Err(_) => break,
            }
            yield_when_due(&mut pace);
        }
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
    /// Arm event logging for up to `rows` more non-lightning events.
    ///
    /// Returns the budget actually set, which is less than asked when the log
    /// has no room for it -- see [`crate::budget::capped`] -- and whether NVS
    /// took it. Zero turns it off. Saved at once, so a restart a second later
    /// still has it.
    ///
    /// **Not printed here**, because what a failed save means depends on the
    /// direction: an arm that did not save is lost by a restart, but a *disarm*
    /// that did not save is worse -- the restart brings the old window back.
    /// The caller says which; [`Armed::on_disk`] is what a restart would find.
    pub fn arm_events(&mut self, rows: u32) -> Armed {
        let armed = crate::budget::capped(rows, self.free_bytes());
        self.event_budget = armed;
        let saved = match crate::settings::store_event_budget(armed) {
            Ok(()) => {
                self.saved_budget = armed;
                Ok(())
            }
            // `saved_budget` keeps what NVS still holds, so the next sync sees
            // the two apart and tries again -- see `sync`.
            Err(e) => Err(e),
        };
        Armed { rows: armed, saved, on_disk: self.saved_budget }
    }

    pub fn event_budget(&self) -> u32 {
        self.event_budget
    }

    pub fn append_event(&mut self, epoch: u64, millis: u32, nf: u8, srej: u8, kind: Kind) {
        // **The last row is written, not spent announcing itself.** This used
        // to `return` on a budget of 1 after printing the exhaustion notice,
        // so `events 10` logged nine and `events 1` logged nothing at all --
        // an off-by-one that quietly understated every armed measurement by one
        // at its tail, which is the end of the window a measurement cares most
        // about. Decrement, append, and announce when the budget reaches zero.
        if self.event_budget == 0 {
            return;
        }
        self.event_budget -= 1;
        let exhausted = self.event_budget == 0;

        let iso = match epoch {
            0 => String::new(),
            epoch => crate::clock::format_local(epoch).to_string(),
        };
        // Built in `budget`, so the row the cap assumes is the row written.
        let line = crate::budget::event_row(epoch, millis, kind.as_str(), nf, srej, &iso);
        self.pending.push(line);

        // Announced after the row is queued, so the notice and the file agree
        // about how many were caught.
        if exhausted {
            println!("log:  event budget spent -- disturbers and noise are no longer logged");
            println!("log:  `events <rows>` arms another window; `dump` to read what was caught");
        }
    }

    pub fn append(&mut self, epoch: u64, millis: u32, nf: u8, srej: u8, strike: &Strike, simulated: bool, strokes: u32) {
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
        // person reading the file, and a person reading it wants local time.
        // Keeping both means neither reader has to know about the other.
        let iso = if epoch == 0 {
            String::new()
        } else {
            crate::clock::format_local(epoch).to_string()
        };

        let _ = write!(
            line,
            "{},{},{},{},{},{},{},{},{},{},{},{}",
            epoch,
            millis,
            Kind::Lightning.as_str(),
            nf,
            srej,
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
        // **The budget before the early return, not after it.** It was saved
        // only on a sync that wrote rows -- so once the budget reached zero no
        // rows came, every sync returned here, and a save that had failed was
        // never tried again. RAM said "off", NVS still held the old count, and
        // the next restart -- a console connection is one -- quietly armed a
        // window somebody had stopped. Nothing pending means every row the
        // budget paid for is on disk, which is the only moment the two may be
        // written to agree.
        if self.pending.is_empty() {
            self.save_budget();
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
        self.save_budget();
        Ok(written)
    }

    /// Write the event budget to NVS if it has moved since it last was.
    ///
    /// **Saved with the sync that writes the rows it paid for**, so after a
    /// restart the count on disk and the count left agree: rows still pending
    /// when power went are lost *and* still owed. Only on a change -- an idle
    /// minute writes nothing. A failed write leaves the two apart, so the next
    /// sync, idle or not, tries again.
    fn save_budget(&mut self) {
        if self.event_budget == self.saved_budget {
            return;
        }
        match crate::settings::store_event_budget(self.event_budget) {
            Ok(()) => self.saved_budget = self.event_budget,
            Err(e) => println!(
                "log:  events: could not save {} row(s) left ({e}) -- NVS still says {}; retried each sync",
                self.event_budget, self.saved_budget
            ),
        }
    }

    /// Erase the log and start again.
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

/// Block for one tick if `pace` says a walk over the log has kept the CPU long
/// enough.
///
/// Called once per step of every walk over the whole file -- the three in this
/// module and the web page's `/log.csv` -- and almost always it only reads the
/// clock and returns. See [`crate::pace`] for the watchdog this keeps quiet and
/// for the arithmetic.
///
/// **A block, not a yield.** `std::thread::yield_now` is `sched_yield`, which
/// ESP-IDF implements as `vTaskDelay(0)` -- see [`crate::pace::BLOCK_TICKS`]
/// for why that never lets the idle task in. Only blocking does, which is the
/// same reason `as3935::settle` takes its tick before it spins.
pub fn yield_when_due(pace: &mut crate::pace::Pace) {
    if pace.due(crate::now_ms()) {
        // SAFETY: a plain FreeRTOS call from task context, as in
        // `as3935::settle`. Back at the next tick interrupt.
        unsafe { sys::vTaskDelay(crate::pace::BLOCK_TICKS) };
    }
}

/// Stream the file to the console.
///
/// A copy rather than a re-render: the stored form is already the output form,
/// which is the point of keeping CSV on disk. Anything reading this — a
/// terminal, `netcat`, or later an HTTP handler — gets identical bytes.
///
/// **Paced, and that is the only thing the pacing changes.** Until 0.21.2 this
/// loop never blocked while the host kept up, so at about 1.32 MB -- some
/// 17,800 rows -- it outlasted the task watchdog and reset the board in the
/// middle of the dump. See [`crate::pace`]. Not a byte of the output moves:
/// the tools and the scripts that read it parse every line, `# end` included.
pub fn dump_csv(log: &Log) {
    let mut pace = crate::pace::Pace::new(crate::now_ms());
    println!("# lightning strike log, {} records", log.len());
    if !log.pending.is_empty() {
        println!("# ({} not yet synced, shown after the file)", log.pending.len());
    }
    match File::open(PATH) {
        Ok(file) => {
            for line in BufReader::new(file).lines().map_while(Result::ok) {
                println!("{line}");
                yield_when_due(&mut pace);
            }
        }
        Err(e) => println!("# could not read {PATH}: {e}"),
    }
    // Buffered lines are shown too. They are real strikes that happened; the
    // only thing not yet true of them is that they are on flash. Paced as
    // well: a minute of an armed `events` window is hundreds of them.
    for line in &log.pending {
        println!("{line}");
        yield_when_due(&mut pace);
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
/// The visitor also receives the stroke count, which the file carries and the
/// replay used to discard.
///
/// **Paced, like every walk over the whole file** -- see [`crate::pace`]. It
/// runs at boot, before the console is up, where a watchdog reset would repeat
/// on every start.
pub fn for_each<F>(mut visit: F)
where
    F: FnMut(u64, Strike, u32),
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

    let mut pace = crate::pace::Pace::new(crate::now_ms());
    for line in lines {
        // First, so the event rows the `continue` below skips are paced too.
        yield_when_due(&mut pace);
        match crate::csv::parse_row(&line, &columns) {
            crate::csv::Row::Strike { epoch, energy_raw, distance, strokes } => visit(
                epoch,
                Strike {
                    distance: match distance {
                        crate::csv::Distance::Overhead => Distance::Overhead,
                        crate::csv::Distance::OutOfRange => Distance::OutOfRange,
                        crate::csv::Distance::Km(km) => Distance::Km(km),
                    },
                    energy_raw,
                },
                strokes,
            ),
            crate::csv::Row::Event | crate::csv::Row::Skip => continue,
        }
    }
}
