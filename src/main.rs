//! Lightning-detector terminal — bring-up.
//!
//! Build order step 1 + 2 (§10): prove the toolchain and the console, then find
//! out who is actually on the I2C bus. Combined into one flash because on a
//! board with no user LED, "the console printed something" *is* the blink test.

mod i2c_scan;

use esp_idf_hal::delay::FreeRtos;
use esp_idf_hal::i2c::{I2cConfig, I2cDriver};
use esp_idf_hal::peripherals::Peripherals;
use esp_idf_hal::units::Hertz;

/// I2C bus speed.
///
/// 100 kHz rather than the 400 kHz the MicroPython reference uses. This is
/// bring-up over hand-soldered wires and a QT cable chain, where the bus
/// capacitance is unknown and a marginal rise time shows up as intermittent
/// NACKs that look exactly like a missing device. Raise it once the chain is
/// known good — neither part needs the speed (§3's register reads are single
/// bytes).
const I2C_HZ: u32 = 100_000;

/// Seconds between scans.
///
/// It repeats rather than scanning once, so a cable can be reseated and the
/// result watched without reflashing.
const SCAN_PERIOD_S: u32 = 5;

fn main() {
    // The one line every esp-idf-hal program needs: it patches the runtime's
    // link-time behaviour so that `main` can be an ordinary Rust `fn main`.
    esp_idf_hal::sys::link_patches();

    // The USB-serial-JTAG console enumerates when the host opens the port,
    // which is a moment or two after boot. Anything printed before then goes
    // into a FIFO nobody is draining — so the banner would be the one thing
    // never seen.
    FreeRtos::delay_ms(2000);

    println!();
    println!("=== lightning terminal ===");
    println!("fw {}", env!("CARGO_PKG_VERSION"));

    let peripherals = match Peripherals::take() {
        Ok(peripherals) => peripherals,
        Err(e) => {
            println!("FATAL: peripherals unavailable -- {e}");
            return;
        }
    };

    // §2: the display consumes GPIO 2, 3, 4, 5, 8 and 10, which leaves the
    // XIAO's native I2C pads free. These two are fixed by that constraint
    // rather than chosen.
    let sda = peripherals.pins.gpio6;
    let scl = peripherals.pins.gpio7;
    // `Hertz(..)` rather than the `.Hz()` extension method: that one comes from
    // a `FromValueType` trait that has to be in scope, and the plain constructor
    // says the same thing without the import.
    let config = I2cConfig::new().baudrate(Hertz(I2C_HZ));

    let mut i2c = match I2cDriver::new(peripherals.i2c0, sda, scl, &config) {
        Ok(i2c) => i2c,
        Err(e) => {
            println!("FATAL: I2C0 would not initialise -- {e}");
            return;
        }
    };
    println!("i2c:  SDA=GPIO6 SCL=GPIO7 @ {} kHz", I2C_HZ / 1000);

    loop {
        let found = i2c_scan::scan(&mut i2c);

        println!();
        println!("scan: {} device(s)", found.len());
        for device in found.iter() {
            match device.expected {
                Some(what) => println!("      0x{:02x}  {}", device.address, what),
                None => println!("      0x{:02x}  UNEXPECTED -- nothing in §2 claims this address", device.address),
            }
        }
        println!("      => {}", i2c_scan::verdict(&found));

        FreeRtos::delay_ms(SCAN_PERIOD_S * 1000);
    }
}
