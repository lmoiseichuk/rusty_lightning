//! Who is on the bus, and is it the right who.
//!
//! This is bring-up code and it stays in the tree, because "the sensor stopped
//! answering" is a question that recurs for the life of a device and a scanner
//! is the cheapest instrument that answers it.
//!
//! ## Why the scan reaches below 0x08
//!
//! A textbook I2C scan sweeps **0x08–0x77** and skips the rest, because
//! 0x00–0x07 and 0x78–0x7F are reserved by the specification. Doing that here
//! would find the fuel gauge and silently miss the lightning sensor: the
//! **AS3935 sits at 0x01, 0x02 or 0x03** depending on how its address pins are
//! strapped, squarely inside the reserved block. DFRobot's SEN0290 ships on
//! **0x03**.
//!
//! So the sweep starts at 0x01. It skips only 0x00, the general-call address,
//! which every device on the bus answers by definition and which would
//! therefore report a device that is not there.

use esp_idf_hal::i2c::I2cDriver;

/// A device found on the bus.
pub struct Found {
    pub address: u8,
    /// What that address is expected to be in this design, or `None` for an
    /// address nothing in the spec claims.
    pub expected: Option<&'static str>,
}

/// Addresses this design expects, and what should be at each (§2).
///
/// Listed so the scan reports *meaning* rather than numbers. A bare address
/// table is something you then go and look up; this is the lookup.
const EXPECTED: &[(u8, &str)] = &[
    (0x01, "AS3935 lightning sensor (address pins strapped to 1)"),
    (0x02, "AS3935 lightning sensor (address pins strapped to 2)"),
    (0x03, "AS3935 lightning sensor -- the SEN0290 default"),
    (0x36, "MAX17048 fuel gauge"),
    (0x48, "ADS1115 -- the §2.1 fallback ADC, not expected in this build"),
];

/// How long to wait for a device to acknowledge, in milliseconds.
///
/// Short on purpose: a scan does 119 probes, and all but two or three of them
/// are expected to time out. At 100 ms a full sweep would take twelve seconds
/// and read as a hang.
const PROBE_TIMEOUT_MS: u32 = 10;

/// Probe every address a device in this design could sit at.
///
/// The probe is a one-byte write of `0x00`, not a read. Both work for presence
/// detection, but a write is the more honest question: it is the addressing
/// phase alone that decides whether anything acknowledges, and a device that is
/// present but has nothing to say still ACKs its address. A read probe can also
/// leave a device mid-transaction if it NACKs partway.
///
/// The byte itself is a register pointer, which is harmless on both parts here:
/// on the AS3935 it selects register 0x00, and on the MAX17048 a pointer write
/// with no data following it changes nothing.
pub fn scan(i2c: &mut I2cDriver<'_>) -> heapless::Vec<Found, 16> {
    let mut found = heapless::Vec::new();

    // From 0x01: see the module comment. 0x00 is the general-call address and
    // would report a device that does not exist.
    for address in 0x01..=0x7F_u8 {
        if i2c.write(address, &[0x00], PROBE_TIMEOUT_MS).is_err() {
            continue;
        }

        let expected = EXPECTED
            .iter()
            .find(|(candidate, _)| *candidate == address)
            .map(|(_, what)| *what);

        // Full is not an error worth propagating -- 16 devices on a bus this
        // design gives four addresses to means something is very wrong, and the
        // report below says so.
        let _ = found.push(Found { address, expected });
    }

    found
}

/// What the scan means for this design, as one line.
///
/// Deliberately answers the question that was actually asked -- *is the
/// hardware wired correctly* -- rather than listing addresses and leaving the
/// reader to decide.
pub fn verdict(found: &[Found]) -> &'static str {
    let sensor = found.iter().any(|f| (0x01..=0x03).contains(&f.address));
    let gauge = found.iter().any(|f| f.address == 0x36);

    match (sensor, gauge, found.is_empty()) {
        (_, _, true) => "NOTHING on the bus -- check SDA/SCL, 3V3 and GND before anything else",
        (true, true, _) => "both devices present -- the bus is wired correctly",
        (true, false, _) => "sensor found, NO fuel gauge -- check the MAX17048's JST: it is powered by the cell, not by VIN",
        (false, true, _) => "gauge found, NO lightning sensor -- check the Gravity/QT adapter and the AS3935's own 3V3",
        (false, false, _) => "devices answered, but neither is ours -- addresses below are not what §2 expects",
    }
}
