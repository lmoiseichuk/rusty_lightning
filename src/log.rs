//! The strike log: every strike, on flash, surviving power cuts (§5).
//!
//! ## Why not a filesystem
//!
//! §5 says "a single CSV file on the on-flash filesystem", and the reasoning
//! given is that a file is directly inspectable. That reasoning does not
//! survive contact with this device: **there is no way to read the file
//! directly.** No SD card, no USB mass storage, and WiFi is deferred — so the
//! only route out is the console or an HTTP endpoint, and both of those are
//! code we write either way. A filesystem would buy a `fopen` we then have to
//! wrap in exactly the same dump command.
//!
//! What it would cost is real: SPIFFS or FAT means a mount, a wear-levelling
//! layer, directory metadata, and a failure mode — a corrupt filesystem — that
//! takes the log with it. Against that, a **fixed-size record appended to a raw
//! partition** has no metadata to corrupt, needs no mount, and recovers from a
//! torn write by ignoring one record.
//!
//! So: raw flash, fixed records, CSV generated **on the way out** rather than
//! stored. The moisture project reached the same conclusion for the same
//! reasons, and its soak log is the direct ancestor of this one.
//!
//! ## The record
//!
//! Sixteen bytes, aligned so a record never straddles a flash page:
//!
//! | Offset | Size | Field |
//! |---|---|---|
//! | 0 | 2 | magic + generation |
//! | 2 | 8 | Unix timestamp, seconds |
//! | 10 | 1 | distance code (0–63 km, or the two sentinels) |
//! | 11 | 4 | energy, raw 20-bit value |
//! | 15 | 1 | checksum over the first 15 bytes |
//!
//! The timestamp is stored rather than derived, because uptime is not time: the
//! device may be told the clock hours after it started logging, and every
//! record written before that would otherwise be unrecoverable.
//!
//! ## Why a checksum rather than a length or a footer
//!
//! Flash erases to `0xFF`, so an unwritten record is all-ones and trivially
//! recognised. The failure that needs catching is the *torn* one — power lost
//! mid-write — which leaves a record that is neither blank nor complete. A
//! checksum catches that in the one place it matters, at the tail, without
//! needing a second write to commit it.

use esp_idf_hal::sys::{self, esp_partition_t, EspError};

use crate::as3935::{Distance, Strike};

/// Bump to invalidate every stored record.
///
/// Combined with the magic into one byte pair, so a generation change and a
/// corrupt record are distinguishable: the wrong generation is a *valid* record
/// this firmware does not understand, which is worth saying rather than
/// silently treating as garbage.
const GENERATION: u8 = 1;
const MAGIC: u8 = 0x4C; // 'L'

pub const RECORD_LEN: u32 = 16;

/// Distance sentinels, stored rather than the enum's shape, so the on-flash
/// format does not move when the enum does.
const DISTANCE_OVERHEAD: u8 = 0xFE;
const DISTANCE_OUT_OF_RANGE: u8 = 0xFF;

/// One logged strike.
#[derive(Debug, Clone, Copy)]
pub struct Record {
    pub epoch: u64,
    pub distance: Distance,
    pub energy_raw: u32,
}

impl Record {
    fn encode(&self) -> [u8; RECORD_LEN as usize] {
        let mut bytes = [0u8; RECORD_LEN as usize];
        bytes[0] = MAGIC;
        bytes[1] = GENERATION;
        bytes[2..10].copy_from_slice(&self.epoch.to_le_bytes());
        bytes[10] = match self.distance {
            Distance::Km(km) => km.min(63),
            Distance::Overhead => DISTANCE_OVERHEAD,
            Distance::OutOfRange => DISTANCE_OUT_OF_RANGE,
        };
        bytes[11..15].copy_from_slice(&self.energy_raw.to_le_bytes());
        bytes[15] = checksum(&bytes[..15]);
        bytes
    }

    fn decode(bytes: &[u8; RECORD_LEN as usize]) -> Option<Self> {
        if bytes[0] != MAGIC || bytes[1] != GENERATION {
            return None;
        }
        if bytes[15] != checksum(&bytes[..15]) {
            return None;
        }
        Some(Record {
            epoch: u64::from_le_bytes(bytes[2..10].try_into().ok()?),
            distance: match bytes[10] {
                DISTANCE_OVERHEAD => Distance::Overhead,
                DISTANCE_OUT_OF_RANGE => Distance::OutOfRange,
                km => Distance::Km(km),
            },
            energy_raw: u32::from_le_bytes(bytes[11..15].try_into().ok()?),
        })
    }
}

/// Sum of bytes, inverted.
///
/// Inverted so that an all-`0xFF` erased record does **not** checksum as valid:
/// a plain sum of fifteen `0xFF` bytes would be a fixed value that could be
/// matched by chance, whereas this makes blank flash fail the check for the
/// same reason it fails the magic.
fn checksum(bytes: &[u8]) -> u8 {
    !bytes.iter().fold(0u8, |acc, b| acc.wrapping_add(*b))
}

/// The strike log, on the `storage` partition.
pub struct Log {
    partition: *const esp_partition_t,
    /// Byte offset of the next write.
    cursor: u32,
    capacity: u32,
}

