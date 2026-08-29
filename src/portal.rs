//! The access point and the web server it carries.
//!
//! Raised by a long press on BOOT, dropped by another one or by its own window
//! expiring. While it is up the panel shows two QR codes — one to join the
//! network, one to open the page — and the page carries every setting, every
//! statistic and the strike log.
//!
//! ## Why the handlers do not touch the device
//!
//! An HTTP handler runs on the server's own task. The console's command table
//! is written for a single caller and keeps its state in statics; the sensor is
//! behind an I2C bus with no lock; the strike log holds a file handle and a
//! pending buffer. Reaching into any of those from another task is a data race
//! that would show up as corruption weeks later, in a log, with no way back to
//! this decision.
//!
//! So handlers do two things and no more: **read a snapshot** the main loop
//! publishes, and **queue a console line** the main loop runs. Everything the
//! page can do, the console can already do — which is not a coincidence, it is
//! the reason the page needs no new vocabulary and no second implementation of
//! anything.

use esp_idf_hal::sys::{self, EspError};
use std::sync::Mutex;

use crate::credentials::Credentials;

/// How long the access point stays up with nobody on it.
///
/// **Re-armed while a station is associated**, so reading a long page or
/// downloading the log does not drop the network underneath it. Sixty seconds
/// is long enough to find the QR, point a phone at it and accept the "no
/// internet" prompt, and short enough that a press nobody follows up on costs
/// almost nothing — the radio is the largest current draw on the board.
pub const WINDOW_S: u32 = 60;

/// The address the softAP hands out, and the one both QR codes point at.
pub const ADDRESS: &str = "192.168.4.1";

/// What the page renders. Published by the main loop, read by the server task.
///
/// A snapshot rather than live references: everything here is `Copy` or owned,
/// so the server never holds a borrow into anything the main loop is mutating.
#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub uptime_s: u32,
    pub epoch: u64,
    pub time_local: String,
    pub location: &'static str,
    pub defence_raw: u16,
    pub defence_percent: u32,
    pub noise_per_min: u32,
    pub disturbers_per_min: u32,
    pub quiet_per_min: u32,
    pub strikes_total: u32,
    pub strikes_hour: u32,
    pub strikes_day: u32,
    pub nearest_km: Option<u8>,
    pub last_strike: Option<String>,
    pub battery_mv: u32,
    pub battery_percent: u32,
    pub free_ram_kb: u32,
    pub log_records: u32,
    pub log_bytes: u32,
    pub frozen: bool,
    pub tz_minutes: i32,
    pub recent: Vec<RecentRow>,
    pub credentials: Option<(String, String, bool)>,
}

/// One line of the strike table on the page.
#[derive(Clone, Debug)]
pub struct RecentRow {
    pub when: String,
    pub distance: String,
    pub energy: u32,
    pub score: u32,
    pub strokes: u32,
}

static SNAPSHOT: Mutex<Option<Snapshot>> = Mutex::new(None);
static QUEUE: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Publish what the page should show. Called by the main loop.
pub fn publish(snapshot: Snapshot) {
    if let Ok(mut held) = SNAPSHOT.lock() {
        *held = Some(snapshot);
    }
}

/// Take everything the page has asked for since the last call.
///
/// Returned rather than executed here, because running a console line needs the
/// sensor, the bus and the log — all of which belong to the main loop.
pub fn drain() -> Vec<String> {
    match QUEUE.lock() {
        Ok(mut queue) => core::mem::take(&mut *queue),
        Err(_) => Vec::new(),
    }
}

fn enqueue(line: String) {
    if let Ok(mut queue) = QUEUE.lock() {
        // A page left open with a stuck reload must not grow this without
        // bound. Sixty-four is far more than a person can generate and small
        // enough to be free.
        if queue.len() < 64 {
            queue.push(line);
        }
    }
}

fn snapshot() -> Snapshot {
    SNAPSHOT.lock().ok().and_then(|held| held.clone()).unwrap_or_default()
}

