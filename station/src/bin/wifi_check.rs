//! The station's first bench check: does the FireBeetle's ESP32-C6 work as its
//! radio, and does the board keep running on a bare Li-ion cell?
//!
//! It brings Wi-Fi up through the C6 (esp_hosted over SDIO, the esp_wifi API on
//! top through esp_wifi_remote) and scans every 20 seconds, logging the
//! strongest networks it hears. Nothing connects to anything: a scan is enough to
//! prove the link to the C6 works and the radio transmits (an active scan sends
//! probe requests), and it gives a signal strength per network to compare
//! antennas with.
//!
//! **It remembers each run in NVS**, because the interesting run is the one with
//! no cable: unplug USB, power the board from a cell, come back later. Plugging
//! USB back in restarts the board, and that boot prints what the previous run
//! saw before starting a new one:
//! - how long it ran;
//! - how many scans worked;
//! - the strongest signal;
//! - every reset since the first boot, by cause. A brownout counted here means
//!   the cell could not carry the radio.
//!
//! **If the C6 answers nothing** (`sdmmc_card_init failed`, error 0x107), its
//! image is the likely culprit: this board's C6 shipped with one it could not
//! boot, as other DFRobot P4 boards have. `c6_reflash/` rewrites it from the P4,
//! over the same SDIO lines.
//!
//! **Never connect the cell and USB at the same time.** The FireBeetle wires VIN
//! straight to the USB-C's VBUS, with no diode (DFRobot's schematic V1.0), so a
//! cell on VIN while USB is plugged in has the PC pushing 5 V into the cell.

use std::thread;
use std::time::Duration;

use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::hal::peripherals::Peripherals;
use esp_idf_svc::nvs::{EspDefaultNvsPartition, EspNvs, NvsDefault};
use esp_idf_svc::sys;
use esp_idf_svc::sys::EspError;
use esp_idf_svc::wifi::{AccessPointInfo, BlockingWifi, ClientConfiguration, Configuration, EspWifi};

/// Seconds between scans. A scan through the C6 takes a few seconds of its own.
const SCAN_PERIOD: Duration = Duration::from_secs(20);

/// How many of the strongest networks each scan logs.
const STRONGEST_SHOWN: usize = 6;

/// The NVS namespace this check keeps its record in (at most 15 characters, as
/// every NVS name is).
const NAMESPACE: &str = "wifi_check";

/// Why the chip last restarted, sorted into the counters the record keeps.
///
/// ESP-IDF reports the reason as a plain integer (`esp_reset_reason_t`); naming
/// the cases here keeps the counting below readable.
enum Cause {
    PowerOn,
    Usb,
    Brownout,
    Panic,
    Watchdog,
    Software,
    Other,
}

impl Cause {
    /// The cause of the boot that is running now.
    fn of_this_boot() -> Cause {
        // `esp_reset_reason` is a plain C function with no preconditions; the
        // `unsafe` is only because every foreign call is.
        let reason = unsafe { sys::esp_reset_reason() };
        // The reasons are constants generated from the C enum. A `match` on a
        // constant compares by value, so each arm reads like the C `case`.
        match reason {
            sys::esp_reset_reason_t_ESP_RST_POWERON => Cause::PowerOn,
            // Opening the USB serial/JTAG port resets the chip: a reconnect.
            sys::esp_reset_reason_t_ESP_RST_USB => Cause::Usb,
            sys::esp_reset_reason_t_ESP_RST_BROWNOUT => Cause::Brownout,
            sys::esp_reset_reason_t_ESP_RST_PANIC => Cause::Panic,
            sys::esp_reset_reason_t_ESP_RST_INT_WDT
            | sys::esp_reset_reason_t_ESP_RST_TASK_WDT
            | sys::esp_reset_reason_t_ESP_RST_WDT => Cause::Watchdog,
            // esp_hosted restarts the host when the C6 stops answering: a run of
            // these means the link to the C6 is failing.
            sys::esp_reset_reason_t_ESP_RST_SW => Cause::Software,
            _ => Cause::Other,
        }
    }

    /// The NVS key of the counter this cause adds to.
    fn counter_key(&self) -> &'static str {
        match self {
            Cause::PowerOn => "power_ons",
            Cause::Usb => "usb",
            Cause::Brownout => "brownouts",
            Cause::Panic => "panics",
            Cause::Watchdog => "watchdogs",
            Cause::Software => "software",
            Cause::Other => "other",
        }
    }
}

/// Every counter the record keeps across boots, in the order they are printed.
const COUNTERS: [&str; 8] = ["boots", "power_ons", "usb", "brownouts", "panics", "watchdogs", "software", "other"];

/// The record of one run: overwritten as the run goes on, printed by the next boot.
struct Run {
    /// Seconds since this boot, at the last save.
    up_seconds: u32,
    scans_ok: u32,
    scans_failed: u32,
    /// How many networks the last good scan heard.
    networks: u32,
    /// The strongest signal any scan of this run heard, in dBm; 0 until one does.
    strongest: i32,
}

impl Run {
    fn new() -> Run {
        Run { up_seconds: 0, scans_ok: 0, scans_failed: 0, networks: 0, strongest: 0 }
    }

