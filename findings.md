# Review notes — lightning 0.1.0 (static review)

Scope: all 15 files in `src/`, `Cargo.toml`, `build.rs`, `.cargo/config.toml`,
`rust-toolchain.toml`, `sdkconfig.defaults`, `partitions.csv`, `README.md`,
`doc/specs.md` (synced at HEAD, `8ae9b73140d8`), `tests/host/` (README +
`defence.rs` + `history.rs`), `tools/` (`recover.sh`, `watch.py`), `flash.sh`,
`littlefs_bindings.h`.

Method note: this machine has **no Rust toolchain** (`rustc`/`cargo` absent), so
nothing here was compiled and the `tests/host` harnesses were not run. The
findings below are from reading the source against the spec, not from execution.

---

## Verified consistent (no action)

- The IRQ pad story in `main.rs:153-193` matches §2 AS BUILT exactly: `gpio_reset_pin`
  before `PinDriver::input` (because `PinDriver` only calls `gpio_set_direction`,
  which does not un-mux GPIO21 from U0TXD), the claim *before* the 2 s console
  settle delay, and the idle-high-UART-vs-idle-low-INT contention documented
  with the hardware fix pointed at.
- Defence ladder (`defence.rs`, exercised in `tests/host/defence.rs`) matches §4.2:
  31 rungs across NF 0–7 / WDTH 2–15 / SREJ 0–11, one knob per rung, up **per
  batch**, down after 1 min of silence (`main.rs:958-971`). The SREJ cap at 11
  (not the 4-bit max of 15) is deliberate and documented.
- History rings (`history.rs`, `tests/host/history.rs`) match §4.3: Fine
  15 min/24 h, Medium 1 h/7 d, Coarse 6 h/30 d; epoch-indexed buckets so the CSV
  can be replayed at boot (`main.rs:546-555`); `distance_samples` is separate
  from `strikes` (Overhead/OutOfRange count but are not averaged).