/// A running access point: the radio, the server, and when it should stop.
pub struct Portal {
    server: sys::httpd_handle_t,
    /// When the window last had a reason to stay open.
    armed_ms: u32,
    pub credentials: Credentials,
}

impl Portal {
    /// Bring up the softAP and the web server.
    pub fn raise(credentials: Credentials, now_ms: u32) -> Result<Portal, EspError> {
        unsafe {
            // The netif and event loop are process-wide and may only be set up
            // once. Both return ESP_ERR_INVALID_STATE when they already exist,
            // which is a success for our purposes -- a second portal in one
            // boot is the ordinary case, not an error.
            let err = sys::esp_netif_init();
            if err != sys::ESP_OK && err != sys::ESP_ERR_INVALID_STATE {
                return Err(EspError::from(err).unwrap());
            }
            let err = sys::esp_event_loop_create_default();
            if err != sys::ESP_OK && err != sys::ESP_ERR_INVALID_STATE {
                return Err(EspError::from(err).unwrap());
            }

            // Creating the default AP netif twice aborts inside the IDF rather
            // than returning an error, so it is done once and leaked
            // deliberately. There is exactly one AP on this device for the life
            // of the process; freeing it would buy nothing and the IDF's own
            // examples do the same.
            static mut AP_NETIF: *mut sys::esp_netif_t = core::ptr::null_mut();
            if AP_NETIF.is_null() {
                AP_NETIF = sys::esp_netif_create_default_wifi_ap();
            }

            let config = sys::wifi_init_config_t {
                ..default_wifi_init_config()
            };
            let err = sys::esp_wifi_init(&config);
            if err != sys::ESP_OK && err != sys::ESP_ERR_WIFI_INIT_STATE {
                return Err(EspError::from(err).unwrap());
            }

            check(sys::esp_wifi_set_storage(sys::wifi_storage_t_WIFI_STORAGE_RAM))?;
            check(sys::esp_wifi_set_mode(sys::wifi_mode_t_WIFI_MODE_AP))?;

            let mut ap: sys::wifi_ap_config_t = core::mem::zeroed();
            let ssid = credentials.ssid.as_bytes();
            let password = credentials.password.as_bytes();
            ap.ssid[..ssid.len()].copy_from_slice(ssid);
            ap.ssid_len = ssid.len() as u8;
            ap.password[..password.len()].copy_from_slice(password);
            ap.channel = 1;
            ap.authmode = sys::wifi_auth_mode_t_WIFI_AUTH_WPA2_PSK;
            // Four is plenty for a device one person walks up to, and each
            // association costs RAM the strike path would rather keep.
            ap.max_connection = 4;
            ap.beacon_interval = 100;

            let mut config = sys::wifi_config_t { ap };
            check(sys::esp_wifi_set_config(
                sys::wifi_interface_t_WIFI_IF_AP,
                &mut config,
            ))?;
            check(sys::esp_wifi_start())?;

            // **The radio is the reason light sleep has to go.** Light sleep
            // powers down the modem between beacons; an associated station
            // then loses the connection mid-request. The caller pins the awake
            // policy for the life of the portal.
            check(sys::esp_wifi_set_ps(sys::wifi_ps_type_t_WIFI_PS_NONE))?;

            // The radio is up but the server is not; report the heap either
            // way, because `ESP_ERR_HTTPD_TASK` means "could not create the
            // task" and the only cause worth checking is memory.
            let free_before = sys::esp_get_free_heap_size();
            let server = match start_server() {
                Ok(server) => server,
                Err(e) => {
                    println!("ap:   httpd would not start -- {free_before} B heap free after WiFi");
                    sys::esp_wifi_stop();
                    return Err(e);
                }
            };
            println!("ap:   {free_before} B heap free after WiFi, server up");

            Ok(Portal { server, armed_ms: now_ms, credentials })
        }
    }

