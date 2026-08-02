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