- The `tests/host` copies are **in sync right now**: a fresh diff of both pairs
  shows only doc comments and the harness `main()` differ from `src/`. The
  warning in `tests/host/README.md` ("re-sync when the logic they mirror
  changes") is the standing risk, not a current failure.
- LittleFS integration is the normal pattern, not a mismatch: the partition is
  typed `data`/`spiffs` in `partitions.csv` but mounted by `esp_littlefs`
  (`littlefs_bindings.h`; `joltwallet/littlefs` 1.22.3 pinned in
  `components_esp32c3.lock`). The battery-switch rationale in
  `Cargo.toml:63-72` is consistent with §5.
- Power policy matches §7 AS BUILT: 5 min grace + 10 min console-activity
  (`power.rs:155,163`), no supply detection, `esp_pm_configure` with read-back
  (`power.rs:96-139`), `no-light-sleep` recovery feature (`Cargo.toml:59`) forces
  `Policy::Awake`.
- Clock: persisted epoch + uptime, re-saved every 15 min (`clock.rs:39`),
  `PLAUSIBLE_EPOCH` guard, tz stored in minutes (half-hour zones), UTC stored /
  local applied on the way out.
- Battery arithmetic is exact-integer on the FPU-less C3: `5/64` mV, `/256` %,
  `26/125` %/hr (`battery.rs:108-112`); the learned-range midpoint rule and the
  200 mV span floor are coherent with the moisture precedent.
- ISR discipline: the GPIO21/GPIO9 callbacks post a notification only
  (`main.rs:330-333,364-367`); all I2C runs in the main task after the settle
  delay (`collect`, `main.rs:1159-1161`) — the MicroPython pattern (bus access
  in the ISR) is explicitly not copied.
- Button handling: falling edge is only a hint; `button_held` (duration sample,
  `main.rs:1367-1377`) separates a person from a USB host asserting DTR, and the
  reject path says so on the console instead of swallowing it.
- `antenna_self_test` (`main.rs:1297-1356`) correctly drives the IRQ pin with
  LCO÷16 (~31 kHz) and confirms the wire before trusting silence — §2's own
  GPIO21-vs-GPIO20 ambiguity is the reason it exists.
- Log sync is batched to 1/min (`main.rs:890-899`), safe on LittleFS
  specifically (unsynced write is lost, not corrupting); `clear` also resets
  charts and counters so screen and CSV cannot disagree.

---

## Findings

### 1. `src/main.rs` is 1440 lines; `listen` alone is ~560 — the highest-value split

- `main()` init runs 142–434 (~290 lines), and `listen()` spans 516–1077
  (~560). Inside `listen`, the console-command dispatch is a self-contained
  ~200-line match (`main.rs:686-883`), the gauge/PM cadence is 915–947, the
  auto-tune step 949–971, and the redraw gate 973–1072.

Concrete moves, in order of value:

a. **Extract the console dispatch into its own module** (`commands.rs`) with a
   small `Ctx` struct borrowing the mutable state (`sensor`, `i2c`, `location`,
   `totals`, `history`, `strike_log`, `chart_period`, `reading`, `range`,
   `level`, `die_temperature`, `antenna_khz`, `irq_confirmed`). The block is
   already one big `match console::Command`; moving it as-is takes the largest
   single chunk out of the loop with zero behavioural change.

b. **Collapse the eight repeated FATAL match arms** (`main.rs:145-151, 187-193,
   221-232, 306-309, 317-320, 330-337, 350-356, 358-361, 364-371`) into
   `let-else` — the crate declares `rust-version = "1.77"`, and `let-else` is
   stable since 1.65. Each becomes
   `let Ok(x) = ... else { println!("FATAL: ..."); return };`.

c. **Move `Batch`, `Totals`, `Drawn` and their helpers** (`collect`,
   `report`, `tune`, `toggle_location`, `fill_chart`) out of `main.rs`. They are
   already cohesive; they are only in `main.rs` because that is where they grew.
   `listen` then reads as a loop skeleton calling the same well-named steps it
   already executes.

`src/ui.rs` (865 lines) is a second candidate: `status` 188–354, `status_line`
436–621, `charts` 752–865. It is already cleanly factored into section drawers;
a split into `ui/status.rs` / `ui/charts.rs` is optional and only worth doing
after the header essay (finding 2) is trimmed.

### 2. The in-code essays now duplicate `doc/specs.md` — the spec sync made them second copies

~1,900 of 5,138 source lines are comments (37 %). Highest densities: `power.rs`
112/180, `battery.rs` 118/232, `defence.rs` 54/113, `ui.rs` 256/865, `main.rs`
438/1440. Since `8ae9b73140d8` made `doc/specs.md` the AS BUILT record, the long
module headers restate it: `power.rs:1-66` vs §7, `battery.rs:1-29` vs §2.1,
`defence.rs:1-40` vs §4.2, `ui.rs:1-41` vs §6.

The decay is not hypothetical. `power.rs:141-145` is a **dangling doc block on
`GRACE_S`** that still reads "which policy the gauge's discharge rate calls
for… every case except 'actively discharging' resolves to [`Policy::Usb`]" —
`Policy::Usb` no longer exists and supply detection was abandoned (§7 AS BUILT).
It is three stale paragraphs attached to an unrelated constant.

Recommendation: collapse each essay to a 2–4 line pointer ("see §N"), keep the
per-line comments that carry non-obvious *code* reasoning (midpoint rule, the
ISR contract, the GPIO21 ordering), and delete what the spec now owns. This
shrinks the files, makes finding 1's split easier, and removes a second place
for the AS BUILT story to drift.

### 3. `tests/host` are hand-synced copies — extract a shared lib target

Same hazard as the moisture project's finding 3, in better shape: both pairs
(`defence`, `history`) are in sync today and the harnesses re-declare the
minimum of scaffolding (`tests/host/history.rs:9-21` re-declares `Distance` /
`Strike` because they come from `crate::as3935`). But "a copy of `src/…`'s
logic" (`tests/host/README.md`, `tests/host/defence.rs:4`) is a manual step
that will be skipped exactly once, and nothing runs them: `harness = false` on
the bin and no CI/justfile/Makefile wire the host tests to anything.

Durable fix, identical to moisture's: extract the pure cores (`defence::settings`,
`history::History`, `battery::widened`, `clock::format`) into a library target
(`src/lib.rs` or a `core` module) that both `src/main.rs` and `tests/host/*.rs`
import. Divergence becomes a compile error instead of a note. That also
unblocks `cargo test` for the modules whose behaviour is invisible on hardware.

