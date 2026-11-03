//! Leaves the on-board ESP32-C6 alone, so it can be reflashed through its pads.
//!
//! The P4 drives the C6's EN pin from GPIO54 (DFRobot's schematic V1.0), and
//! esp_hosted holds it as an output to reset the C6. This firmware only makes
//! GPIO54 an input, with no pull either way. The board's 10 kΩ pull-up then keeps
//! the C6 enabled, and the C6_RST and C6_IO9 test pads alone decide when it
//! resets and whether it boots into download mode.
//!
//! Run it while the C6 is reflashed over the C6_TX0 and C6_RX0 pads with a
//! USB-to-TTL adapter; then flash `wifi_check` back.

use std::thread;
use std::time::Duration;

use esp_idf_svc::hal::gpio::{PinDriver, Pull};
use esp_idf_svc::hal::peripherals::Peripherals;
use esp_idf_svc::sys::EspError;

fn main() -> Result<(), EspError> {
    // Keeps ESP-IDF's patches for the Rust standard library in the link.
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();

    let peripherals = Peripherals::take()?;
    // An input with no pull: the board's own pull-up to 3.3 V decides the level.
    // The driver has to stay alive (bound to a name, not dropped) for the pin to
    // stay configured this way.
    let c6_enable = PinDriver::input(peripherals.pins.gpio54, Pull::Floating)?;

    loop {
        // What the C6's EN pin reads: high while the C6 runs, low while a
        // C6_RST press holds it in reset.
        let level = match c6_enable.is_high() {
            true => "high: the C6 runs",
            false => "low: the C6 is held in reset",
        };
        log::info!("idle: GPIO54 released; C6 EN is {level}");
        thread::sleep(Duration::from_secs(5));
    }
}