    /// Whether the window has run out with nobody connected.
    ///
    /// Re-arms whenever a station is associated, so the countdown only runs
    /// against an access point nobody is using.
    pub fn expired(&mut self, now_ms: u32) -> bool {
        if self.stations() > 0 {
            self.armed_ms = now_ms;
            return false;
        }
        crate::uptime::due(now_ms, self.armed_ms, WINDOW_S * 1000)
    }

    /// Seconds left before the window closes, for the panel's countdown.
    pub fn remaining_s(&self, now_ms: u32) -> u32 {
        let gone = crate::uptime::since(now_ms, self.armed_ms) / 1000;
        WINDOW_S.saturating_sub(gone)
    }

    /// How many devices are associated right now.
    pub fn stations(&self) -> u32 {
        unsafe {
            let mut list: sys::wifi_sta_list_t = core::mem::zeroed();
            if sys::esp_wifi_ap_get_sta_list(&mut list) == sys::ESP_OK {
                list.num as u32
            } else {
                0
            }
        }
    }

    /// Stop the server and the radio.
    ///
    /// **The server first.** Stopping the radio underneath a request in flight
    /// leaves the handler writing to a socket whose interface has gone.
    pub fn lower(self) {
        unsafe {
            if !self.server.is_null() {
                sys::httpd_stop(self.server);
            }
            sys::esp_wifi_stop();
        }
    }
}

fn check(err: sys::esp_err_t) -> Result<(), EspError> {
    match EspError::from(err) {
        Some(e) if err != sys::ESP_OK => Err(e),
        _ => Ok(()),
    }
}

/// `WIFI_FEATURE_CAPS`, which is a compile-time constant rather than a symbol.
///
/// **Not a variable to link against**, which is the trap: the field it fills is
/// named after `g_wifi_feature_caps`, a global that exists in some IDF versions
/// and not in 5.3.4, so declaring it `extern` compiles and then fails at link
/// with an undefined reference.
///
/// The IDF builds this from the `CONFIG_ESP_WIFI_*` options. Read out of this
/// build's own `sdkconfig.h`: WPA3-SAE, GCMP, GMAC and enterprise are on, FTM
/// is supported by the part, and the cache-TX-buffer bit follows
/// `cache_tx_buf_num`, which is zero here. Under-setting a bit only turns off a
/// feature this device does not use — the access point is WPA2-PSK — so the
/// conservative reading is the safe one.
const WIFI_FEATURE_CAPS: u64 = (1 << 0)   // WPA3-SAE
    | (1 << 2)                            // FTM initiator
    | (1 << 3)                            // FTM responder
    | (1 << 4)                            // GCMP
    | (1 << 5)                            // GMAC
    | (1 << 7); // enterprise

/// The IDF's `WIFI_INIT_CONFIG_DEFAULT`, which is a C macro and so has no
/// binding. Filled out by hand from `esp_wifi.h`; the fields that matter are
/// the buffer counts, and these are the defaults for a device that is an access
/// point only.
fn default_wifi_init_config() -> sys::wifi_init_config_t {
    sys::wifi_init_config_t {
        osi_funcs: unsafe { core::ptr::addr_of_mut!(sys::g_wifi_osi_funcs) },
        wpa_crypto_funcs: unsafe { sys::g_wifi_default_wpa_crypto_funcs },
        static_rx_buf_num: 10,
        dynamic_rx_buf_num: 32,
        tx_buf_type: 1,
        static_tx_buf_num: 0,
        dynamic_tx_buf_num: 32,
        rx_mgmt_buf_type: 0,
        rx_mgmt_buf_num: 0,
        cache_tx_buf_num: 0,
        csi_enable: 0,
        ampdu_rx_enable: 1,
        ampdu_tx_enable: 1,
        amsdu_tx_enable: 0,
        nvs_enable: 0,
        nano_enable: 0,
        rx_ba_win: 6,
        wifi_task_core_id: 0,
        beacon_max_len: 752,
        mgmt_sbuf_num: 32,
        feature_caps: WIFI_FEATURE_CAPS,
        sta_disconnected_pm: false,
        espnow_max_encrypt_num: 7,
        tx_hetb_queue_num: 0,
        dump_hesigb_enable: false,
        magic: 0x1F2F3F4F,
    }
}

