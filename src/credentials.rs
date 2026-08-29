//! The access point's name and password.
//!
//! Both live in NVS so they survive a reflash and a filesystem format, and both
//! are editable from the web UI the access point serves. A device that has
//! never had them set generates a password to *show*, and does not store it —
//! see [`Credentials::load`].

use crate::storage::Namespace;

const NAMESPACE: &[u8] = b"portal\0";
const KEY_SSID: &[u8] = b"ssid\0";
const KEY_PASSWORD: &[u8] = b"password\0";

/// WPA2 will not accept anything shorter, and the standard says so.
pub const PASSWORD_MIN: usize = 8;

/// The IDF's own field width, less the NUL.
pub const PASSWORD_MAX: usize = 63;
pub const SSID_MAX: usize = 31;

/// What the access point announces, and what gets somebody onto it.
#[derive(Clone, Debug)]
pub struct Credentials {
    pub ssid: String,
    pub password: String,
    /// True when the password was made up during this boot rather than read
    /// back from NVS. The join screen says so, and nothing writes it until
    /// somebody presses the button on the settings page.
    pub generated: bool,
}

impl Credentials {
    /// Read the stored pair, inventing a password if there is none.
    ///
    /// **A generated password is not saved.** Saving it here would be the
    /// obvious thing and it is wrong twice over: it writes NVS on a path that
    /// is only meant to read, and it silently commits a secret the owner has
    /// not seen yet, so a device that raised its portal once and was never
    /// looked at would carry a password nobody knows for the rest of its life.
    /// It is shown on the panel and written only when the settings page asks.
    ///
    /// The same argument does not apply to the SSID, which is derived rather
    /// than random: it is the same on every boot, so there is nothing to lose
    /// by not storing it.
    pub fn load() -> Credentials {
        let nvs = Namespace::open(NAMESPACE).ok();

        let stored_ssid = nvs.as_ref().and_then(|n| n.get_string(KEY_SSID));
        let stored_password = nvs.as_ref().and_then(|n| n.get_string(KEY_PASSWORD));

        let ssid = match stored_ssid {
            Some(name) if !name.is_empty() => name,
            _ => default_ssid(&machine_id()),
        };

        match stored_password {
            Some(password) if password.len() >= PASSWORD_MIN => {
                Credentials { ssid, password, generated: false }
            }
            _ => Credentials { ssid, password: generate_password(), generated: true },
        }
    }

    /// Store both. Called only from the settings page.
    pub fn save(&self) -> Result<(), esp_idf_hal::sys::EspError> {
        let nvs = Namespace::open(NAMESPACE)?;
        nvs.set_string(KEY_SSID, &self.ssid)?;
        nvs.set_string(KEY_PASSWORD, &self.password)?;
        nvs.commit()
    }

    /// Whether these could actually be used, and why not if they could not.
    ///
    /// Checked before saving, because a rejected pair stored is a device that
    /// raises an access point nobody can join and that cannot be fixed without
    /// the console.
    pub fn check(ssid: &str, password: &str) -> Result<(), &'static str> {
        if ssid.is_empty() {
            return Err("the network name cannot be empty");
        }
        if ssid.len() > SSID_MAX {
            return Err("the network name is too long (31 characters at most)");
        }
        if password.len() < PASSWORD_MIN {
            return Err("the password must be at least 8 characters -- WPA2 requires it");
        }
        if password.len() > PASSWORD_MAX {
            return Err("the password is too long (63 characters at most)");
        }
        if !password.is_ascii() || !ssid.is_ascii() {
            return Err("use ASCII only -- phones disagree about the rest");
        }
        Ok(())
    }
}

/// The last three bytes of the factory MAC, as hex.
///
/// **From eFuse, not from the WiFi driver.** `esp_read_mac` needs the netif
/// stack up; `esp_efuse_mac_get_default` reads the burned-in value directly, so
/// this works before the radio exists — which is exactly when the SSID is
/// needed, since the SSID is an argument to bringing the radio up.
pub fn machine_id() -> String {
    let mut mac = [0u8; 6];
    let err = unsafe { esp_idf_hal::sys::esp_efuse_mac_get_default(mac.as_mut_ptr()) };
    if err != esp_idf_hal::sys::ESP_OK {
        // Every board has a MAC, so this should not happen -- but a fixed
        // string is a working access point with a dull name, where a panic
        // would be no access point at all.
        return "unknown".to_string();
    }
    format!("{:02X}{:02X}{:02X}", mac[3], mac[4], mac[5])
}

/// `lightning-XXXXXX` from the machine id.
///
/// Derived rather than fixed so two of these on one bench do not collide, and
/// stable so the QR code on the panel keeps working across reboots — a name
/// that changed every boot would mean a printed code that stops working.
fn default_ssid(machine_id: &str) -> String {
    format!("lightning-{machine_id}")
}

/// Eight characters from an unambiguous alphabet.
///
/// **Eight because WPA2 will not take seven**, not because eight is strong.
/// This protects a device on a bench for sixty seconds at a time, and the
/// threat it is really defending against is a neighbour joining by accident.
/// Against that, the length that matters is the one somebody will actually type
/// off a screen — and every character that could be misread costs more than it
/// buys, so `0`/`O` and `1`/`l`/`I` are not in the alphabet.
fn generate_password() -> String {
    // No `O`, `0`, `I`, `l`, `1`.
    const ALPHABET: &[u8] = b"abcdefghijkmnopqrstuvwxyz23456789";
    let mut out = String::with_capacity(PASSWORD_MIN);
    for _ in 0..PASSWORD_MIN {
        // `esp_random` is the hardware RNG. It is only guaranteed to deserve
        // the name once the radio is up -- before that it is a PRNG seeded the
        // same way every boot. The portal starts the WiFi driver before this is
        // called, which is exactly the condition the documentation asks for.
        let index = unsafe { esp_idf_hal::sys::esp_random() } as usize % ALPHABET.len();
        out.push(ALPHABET[index] as char);
    }
    out
}