    /// The previous run's record, as it stood at its last save. A missing key
    /// (the very first boot) reads as zero.
    fn load(nvs: &EspNvs<NvsDefault>) -> Result<Run, EspError> {
        Ok(Run {
            up_seconds: nvs.get_u32("up_s")?.unwrap_or(0),
            scans_ok: nvs.get_u32("scans_ok")?.unwrap_or(0),
            scans_failed: nvs.get_u32("scans_bad")?.unwrap_or(0),
            networks: nvs.get_u32("networks")?.unwrap_or(0),
            strongest: nvs.get_i32("strongest")?.unwrap_or(0),
        })
    }

    fn save(&self, nvs: &mut EspNvs<NvsDefault>) -> Result<(), EspError> {
        nvs.set_u32("up_s", self.up_seconds)?;
        nvs.set_u32("scans_ok", self.scans_ok)?;
        nvs.set_u32("scans_bad", self.scans_failed)?;
        nvs.set_u32("networks", self.networks)?;
        nvs.set_i32("strongest", self.strongest)?;
        Ok(())
    }
}

/// Seconds since this boot, from ESP-IDF's microsecond timer.
fn uptime_seconds() -> u32 {
    let microseconds = unsafe { sys::esp_timer_get_time() };
    (microseconds / 1_000_000) as u32
}

/// Adds one to a counter in NVS. A counter never seen before starts at zero.
fn count(nvs: &mut EspNvs<NvsDefault>, key: &str) -> Result<(), EspError> {
    let before = nvs.get_u32(key)?.unwrap_or(0);
    nvs.set_u32(key, before + 1)
}

/// Prints what the previous run saw, and every reset since the first boot.
fn report_previous_run(nvs: &EspNvs<NvsDefault>) -> Result<(), EspError> {
    let previous = Run::load(nvs)?;
    log::info!(
        "previous run: up {} s, scans {} good / {} failed, last scan {} networks, strongest {} dBm",
        previous.up_seconds,
        previous.scans_ok,
        previous.scans_failed,
        previous.networks,
        previous.strongest
    );
    let mut line = String::from("since the first boot:");
    for key in COUNTERS {
        let value = nvs.get_u32(key)?.unwrap_or(0);
        line.push_str(&format!(" {key} {value}"));
    }
    log::info!("{line}");
    Ok(())
}

/// Logs the strongest networks of one scan, and returns the strongest signal.
fn log_scan(found: &mut Vec<AccessPointInfo>) -> i32 {
    // Strongest first. `signal_strength` is in dBm, so a larger number (closer to
    // zero) is a stronger signal; comparing `b` to `a` sorts it descending.
    found.sort_by(|a, b| b.signal_strength.cmp(&a.signal_strength));
    log::info!("scan: {} networks", found.len());
    for network in found.iter().take(STRONGEST_SHOWN) {
        log::info!("  {:4} dBm  ch {:2}  {}", network.signal_strength, network.channel, network.ssid);
    }
    match found.first() {
        Some(strongest) => strongest.signal_strength as i32,
        None => 0,
    }
}

fn main() -> Result<(), EspError> {
    // Keeps ESP-IDF's patches for the Rust standard library in the link; without
    // this call some of them are silently dropped.
    sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();

    // `take` hands out each singleton once; a second call would return an error.
    let peripherals = Peripherals::take()?;
    let event_loop = EspSystemEventLoop::take()?;
    let nvs_partition = EspDefaultNvsPartition::take()?;
    // `.clone()` on the partition is cheap: a shared handle, not a copy of the
    // flash. Wi-Fi keeps its calibration in the same partition.
    let mut nvs = EspNvs::new(nvs_partition.clone(), NAMESPACE, true)?;

    let cause = Cause::of_this_boot();
    report_previous_run(&nvs)?;
    count(&mut nvs, "boots")?;
    count(&mut nvs, cause.counter_key())?;
    let mut run = Run::new();
    run.save(&mut nvs)?;
    log::info!("this boot: {}", cause.counter_key());

    // Wi-Fi through the C6. `EspWifi::new` brings esp_hosted's SDIO link up and
    // asks the C6 for its radio; a C6 without esp_hosted's firmware fails here,
    // or esp_hosted restarts the board (counted as `software` above).
    let driver = EspWifi::new(peripherals.modem, event_loop.clone(), Some(nvs_partition))?;
    // `BlockingWifi` waits for each step's event, so the calls below return when
    // the C6 has done what was asked.
    let mut wifi = BlockingWifi::wrap(driver, event_loop)?;
    // Station mode with no network configured: enough to scan.
    wifi.set_configuration(&Configuration::Client(ClientConfiguration::default()))?;
    wifi.start()?;
    log::info!("wifi: started through the C6");

    loop {
        match wifi.scan() {
            Ok(mut found) => {
                let strongest = log_scan(&mut found);
                run.scans_ok += 1;
                run.networks = found.len() as u32;
                // The strongest of the run: keep the better of the two. A run that
                // has heard nothing yet still holds 0.
                if run.strongest == 0 || strongest > run.strongest {
                    run.strongest = strongest;
                }
            }
            Err(error) => {
                run.scans_failed += 1;
                log::warn!("scan failed: {error}");
            }
        }
        run.up_seconds = uptime_seconds();
        // A failed save is worth a line but not a stop: the scans go on.
        if let Err(error) = run.save(&mut nvs) {
            log::warn!("record not saved: {error}");
        }
        thread::sleep(SCAN_PERIOD);
    }
}