// --- the server -------------------------------------------------------------

/// Start httpd and register every route.
///
/// The routes are deliberately few. Anything that changes the device goes
/// through `/do`, which queues a console line — so the page never grows a
/// second way to do something the console already does.
fn start_server() -> Result<sys::httpd_handle_t, EspError> {
    unsafe {
        let mut config: sys::httpd_config_t = default_httpd_config();
        // The catch-all that makes the captive-portal prompt appear needs
        // wildcard matching, which is off by default.
        config.uri_match_fn = Some(sys::httpd_uri_match_wildcard);
        // Handlers are few but the wildcard one is last, so the table has to be
        // big enough to hold all of them.
        config.max_uri_handlers = 8;
        // The log download streams, but the page itself is built in RAM; give
        // the server task room for it.
        config.stack_size = 8192;

        let mut server: sys::httpd_handle_t = core::ptr::null_mut();
        check(sys::httpd_start(&mut server, &config))?;

        register(server, c"/", sys::http_method_HTTP_GET, Some(handle_page));
        register(server, c"/do", sys::http_method_HTTP_GET, Some(handle_do));
        register(server, c"/log.csv", sys::http_method_HTTP_GET, Some(handle_log));
        // **Last, and wildcard.** Every captive-portal probe -- Android's
        // `/generate_204`, Apple's `/hotspot-detect.html`, Windows'
        // `/connecttest.txt` -- lands here and is answered with a redirect,
        // which is what makes the phone open the page by itself instead of
        // quietly deciding the network is useless and disconnecting.
        register(server, c"/*", sys::http_method_HTTP_GET, Some(handle_catchall));

        Ok(server)
    }
}

unsafe fn register(
    server: sys::httpd_handle_t,
    uri: &core::ffi::CStr,
    method: sys::http_method,
    handler: Option<unsafe extern "C" fn(*mut sys::httpd_req_t) -> sys::esp_err_t>,
) {
    let descriptor = sys::httpd_uri_t {
        uri: uri.as_ptr(),
        method,
        handler,
        user_ctx: core::ptr::null_mut(),
    };
    if unsafe { sys::httpd_register_uri_handler(server, &descriptor) } != sys::ESP_OK {
        println!("ap:   could not register {}", uri.to_string_lossy());
    }
}

/// `HTTPD_DEFAULT_CONFIG`, another C macro with no binding.
fn default_httpd_config() -> sys::httpd_config_t {
    sys::httpd_config_t {
        task_priority: 5,
        stack_size: 4096,
        // `tskNO_AFFINITY`. The C3 has one core, so this only matters for being
        // the same value the IDF's own default uses.
        core_id: i32::MAX,
        server_port: 80,
        ctrl_port: 32768,
        max_open_sockets: 7,
        max_uri_handlers: 8,
        max_resp_headers: 8,
        backlog_conn: 5,
        lru_purge_enable: true,
        recv_wait_timeout: 5,
        send_wait_timeout: 5,
        global_user_ctx: core::ptr::null_mut(),
        global_user_ctx_free_fn: None,
        global_transport_ctx: core::ptr::null_mut(),
        global_transport_ctx_free_fn: None,
        enable_so_linger: false,
        linger_timeout: 0,
        keep_alive_enable: false,
        keep_alive_idle: 0,
        keep_alive_interval: 0,
        keep_alive_count: 0,
        open_fn: None,
        close_fn: None,
        uri_match_fn: None,
        // **Not zero, which is what a hand-filled struct defaults to and what
        // cost an evening here.** This is the capability mask the server's task
        // stack is allocated with, and asking the allocator for memory with no
        // capabilities fails however much is free -- `httpd_start` then returns
        // `ESP_ERR_HTTPD_TASK`, which reads as "out of memory" and is not.
        // Measured at the failure: 166 KB free.
        task_caps: sys::MALLOC_CAP_INTERNAL | sys::MALLOC_CAP_8BIT,
    }
}