### 4. API — adopt `max170xx` for the gauge's register plumbing (optional, small win)

`battery.rs:86-126` hand-rolls three big-endian reads plus a version probe.
`max170xx` 1.0.0 (eldruin, embedded-hal 1.0 — a direct match for esp-idf-hal
0.46) provides `soc()`, `voltage()`, `charge_rate()` and `version()` for the
MAX17048.

Caveats: it returns `f32` (soft float on `riscv32imc` — irrelevant at a 60 s
poll), and you would keep the exact-integer conversions, the sign semantics
(`crate_centi_per_hour`) and the learned range (`battery.rs:128-232`), which the
crate does not cover. Net win: ~30 lines of endianness-prone code that the
vendor maintains instead of you. Optional; the current code is small and exact.

### 5. API — replace the two hand-rolled digit writers with `core::fmt::Write`

`clock::push_padded` (`clock.rs:139-162`) and `ui::write_u32`
(`ui.rs:417-434`) are manual digit loops. `heapless::String` implements
`core::fmt::Write`, and the binary already pulls the formatter in via
`println!`, so `write!(&mut out, "{value}")` costs no new machinery and no heap
allocation on the render path (fmt pads on the stack, which is fine against the
"no allocating on the way in" constraint at `main.rs:556-558`). Removes ~45
lines across the two files.

### 6. API — `esp-idf-svc`, deferred until §5 WiFi lands

`Cargo.toml:14-17` documents the deliberate absence (it links the radio stacks
for pure binary-size cost while there is no network). When §5's WiFi/SNTP does
land, `esp-idf-svc` brings `EspLogger`/`log` (levels, timestamped output) and
the WiFi API in the same dependency — and the NVS-clock `save()` hack
(`clock.rs:91-95`) can stop fighting for accuracy. Not now; the seam is worth
noting so it is not added early or forgotten late.

### 7. `as3935-generic` exists but does not fit — no action

`as3935-generic` 0.1.5 (embedded-hal 1.0) covers basic register access but not
`DISP_LCO` — which `antenna_self_test` (`main.rs:1297-1356`) depends on to drive
the IRQ pin and count edges — nor the tuning-cap / min-strikes / disturber-
enabled sequence §3/§4.2 relies on. Keeping the custom driver
(`as3935.rs`) is the right call; worth one line in `as3935.rs` saying so, so it
is not re-litigated after the next crates.io search.

### 8. Dedupe the heapless fallback strings and the STRIKE print

- `heapless::String::try_from("(clock not set)").unwrap_or_default()` appears at
  `main.rs:719, 756` and `(no clock)` at `main.rs:1199` (plus `:76`). One helper
  `fn fallback(s: &str) -> heapless::String<20>` removes all of them.
- The real-strike path (`collect`, `main.rs:1201-1208`) and the simulated path
  (`main.rs:752-762`) print the identical `STRIKE  {when}  {:?}  energy {}
  (intensity {}.{:03})` and perform the same record/append/history sequence.
  A shared `fn record_strike(sensor, i2c, totals, history, log, strike, sim)`
  keeps the two paths — which must stay behaviourally identical — from drifting.

### 9. `fill_chart` is three identical arms

`fill_chart` (`main.rs:1392-1424`) repeats the same flatten-and-copy three times,
differing only in the ring's const length. `series_of` is already generic over
the ring length, so a single
`fn flatten<const N: usize>(ring: &history::Ring<N>, counts: &mut [u16], scores:
&mut [u32]) -> (usize, usize)` collapses all three arms into three one-line calls.

---

## Suggested next step

The review is complete. The highest-value code change is finding 1 + finding 2
together: trim the essays, extract the console dispatch and the state helpers,
collapse the FATAL arms with `let-else`, and (finding 3) pull the pure cores
into a shared lib target so `tests/host` imports them instead of copying. Say
the word and I can do that refactor — it is mechanical and the spec is in sync
to verify against.
