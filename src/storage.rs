//! Non-volatile storage — thin wrappers over ESP-IDF's `nvs_*` API.
//!
//! Carried over from the moisture project, minus what this design does not
//! need. Keeping the FFI in one module means the `unsafe`, the C-string
//! handling and the error conversion are written once and reviewed once.
//!
//! Used directly rather than through `esp-idf-svc`: that crate links the radio
//! stacks behind it, and §5's WiFi is deferred.

use core::ffi::c_char;

use esp_idf_hal::sys::{self, esp_err_t, nvs_handle_t, EspError, ESP_OK};

/// Initialise NVS if it has not been already. Idempotent.
pub fn ensure_init() -> Result<(), EspError> {
    unsafe {
        let mut err = sys::nvs_flash_init();
        if err == sys::ESP_ERR_NVS_NO_FREE_PAGES || err == sys::ESP_ERR_NVS_NEW_VERSION_FOUND {
            check(sys::nvs_flash_erase())?;
            err = sys::nvs_flash_init();
        }
        check(err)
    }
}

/// An open NVS namespace. Closes itself on drop.
pub struct Namespace {
    handle: nvs_handle_t,
}

impl Namespace {
    /// `name` must be NUL-terminated — it goes straight to a C API.
    pub fn open(name: &[u8]) -> Result<Self, EspError> {
        unsafe {
            ensure_init()?;
            let mut handle: nvs_handle_t = 0;
            check(sys::nvs_open(
                name.as_ptr() as *const c_char,
                sys::nvs_open_mode_t_NVS_READWRITE,
                &mut handle,
            ))?;
            Ok(Self { handle })
        }
    }

    /// Read a `u8`, or `None` if the key is absent.
    ///
    /// Absent is deliberately not an error: "nobody has ever set this" is an
    /// ordinary first-boot state, and the caller wants a default, not a failure.
    pub fn get_u8(&self, key: &[u8]) -> Option<u8> {
        let mut value = 0u8;
        let err =
            unsafe { sys::nvs_get_u8(self.handle, key.as_ptr() as *const c_char, &mut value) };
        (err == ESP_OK).then_some(value)
    }

    pub fn set_u8(&self, key: &[u8], value: u8) -> Result<(), EspError> {
        check(unsafe { sys::nvs_set_u8(self.handle, key.as_ptr() as *const c_char, value) })
    }

    pub fn get_u16(&self, key: &[u8]) -> Option<u16> {
        let mut value = 0u16;
        let err =
            unsafe { sys::nvs_get_u16(self.handle, key.as_ptr() as *const c_char, &mut value) };
        (err == ESP_OK).then_some(value)
    }

    pub fn set_u16(&self, key: &[u8], value: u16) -> Result<(), EspError> {
        check(unsafe { sys::nvs_set_u16(self.handle, key.as_ptr() as *const c_char, value) })
    }

    /// Make pending writes durable. Nothing is visible to a later boot until
    /// this returns.
    pub fn commit(&self) -> Result<(), EspError> {
        check(unsafe { sys::nvs_commit(self.handle) })
    }
}

impl Drop for Namespace {
    fn drop(&mut self) {
        unsafe { sys::nvs_close(self.handle) };
    }
}

/// Turn an `esp_err_t` into a `Result`.
pub fn check(err: esp_err_t) -> Result<(), EspError> {
    match core::num::NonZeroI32::new(err) {
        None => Ok(()),
        Some(e) => Err(EspError::from_non_zero(e)),
    }
}