/// Send a whole response, given a Rust string.
unsafe fn respond(request: *mut sys::httpd_req_t, kind: &core::ffi::CStr, body: &str) -> sys::esp_err_t {
    unsafe {
        sys::httpd_resp_set_type(request, kind.as_ptr());
        sys::httpd_resp_send(request, body.as_ptr() as *const core::ffi::c_char, body.len() as isize)
    }
}

/// The query string, if there is one.
unsafe fn query(request: *mut sys::httpd_req_t) -> Option<String> {
    unsafe {
        let len = sys::httpd_req_get_url_query_len(request);
        if len == 0 {
            return None;
        }
        let mut buffer = vec![0u8; len + 1];
        let err = sys::httpd_req_get_url_query_str(
            request,
            buffer.as_mut_ptr() as *mut core::ffi::c_char,
            buffer.len(),
        );
        if err != sys::ESP_OK {
            return None;
        }
        buffer.truncate(len);
        String::from_utf8(buffer).ok()
    }
}

unsafe extern "C" fn handle_page(request: *mut sys::httpd_req_t) -> sys::esp_err_t {
    let body = crate::webui::page(&snapshot());
    unsafe { respond(request, c"text/html; charset=utf-8", &body) }
}

/// Queue a console line and bounce back to the page.
///
/// **A redirect rather than rendering the result.** The command has only been
/// *queued* when this returns — the main loop has not run it yet — so anything
/// rendered here would be the state from before. A redirect makes the browser
/// come back for a fresh page, by which time the loop has almost always drained
/// the queue, and it stops a reload from repeating the command.
unsafe extern "C" fn handle_do(request: *mut sys::httpd_req_t) -> sys::esp_err_t {
    unsafe {
        let raw = query(request).unwrap_or_default();
        match crate::webui::command_from_query(&raw) {
            Some(line) => {
                println!("ap:   queued `{line}`");
                enqueue(line);
            }
            None => println!("ap:   ignored a request it could not read: {raw}"),
        }
        sys::httpd_resp_set_status(request, c"303 See Other".as_ptr());
        sys::httpd_resp_set_hdr(request, c"Location".as_ptr(), c"/".as_ptr());
        sys::httpd_resp_send(request, c"".as_ptr(), 0)
    }
}

