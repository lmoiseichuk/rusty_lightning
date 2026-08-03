//! What the device can say about itself: clock, die temperature, free heap.
//!
//! Everything here is about the *computer* rather than about the weather, which
//! is why it sits in one module and occupies one line of the screen. In a
//! device that is meant to run unattended for weeks, that line is the only way
//! to ask whether the machine underneath the readings is still healthy.

use esp_idf_hal::sys;

/// A snapshot of device health.
#[derive(Debug, Clone, Copy)]
pub struct Health {
    pub cpu_mhz: u32,
    /// Die temperature in tenths of a degree Celsius.
    ///
    /// Tenths rather than a float: `riscv32imc` has no FPU, and one decimal is
    /// all the sensor's accuracy justifies anyway (±2 °C typical).
    ///
    /// **This is the die, not the room.** It reads well above ambient because
    /// it is measuring the chip that is doing the work — useful as a trend and
    /// as a fault signal, misleading as a thermometer.
    pub die_temp_tenths: Option<i32>,
    pub free_heap_kb: u32,
}

/// The CPU frequency the chip is actually running at.
///
/// Read from `ets_get_cpu_frequency` rather than assumed from configuration.
/// With `CONFIG_PM_ENABLE` the frequency moves at runtime, so what the build
/// asked for and what the core is doing are different questions.
pub fn cpu_mhz() -> u32 {
    unsafe { sys::ets_get_cpu_frequency() }
}

pub fn free_heap_kb() -> u32 {
    (unsafe { sys::esp_get_free_heap_size() }) / 1024
}

/// The on-die temperature sensor.
///
/// Held open rather than installed per read: `temperature_sensor_install`
/// allocates a handle and enabling it takes time to settle, so doing that on
/// every screen refresh would pay the cost repeatedly for a value that moves
/// slowly.
pub struct DieTemperature {
    handle: sys::temperature_sensor_handle_t,
}

impl DieTemperature {
    pub fn new() -> Option<Self> {
        let config = sys::temperature_sensor_config_t {
            // The C3's sensor trades range against resolution; this span covers
            // anything an indoor or sheltered outdoor device will see, at the
            // best accuracy on offer for it.
            range_min: -10,
            range_max: 80,
            clk_src: sys::soc_periph_temperature_sensor_clk_src_t_TEMPERATURE_SENSOR_CLK_SRC_DEFAULT,
        };
        let mut handle: sys::temperature_sensor_handle_t = core::ptr::null_mut();
        // SAFETY: plain IDF calls; the handle is owned by this struct from here.
        unsafe {
            if sys::temperature_sensor_install(&config, &mut handle) != sys::ESP_OK {
                return None;
            }
            if sys::temperature_sensor_enable(handle) != sys::ESP_OK {
                sys::temperature_sensor_uninstall(handle);
                return None;
            }
        }
        Some(Self { handle })
    }

    /// Read the die temperature, in tenths of a degree.
    pub fn read_tenths(&self) -> Option<i32> {
        let mut celsius: f32 = 0.0;
        // SAFETY: `handle` was installed and enabled in `new`.
        let err = unsafe { sys::temperature_sensor_get_celsius(self.handle, &mut celsius) };
        if err != sys::ESP_OK {
            return None;
        }
        // The IDF API is float-only, so one conversion is unavoidable. It
        // happens here, once per reading, and everything downstream is integer.
        Some((celsius * 10.0) as i32)
    }
}

impl Drop for DieTemperature {
    fn drop(&mut self) {
        // SAFETY: disabling before uninstalling is the documented order.
        unsafe {
            sys::temperature_sensor_disable(self.handle);
            sys::temperature_sensor_uninstall(self.handle);
        }
    }
}

/// Collect everything at once.
pub fn health(temperature: Option<&DieTemperature>) -> Health {
    Health {
        cpu_mhz: cpu_mhz(),
        die_temp_tenths: temperature.and_then(|t| t.read_tenths()),
        free_heap_kb: free_heap_kb(),
    }
}


/// Why the device last restarted, as a name.
///
/// **Worth printing on every boot**, because on this chip an unexpected reset
/// is usually not a firmware fault and the reason says so immediately.
///
/// `ESP_RST_USB` in particular: the C3's USB-Serial/JTAG maps the host's CDC
/// control lines onto the reset and boot straps, so a host that asserts DTR/RTS
/// — which Linux's ModemManager does to *every* new serial device it probes —
/// reboots the board simply by having a cable plugged into it. That presents as
/// "plugging in USB reboots the device", which looks like a power fault and is
/// not one. The moisture project lost time to the same thing.
pub fn reset_reason_name() -> &'static str {
    // bindgen flattens C enums to `<enum>_<VARIANT>` constants, which is why
    // these names are as long as they are.
    match unsafe { sys::esp_reset_reason() } {
        sys::esp_reset_reason_t_ESP_RST_POWERON => "power-on",
        sys::esp_reset_reason_t_ESP_RST_SW => "software restart",
        sys::esp_reset_reason_t_ESP_RST_PANIC => "PANIC",
        sys::esp_reset_reason_t_ESP_RST_INT_WDT => "interrupt watchdog",
        sys::esp_reset_reason_t_ESP_RST_TASK_WDT => "TASK WATCHDOG",
        sys::esp_reset_reason_t_ESP_RST_WDT => "other watchdog",
        sys::esp_reset_reason_t_ESP_RST_DEEPSLEEP => "deep-sleep wake",
        sys::esp_reset_reason_t_ESP_RST_BROWNOUT => "BROWNOUT",
        sys::esp_reset_reason_t_ESP_RST_USB => "USB (host asserted DTR/RTS -- not a fault)",
        sys::esp_reset_reason_t_ESP_RST_JTAG => "JTAG",
        _ => "unknown",
    }
}