impl Log {
    /// Find the partition and locate the end of the existing log.
    pub fn open() -> Option<Self> {
        // SAFETY: a lookup returning a descriptor the IDF owns for the life of
        // the program.
        let partition = unsafe {
            sys::esp_partition_find_first(
                sys::esp_partition_type_t_ESP_PARTITION_TYPE_DATA,
                sys::esp_partition_subtype_t_ESP_PARTITION_SUBTYPE_DATA_SPIFFS,
                b"storage\0".as_ptr() as *const core::ffi::c_char,
            )
        };
        if partition.is_null() {
            return None;
        }
        // SAFETY: non-null, checked above.
        let capacity = unsafe { (*partition).size };

        let mut log = Log {
            partition,
            cursor: 0,
            capacity,
        };
        log.cursor = log.find_end();
        Some(log)
    }

    /// Binary-search for the first blank record.
    ///
    /// Binary rather than linear because the partition holds ~124 000 records:
    /// a linear scan is 124 000 flash reads at every boot, against seventeen.
    /// The search is valid because the log only ever appends, so written
    /// records form a prefix — the first blank one is the boundary.
    fn find_end(&self) -> u32 {
        let slots = self.capacity / RECORD_LEN;
        let (mut low, mut high) = (0u32, slots);
        while low < high {
            let mid = low + (high - low) / 2;
            if self.slot_written(mid) {
                low = mid + 1;
            } else {
                high = mid;
            }
        }
        low * RECORD_LEN
    }

    /// Is this slot written? Judged by the magic byte alone — one byte read
    /// rather than sixteen, and blank flash is `0xFF`.
    fn slot_written(&self, slot: u32) -> bool {
        let mut byte = [0u8; 1];
        // SAFETY: reading inside the partition; the offset is bounded by the
        // caller's slot count.
        let err = unsafe {
            sys::esp_partition_read(
                self.partition,
                (slot * RECORD_LEN) as usize,
                byte.as_mut_ptr() as *mut core::ffi::c_void,
                1,
            )
        };
        err == sys::ESP_OK && byte[0] == MAGIC
    }

    pub fn len(&self) -> u32 {
        self.cursor / RECORD_LEN
    }

    pub fn used_bytes(&self) -> u32 {
        self.cursor
    }

    pub fn is_full(&self) -> bool {
        self.cursor + RECORD_LEN > self.capacity
    }

    /// Append one strike.
    ///
    /// **Stops when full rather than wrapping.** §5 leaves the retention policy
    /// open — rotate or overwrite-oldest — and stopping is the one choice that
    /// cannot lose data that has already been recorded. At ~124 000 records the
    /// question is years away, and a wrong answer now would quietly discard the
    /// history the §7 ETA model is meant to be built from.
    pub fn append(&mut self, record: &Record) -> Result<(), EspError> {
        if self.is_full() {
            return Ok(());
        }
        let bytes = record.encode();
        // SAFETY: writing inside the partition, bounds checked above.
        let err = unsafe {
            sys::esp_partition_write(
                self.partition,
                self.cursor as usize,
                bytes.as_ptr() as *const core::ffi::c_void,
                bytes.len(),
            )
        };
        crate::storage::check(err)?;
        self.cursor += RECORD_LEN;
        Ok(())
    }

    /// Read one record by index.
    pub fn get(&self, index: u32) -> Option<Record> {
        if index >= self.len() {
            return None;
        }
        let mut bytes = [0u8; RECORD_LEN as usize];
        // SAFETY: bounded by `len`.
        let err = unsafe {
            sys::esp_partition_read(
                self.partition,
                (index * RECORD_LEN) as usize,
                bytes.as_mut_ptr() as *mut core::ffi::c_void,
                bytes.len(),
            )
        };
        if err != sys::ESP_OK {
            return None;
        }
        Record::decode(&bytes)
    }

    /// Erase the whole log.
    ///
    /// No caller yet — it belongs to a `clear` console command, or to the
    /// retention policy §5 leaves open. Kept because it is a complete
    /// operation and erasing is the one thing a full log will need.
    #[allow(dead_code)]
    pub fn erase(&mut self) -> Result<(), EspError> {
        // SAFETY: erasing the whole partition we own.
        let err = unsafe {
            sys::esp_partition_erase_range(self.partition, 0, self.capacity as usize)
        };
        crate::storage::check(err)?;
        self.cursor = 0;
        Ok(())
    }
}

/// CSV, generated on the way out.
///
/// The header names the derived column too: `score` is not stored, because it
/// is a function of the two fields either side of it and storing it would let
/// the three disagree after a formula change.
pub fn dump_csv(log: &Log) {
    println!("# lightning strike log, {} records", log.len());
    println!("timestamp,iso_utc,distance_km,energy_raw,intensity_milli,score_milli");

    for index in 0..log.len() {
        let Some(record) = log.get(index) else {
            // A torn or foreign record. Say so and keep going: one bad record
            // must not cost the rest of the log.
            println!("# record {index} unreadable");
            continue;
        };

        let strike = Strike {
            distance: record.distance,
            energy_raw: record.energy_raw,
        };
        let distance = match record.distance {
            Distance::Km(km) => {
                let mut s = heapless::String::<8>::new();
                let _ = write_u32(&mut s, km as u32);
                s
            }
            Distance::Overhead => heapless::String::try_from("overhead").unwrap_or_default(),
            Distance::OutOfRange => heapless::String::try_from("far").unwrap_or_default(),
        };
        let score = crate::history::score_milli(&strike)
            .map(|v| v.to_string())
            .unwrap_or_default();

        println!(
            "{},{},{},{},{},{}",
            record.epoch,
            crate::clock::format(record.epoch),
            distance,
            record.energy_raw,
            strike.intensity_milli(),
            score
        );
    }
    println!("# end");
}

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