/// Stream the strike log.
///
/// **Chunked, from the file, never into RAM.** The log runs to megabytes and
/// the device has tens of kilobytes free; reading it into a `String` to send it
/// would be an allocation failure at exactly the moment somebody is trying to
/// rescue the data. `httpd_resp_send_chunk` writes as it reads.
unsafe extern "C" fn handle_log(request: *mut sys::httpd_req_t) -> sys::esp_err_t {
    use std::io::Read;
    unsafe {
        sys::httpd_resp_set_type(request, c"text/csv".as_ptr());
        sys::httpd_resp_set_hdr(
            request,
            c"Content-Disposition".as_ptr(),
            c"attachment; filename=\"strikes.csv\"".as_ptr(),
        );

        let Ok(mut file) = std::fs::File::open(crate::log::PATH) else {
            // Not an error: a device that has seen no strikes has no file.
            let empty = "no log on this device yet\n";
            return sys::httpd_resp_send(
                request,
                empty.as_ptr() as *const core::ffi::c_char,
                empty.len() as isize,
            );
        };

        let mut buffer = [0u8; 1024];
        loop {
            match file.read(&mut buffer) {
                Ok(0) => break,
                Ok(n) => {
                    let err = sys::httpd_resp_send_chunk(
                        request,
                        buffer.as_ptr() as *const core::ffi::c_char,
                        n as isize,
                    );
                    if err != sys::ESP_OK {
                        // The client hung up. Stop reading; the empty chunk
                        // below still has to be sent to close the response.
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        // A zero-length chunk is what terminates a chunked response.
        sys::httpd_resp_send_chunk(request, core::ptr::null(), 0)
    }
}

/// Answer every unknown path with a redirect to the page.
unsafe extern "C" fn handle_catchall(request: *mut sys::httpd_req_t) -> sys::esp_err_t {
    unsafe {
        let location = format!("http://{ADDRESS}/");
        let location = std::ffi::CString::new(location).unwrap_or_default();
        sys::httpd_resp_set_status(request, c"302 Found".as_ptr());
        sys::httpd_resp_set_hdr(request, c"Location".as_ptr(), location.as_ptr());
        sys::httpd_resp_send(request, c"".as_ptr(), 0)
    }
}

// --- what the page sees -----------------------------------------------------

/// Build a snapshot from the live state and publish it.
///
/// Called once a batch by the main loop. Cheap enough to do unconditionally —
/// a few dozen field reads and one short table — and doing it unconditionally
/// means the page is never rendering something stale from before a setting
/// changed.
#[allow(clippy::too_many_arguments)]
pub fn publish_from(
    uptime_s: u32,
    location: &'static str,
    tuning: &crate::tuning::Tuning,
    totals: &crate::session::Totals,
    history: &crate::history::History,
    battery_mv: u32,
    battery_percent: u32,
    log_records: u32,
    log_bytes: u32,
    credentials: Option<&Credentials>,
) {
    let epoch = crate::clock::now().unwrap_or(0);
    let point = tuning.point;

    let recent = history
        .recent
        .iter()
        .map(|entry| RecentRow {
            when: match entry.epoch {
                Some(epoch) => crate::clock::format_local(epoch).to_string(),
                // The strike is real; only its time is unknown. Saying so beats
                // a plausible 1970.
                None => "clock unset".to_string(),
            },
            distance: match entry.distance {
                crate::strike::Distance::Overhead => "overhead".to_string(),
                crate::strike::Distance::OutOfRange => "out of range".to_string(),
                crate::strike::Distance::Km(km) => format!("{km} km"),
            },
            energy: entry.energy_raw,
            score: entry.score_milli,
            strokes: entry.strokes,
        })
        .collect();

    let nearest_km = history.recent.iter().filter_map(|entry| match entry.distance {
        crate::strike::Distance::Overhead => Some(0u8),
        crate::strike::Distance::Km(km) => Some(km),
        crate::strike::Distance::OutOfRange => None,
    }).min();

    publish(Snapshot {
        uptime_s,
        epoch,
        time_local: match epoch {
            0 => String::new(),
            epoch => crate::clock::format_local(epoch).to_string(),
        },
        location,
        defence_raw: point.raw(),
        defence_percent: point.percent(),
        noise_per_min: totals.noise_per_min,
        disturbers_per_min: totals.disturbers_per_min,
        quiet_per_min: tuning.quiet_per_min(),
        strikes_total: totals.strikes,
        strikes_hour: history.last_hour().strikes as u32,
        strikes_day: history.day.recent(288).strikes as u32,
        nearest_km,
        last_strike: totals.last_strike.as_ref().map(|(distance, energy, epoch)| {
            let when = match epoch {
                Some(epoch) => crate::clock::format_local(*epoch).to_string(),
                None => "time unknown".to_string(),
            };
            let where_ = match distance {
                crate::strike::Distance::Overhead => "overhead".to_string(),
                crate::strike::Distance::OutOfRange => "out of range".to_string(),
                crate::strike::Distance::Km(km) => format!("{km} km"),
            };
            format!("{where_}, energy {energy}, at {when}")
        }),
        battery_mv,
        battery_percent,
        free_ram_kb: crate::system::heap_kb().0,
        log_records,
        log_bytes,
        frozen: tuning.frozen,
        tz_minutes: crate::clock::tz_minutes(),
        recent,
        credentials: credentials.map(|c| {
            (c.ssid.clone(), c.password.clone(), c.generated)
        }),
    });
}
