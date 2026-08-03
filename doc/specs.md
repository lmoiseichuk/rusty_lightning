# Lightning-Detector Terminal — Specification

A battery-capable e-paper terminal that detects lightning with an AS3935 sensor, auto-tunes its
sensitivity, tracks a storm's distance / intensity / score and trend, estimates arrival time, logs
every strike to a CSV database, and shows the current situation on a 7.5" e-paper display —
refreshing only when something meaningful changes.

Firmware target: **Rust (`esp-idf-hal`, std)** on an **ESP32-C3**. A working MicroPython reference
(`DFRobot_AS3935_Lib.py` + `deep_demo.py`) defines the sensor behaviour to reproduce.

---

## 1. Hardware

- **Compute + display:** Seeed **XIAO 7.5" ePaper Panel** — driver board hosting a **XIAO ESP32-C3**
  (RISC-V), a **7.5" 800×480 monochrome e-paper** (**UC8179**, "7.5in **V2**"), and a **2000 mAh**
  battery with onboard charging. [Panel](https://www.seeedstudio.com/XIAO-7-5-ePaper-Panel-p-6416.html)
  · [wiki](https://wiki.seeedstudio.com/xiao_075inch_epaper_panel/) ·
  [Arduino](https://wiki.seeedstudio.com/xiao_075inch_epaper_panel_arduino/) ·
  [ESPHome](https://wiki.seeedstudio.com/xiao_075inch_epaper_panel_esphome/) ·
  [driver-board schematic (PDF)](https://files.seeedstudio.com/wiki/xiao_075inch_epaper_panel/ePaper_Driver_Board.pdf).
- **Sensor:** DFRobot **Gravity Lightning Sensor (SEN0290)** = **AS3935**, I2C (address **0x03**) +
  active-high IRQ. [wiki](https://wiki.dfrobot.com/Gravity:%20Lightning%20Sensor%20SKU:%20SEN0290).

### Firmware stack

ESP32-C3 is RISC-V → the **stock `rustup` target `riscv32imc-esp-espidf`** (no Xtensa/`espup`).
Scaffold with `esp-idf-template`; use `esp-idf-hal` + `esp-idf-svc`. Console + flashing over the
XIAO's **USB-C** (native USB-serial-JTAG), so UART0 (GPIO20/21) is free for I/O.

**Chip choice — stay on C3.** A XIAO ESP32-C6 was considered but gives no benefit here: the display
consumes the analog pads (D0–D3) on *both* chips, and the pads left free are digital-only on both —
so a C6 frees **no ADC pin** either. The C6 also has a **boot/reset-button conflict on this panel**
([forum](https://forum.seeedstudio.com/t/seeed-studio-xiao-7-5-epaper-panel-battery-status/292932/6))
plus GPIO9 strapping quirks. Battery uses an **I2C monitor** (§2.1) that needs no ADC pin, so the
chip choice is irrelevant to it.

---

## 2. Pin map

**Display pins are fixed by the driver board** (from the Seeed ESPHome/Arduino configs):

| Display signal | GPIO | Note |
|---|---|---|
| SPI SCK | GPIO8 | |
| SPI MOSI | GPIO10 | |
| CS | GPIO3 | |
| DC | GPIO5 | |
| RST | GPIO2 | |
| BUSY | GPIO4 | **inverted logic** on this board |

That consumes GPIO 2, 3, 4, 5, 8, 10. **Free XIAO pads → the AS3935:**

| Sensor signal | XIAO pad | GPIO | Note |
|---|---|---|---|
| I2C SDA | D4 / SDA | **GPIO6** | XIAO native I2C, free |
| I2C SCL | D5 / SCL | **GPIO7** | XIAO native I2C, free |
| IRQ | D6 / RX | **GPIO21** | rising-edge input; UART0 pin, free (console is on USB-C) |
| 3V3 / GND | — | — | AS3935 at 3.3 V |

- **Do not** put the IRQ on **GPIO9 (D9)**: it's the boot strap pin and the AS3935 INT idles **low**
  (low at reset → download mode → won't boot). GPIO20 (D7) is an acceptable alternate for the IRQ.

> #### ⚠⚠ AS BUILT: the IRQ is on **GPIO21 (D6)**, and two traps came with it
>
> This document contradicted itself — the table above says GPIO21, while §2.1's diagram and §10's
> build order said GPIO20. The wire follows the table. The firmware's LCO self-test found this in
> seconds by making the sensor drive its own IRQ pin at 31 kHz and counting edges; a pin that sees
> nothing while the sensor is deliberately driving it is not connected.
>
> **`PinDriver` does not un-mux the pad.** It calls `gpio_set_direction`, not `gpio_reset_pin` — and
> only the latter restores a pad's IO_MUX selection to plain GPIO. The ROM muxes GPIO21 to UART0 at
> every reset, so without an explicit `gpio_reset_pin` the pin reads UART idle regardless of what
> the sensor does. That presents as "not connected", which is the wrong conclusion and costs a
> wiring investigation.
>
> **GPIO21 is U0TXD and a UART idles high, while the AS3935's INT idles low.** From reset until the
> pad is claimed, the ROM and the sensor drive it in opposite directions. Nothing in firmware
> prevents the ROM's share of that window; what the firmware *does* do is claim the pad before the
> console settle delay rather than after, so the contention is not extended by two seconds on every
> boot.
>
> **GPIO9 is the mode button, and the USB host presses it.** The C3 maps the host's CDC **DTR** line
> onto GPIO9, so every `espflash` or terminal connection drives it low — indistinguishable from a
> fingertip, because electrically it is one. A press therefore means the pin stays low for 1.5 s,
> which no flashing tool holds.
- **No battery monitoring on the board, and no free C3 ADC pin.** The board has no battery-sense
  circuit ([forum](https://forum.seeedstudio.com/t/seeed-studio-xiao-7-5-epaper-panel-battery-status/292932)),
  and the C3's only ADC pads (A0–A3 = GPIO2–5) are all consumed by the display (ADC2 is also unusable
  with WiFi on). So a divider-into-the-C3-ADC **cannot** be wired here — battery sensing must go on
  the **shared I2C bus** (§2.1).

### 2.1 Battery monitoring — Adafruit MAX17048

A **MAX17048** LiPo fuel gauge on the **shared I2C bus** (GPIO6/7, addr **0x36** ≠ AS3935 0x03) — no
extra MCU pins, no ADC needed. It self-calibrates (ModelGauge) on the cell it powers from and reports:

- **`VCELL` (0x02)** — cell voltage;
- **`SOC` (0x04)** — state of charge **%** (no divider/tuning);
- **`CRATE` (0x16)** — discharge rate %/hr → **time-to-discharge ≈ SOC / |CRATE|** (a real figure).

Two facts about the [Adafruit board](https://learn.adafruit.com/adafruit-max17048-lipoly-liion-fuel-gauge-and-battery-monitor/pinouts)
(#5580) shape the wiring:

- **Powered by the battery, not by VIN** — the chip runs off the cell in its **JST**. The board has
  **two equivalent JST ports** (battery on one, load on the other → **inline pass-through**) and **two
  STEMMA QT** ports for I2C chaining.
- **SDA/SCL pull-ups reference VIN, and there's no level shifter** → **tie VIN → 3.3 V** (XIAO 3V3) so
  the bus stays **3.3 V-safe** for the C3. Do *not* put 5 V or the battery on VIN.

**Battery hookup — parallel is enough.** The MAX17048 measures **voltage only** (no current sense), so
routing the load *through* it gains nothing. Simplest: solder 2 wires from the panel's `BAT+`/`BAT-`
to a JST, plug it into **one** port, and leave the other empty — the gauge reads the cell in parallel
(µA draw). *(Inline pass-through — battery → port A, port B → panel — also works, just isn't needed;
no JST-JST harness required.)* Mind JST **polarity** (BAT+→+, BAT-→−; reversing can damage the gauge);
find `BAT+`/`BAT-` on the
[driver-board schematic](https://files.seeedstudio.com/wiki/xiao_075inch_epaper_panel/ePaper_Driver_Board.pdf)
(`BAT-` = board GND).

**Minimal-wire cascade** (your goal): one shared **4-wire I2C trunk** (3V3, GND, SDA=GPIO6, SCL=GPIO7)
+ **one IRQ wire** — mostly plug-in cables, not solder:

```
XIAO C3 ──5 solder wires──►  3V3 · GND · GPIO6(SDA) · GPIO7(SCL) · GPIO20(IRQ)
                              └───────── I2C trunk (first four) ─────────┘
   STEMMA QT cable →  MAX17048  ──STEMMA-to-Gravity cable──►  AS3935
                      (battery via JST;                       (VCC = 3V3;
                       VIN = 3V3 → pull-ups)                    IRQ → GPIO20)
```

- **Solder only 5 wires** off the XIAO (3V3, GND, GPIO6, GPIO7, GPIO20); chain the two boards with a
  **STEMMA QT (Qwiic) cable** — use a **Qwiic-to-Gravity** adapter for the DFRobot AS3935 (or wire its
  4-pin Gravity header onto the trunk). The gauge's **2nd STEMMA port is the chain-through**.
- Both boards may carry their own I2C pull-ups; parallel pull-ups only lower the effective resistance —
  fine. The gauge's `INT` / `QSTART` pads are unused.
- **Rust:** the `max17048` crate over `esp-idf-hal` I2C, or trivial register reads (0x02 / 0x04 / 0x16).

**Chosen build:** solder a **Gravity→QT** adapter on the AS3935 → QT cable into the MAX17048 → solder
the gauge's **second QT** to the XIAO's **SDA(GPIO6) / SCL(GPIO7) / 3V3 / GND**. Two things are *not*
on the QT bus and need their own connection: the **AS3935 IRQ → GPIO20** (one wire; the Gravity 4-pin
is I2C-only), and the **battery → MAX17048 JST** (from the panel `BAT+`/`BAT-`).

#### Substituting a MAX17043

A **MAX17043** works as a replacement — same family, same **0x36** address, same 2.5–4.5 V range, same
self-powered-from-the-cell wiring — but the firmware is **not** byte-identical. Three differences:

| | MAX17043 | MAX17048 |
|---|---|---|
| `VCELL` (0x02) | **1.25 mV**/LSB in the **top 12 bits** → `mV = (raw >> 4) * 1.25` | **78.125 µV**/LSB, full 16 bits → `mV = raw * 0.078125` |
| `SOC` (0x04) | %·256, identical | %·256 |
| `CRATE` (0x16) | **absent** | 0.208 %/hr per LSB (signed) |
| Registers | 0x02 / 0x04 / 0x06 / 0x08 / 0x0C / 0xFE only | + `HIBRT` 0x0A, `VALRT` 0x14, `CRATE` 0x16, `VRESET/ID` 0x18, `STATUS` 0x1A, `TABLE` 0x40–0x7F |
| Quiescent | 50 µA typ (75 µA max); sleep stops gauging | **23 µA** active, **3 µA** hibernate (auto) |
| Alerts | low-SOC threshold only (`ATHD`) | + voltage window, 1 % SOC step, battery-swap reset |

Impact on this design:

1. **Time-to-discharge must be derived in firmware** — with no `CRATE`, keep the previous `(timestamp,
   SOC)` sample and compute `%/hr` from the slope over a ≥15 min window (EMA-smooth it; a 1-sample
   delta is noise). The status bar already redraws on the hourly cadence, so a slow estimator is fine.
2. **Different `VCELL` maths** — shifting by 4 and scaling by 1.25 mV. Getting this wrong yields a
   plausible-but-wrong voltage, so gate it behind a `VERSION` (0x08) read or a build-time constant.
3. **~2× the idle draw** — 50 µA vs 23 µA ≈ 1.2 mAh/day on the 2000 mAh pack (**0.06 %/day**).
   Irrelevant next to the e-paper refreshes; not a reason to choose either part.

Rust drivers cover both: [`max170xx`](https://crates.io/crates/max170xx) exposes distinct `Max17043` /
`Max17048` types, so the scaling is handled for you — just instantiate the right one.

**Verdict:** stay on **MAX17048** (`CRATE` for free, lower draw, better ModelGauge). The
MAX17043 is a fine fallback if one is already on hand — cost is ~15 lines of slope estimator. Note the
Adafruit-board specifics above (JST power, `VIN` as pull-up reference) are **board**-specific, not
chip-specific: re-check power and pull-up arrangement on whatever MAX17043 breakout you use.

**Fallback (only if no gauge is on hand)** — your 2R + 1C divider into an **ADS1115** I2C ADC
(addr **0x48**), same bus; more parts, needs calibration, no SoC %:

```
 BAT+ ──[ R1 ]──┬──[ R2 ]── GND        R1 = R2 (1:2 → Vbat/2); 1 % metal film,
                │                        e.g. 2× 200 kΩ–1 MΩ (higher = less drain)
                ├───────────► ADS1115 A0
                │
              [100 nF]                   shunt tap → GND (reservoir/filter)
                │
               GND
     ADS1115:  VDD→3V3  GND→BAT-  SDA→GPIO6  SCL→GPIO7  ADDR→GND (0x48)
```
The ADS1115 has a wide input range (no ~2.45 V ceiling like the C3), so the ratio is flexible;
calibrate once against a multimeter.

---

## 3. Sensor: AS3935

Register access and semantics per the reference driver (`DFRobot_AS3935_Lib.py`). Init sequence
(from `deep_demo.py`):

1. **Detect:** probe I2C **0x01/0x02/0x03**, `reset()` (write `0x3C=0x96`) until one ACKs (this unit
   is **0x03**).
2. `powerUp()` — clear PWD (reg 0x00 bit0) + **RCO calibration** (`0x3D=0x96`, then toggle `DISP_SRCO`
   in reg 0x08).
3. **Indoor/outdoor** — reg 0x00 mask 0x3E → `0x24` indoor / `0x1C` outdoor.
4. `disturberEn()` (reg 0x03 `MASK_DIST`), `setIrqOutputSource(0)`, wait 500 ms.
5. **Antenna tuning** `setTuningCaps(120)` (reg 0x08, 120 pF default; scope-tune only if strikes read
   wrong — the reference leaves the LCO-on-IRQ tuning path commented).
6. `setNoiseFloorLv1(0)` start, `setWatchdogThreshold(2)`, `setSpikeRejection(0)`.

**IRQ reason** (reg 0x03 low nibble, read ≥3 ms after the edge; the read clears it): `0x08` =
lightning, `0x04` = disturber, `0x01` = noise-too-high.
On lightning read **distance** (reg 0x07 & 0x3F, km) and **energy** (`(0x06 & 0x1F)<<16 | 0x05<<8 |
0x04`, then `/16777` for the "intensity" figure). **Score = intensity / max(distance, 0.1 km)**.

> **IRQ handling in Rust:** keep the GPIO ISR minimal — it only **notifies** (flag / `Notification` /
> channel). The **main task** does the ≥3 ms wait, the I2C register reads, and event batching. (The
> MicroPython reference does I2C inside the callback; that pattern must not be copied on esp-idf.)

---

## 4. Behaviour

### 4.1 Indoor/outdoor
Set the AFE gain at startup (`0x24` indoor / `0x1C` outdoor). Selectable via config.

### 4.2 Noise-floor auto-tune — a 31-rung ladder, not `NF_LEV` alone
Asymmetric, per the reference: **any** disturber/noise IRQ in the ~1 s processing batch → **+1
immediately**; **60 s with no events** → **−1**. Quick to defend, slow to relax.

> #### ⚠ AS BUILT: `NF_LEV` alone runs out, measured in seconds
>
> On a bench beside a WiFi access point the level saturated at 7 within seconds and then repeated
> *"noise floor already at 7, cannot defend further"* indefinitely. A detector that has stopped
> defending and says so once a second is stuck, not tuned.
>
> The AS3935 has two more knobs doing the same job at different stages of the receive chain, so
> "defence" is **one integer walking all three** in increasing order of what they cost in
> sensitivity:
>
> | Rung | Register | Rejects |
> |---|---|---|
> | `NF_LEV` 0–7 | `0x01` | continuous noise, by raising the detection floor |
> | `WDTH` 2–15 | `0x01` | events that do not look like a strike's envelope |
> | `SREJ` 0–11 | `0x02` | spikes that pass the watchdog but fail the shape test |
>
> 31 rungs rather than 7. `SREJ` stops at 11 of its possible 15 deliberately: the datasheet's curves
> flatten past there and the last settings reject hard enough to discard a genuine nearby strike. A
> detector that hears nothing is worse than one that hears noise, because the noise is at least
> visible.
>
> **The step is per batch, not per event.** An earlier version escalated per interrupt and saturated
> the whole ladder in under two seconds — a counter racing the interrupt rate, not tuning.

### 4.3 Storm tracking
Per lightning strike collect `distance`, `intensity`, `score`. Estimate the situation from the trend:
coming in (distance ↓), moving out (distance ↑), stronger (same distance, higher intensity), weaker,
**aggravating** (closer *and* higher score), **fading** (farther, lower score).

- **Thresholds are placeholders to calibrate in the final enclosure** — breadboard vs. enclosed
  sensitivity differs, so expose the band/hysteresis limits as config constants and tune on-site
  (the auto noise-floor helps absorb some of this).
- A single strike is noisy — declare a trend only over a small **window of the last N strikes**
  (N tunable), not strike-to-strike.
- Reference-data scale (for chart bounds): observed distance ~6 km, intensity ~3–7, score ~0.5–1.2;
  score rises sharply as distance → 0, so the **score chart should auto-scale** (or clamp/log the top).

### 4.4 ETA
Start with a **linear** estimate from the distance trend over the recent window (Δdistance/Δtime).
Refine later from the logged database once enough storms are recorded.

---

## 5. Data logging (CSV) + time

- **Format: a single CSV file** on the on-flash filesystem (LittleFS/FAT via `esp-idf-svc`) — simpler
  than a binary store and directly inspectable. **Fields: `timestamp, distance_km, intensity`**
  (score is derived, not stored). A header row denotes an empty (0-record) file.

> #### ⚠ AS BUILT: `/lfs/strikes.csv`, and **LittleFS specifically**
>
> ```text
> timestamp,iso_local,distance_km,energy_raw,intensity_milli,score_milli
> 1785727634,2026-08-02 23:27:14,6,50331,3000,500
> ```
>
> **LittleFS, not SPIFFS.** The board has a battery ON/OFF switch, so power is removed abruptly and
> often — that is how the device is normally turned off, not an edge case. SPIFFS is not power-loss
> resilient: a cut mid-write can corrupt the *filesystem*, taking the whole log rather than the one
> record being written.
>
> That choice is what makes **one-minute write batching** safe. Lines buffer in RAM and `fsync` on a
> cadence, so a storm producing events every few seconds is not a flash write every few seconds; an
> unsynced LittleFS write is *lost*, not corrupting, so the exposure is bounded to a minute of
> strikes rather than the file.
>
> Two format decisions worth keeping:
>
> * **`overhead` and `far` are words, not 1 and 63.** Those two values are not distances, and
>   anything averaging that column must not silently take them for kilometres.
> * **An unset clock writes `0` and an empty ISO column**, never a plausible 1970 date. The strike
>   happened; what is unknown is *when*, and saying so is recoverable where inventing it is not.
>
> `timestamp` is UTC; `iso_local` applies the offset set by `tz`. The epoch is for machines, the ISO
> string is for whoever opens the file.
- **Capacity / retention:** ~2 MB filesystem. At ~30 bytes/record that's **~70 000 records** —
  effectively unbounded for this use. Track the record count (from a maintained counter or
  `file_size − header`), and estimate remaining capacity as `free_bytes / record_bytes`. When near
  full, rotate (rename/archive) or overwrite oldest — policy TBD.
- **On boot**, reload recent records to rebuild the score chart and storm trend; the CSV is the
  source of truth, the in-RAM chart buffer is the live working copy.
- **⚠ AS BUILT: the clock is set over the console** (`date <unix-epoch>`, `tz <hours>`) and needs no
  network at all. Written to NVS, restored on every boot, and re-saved every fifteen minutes —
  otherwise a device told the time on Monday and power-cycled on Friday comes back believing it is
  Monday. `clock::now()` returns `None` rather than a plausible number when never set, because a
  caller stamping a strike has to decide what to do about that.
- **Time = "fake NTP": persisted epoch + uptime.** Keep the last-known wall-clock (epoch) in NVS; on
  boot `now = nvs_epoch + uptime`, so time is monotonic from the last known point with **no hard
  network dependency**. When a real **SNTP** sync succeeds (brief WiFi up → sync → WiFi down),
  correct `now` and **rewrite the NVS epoch**. Drift between syncs is fine for strike logging.
- **WiFi credentials in NVS** (SSID/password). Provision either by a **first-boot USB-serial console
  prompt** (esp-idf std can read stdin over USB-serial-JTAG) writing to NVS, or by pre-seeding NVS.
  Keep creds out of the source and out of the published CSV.

---

## 6. Display (UC8179, 7.5" V2)

- Rust driver: **`epd-waveshare`** `Epd7in5_V2` (UC8179, 800×480) + `embedded-graphics`. Mind the
  board's **inverted BUSY**.

> #### ⚠⚠ AS BUILT: three assumptions here were wrong, each found by measuring
>
> **1. BUSY is NOT inverted.** An inverting adapter was written on the strength of the line above
> and hung every refresh — the driver read idle as busy and waited forever. Reading the pad either
> side of init settled it: `LOW` immediately after (panel working), `HIGH` a few ms later (idle).
> That is stock Waveshare polarity and exactly what `epd-waveshare`'s `IS_BUSY_LOW = true` expects.
>
> **2. The driver's own wait races the panel.** `update_and_display_frame` reported success in
> **116 ms** — which is just the SPI transfer, 48 000 bytes at 4 MHz. Watching BUSY afterwards
> showed the refresh finishing **3 787 ms** later. Anything issued in that window lands on a panel
> mid-refresh, so the firmware waits again itself: first for BUSY to *fall* (the refresh has begun),
> then for it to *rise*. Waiting for the fall is what closes the race.
>
> **3. The colours are inverted.** `Color::Black` on `Color::White` produced white text on a black
> screen. `epd-waveshare` is not wrong — it sends `White` as `0xFF`, correct for a stock 7.5" V2 —
> this panel reads those bits the other way. Fixed by naming the two by **role**, `INK` and `PAPER`,
> so the inversion lives in one place and no layout code mentions a colour.
>
> A fourth, not an assumption but a trap: **`Ets` is the wrong delay**. It busy-waits, and a
> ~3.8 s refresh starved the idle task until the task watchdog rebooted the device mid-draw — a boot
> loop immediately after "panel up". `Delay` hands long waits to `vTaskDelay`. The [micropython-waveshare 7.5-V2 driver](https://github.com/mcauser/micropython-waveshare-epaper/pull/12/changes/b5d20249d858a0a84629b9b3004a9779b746e76f)
  is a useful command-sequence reference if the crate needs coaxing.
- **Refresh policy:** spec cadence is **5 s**, but gate on the panel: never refresh while **BUSY**,
  and only when a value actually **changed**. A full 800×480 refresh is ~2–5 s; use **partial
  refresh if this panel/driver supports it reliably** (uncertain on 7.5" V2 — verify), otherwise a
  change-gated full refresh, plus a periodic (e.g. hourly) full refresh as a baseline / de-ghost.

> #### ⚠ AS BUILT: full refresh only, measured at **3.8 s**, so the 5 s cadence is unusable
>
> Partial refresh is not available: `epd-waveshare` 0.6 implements `Epd7in5::update_partial_frame`
> as a literal `unimplemented!()`, so calling it **panics** rather than degrading to a full refresh.
> Getting it would mean writing UC8179 support rather than configuring it.
>
> At 3.8 s per refresh the nominal 5 s cadence would leave the panel busy ~80 % of the time, so the
> screen is change-gated with a **30 s floor** and a **5 min baseline**. What is allowed to vote is
> deliberately narrow — **strikes, the mode button, and the noise level**. Everything else rides the
> baseline, because each of the excluded ones was measured pinning the panel:
>
> * the **disturber count** moves every second in a noisy room;
> * the **battery, temperature, clock and heap** move far slower than they are worth reporting.
>
> A **button press bypasses the floor**: it is deliberate, rare, and the person is standing there.
> A setting that takes half a minute to appear reads as a button that does not work.
- 800×480×1bpp = 48 KB framebuffer — fits the C3's SRAM easily.
### Layout (three zones, top → bottom)

1. **Status bar** — clock, **recorded flashes / capacity** (e.g. `1 234 / ~70 000`), and battery
   (voltage · % · time-to-discharge) from the **MAX17048** (§2.1). Time-to-discharge = `SOC / |CRATE|`;
   if a MAX17043 is fitted instead, it comes from the firmware SOC-slope estimator (§2.1).
2. **Situation zone** — the storm state (calm / coming in / moving out / stronger / weaker /
   aggravating / fading) with a mood **icon** (🙂 / 🙁 / ⚡). Reserve space for later stats
   (nearest & strongest strike; counts per day / week / month).
3. **Score chart** — **last 12 h**, bucketed every **15 min** → **48 buckets** across the 800 px
   width (~16 px each). Auto-scale the y-axis (score spikes as distance → 0).

> #### ⚠ AS BUILT: two charts, three selectable spans, filling left to right
>
> ```text
> 2026-08-02 23:27  80MHz  38C  ram 264/52KB  flash 1976/0KB   batt 97% 4.16V  12h left
> ─────────────────────────────────────────────────────────────────────────────────────
> Mode: indoor        Noise ███░░░░ 3/31              Disturbers: 47
> hold BOOT to switch
>     7            1 km · intensity 9 · 23:27
> strikes
>     7              2.19          6 km           1 km
> strikes/hour    mean score   mean distance    closest
> Last 24 hours
> ▁▁▁▃▂▁█░░░░░░  strikes  peak 7
> ▁▁▁▂▄▁█░░░░░░  mean score  peak 2769
> ─────────────────────────────────────────────────────────────────────────────────────
> antenna 500 kHz · IRQ OK · disturbers 47 · cell 3600-4165 mV
> ```
>
> **Two charts, because neither answers the other's question.** A count says how busy the sky was; a
> mean score says how dangerous. One violent overhead strike and a hundred distant ones give the
> same count and wildly different scores, and the reverse is true of the mean.
>
> **Three spans, not one 12 h window**: `scope day|week|month` selects a 24 h / 7 d / 30 d ring
> (15 min / 1 h / 6 h buckets). Rings are indexed from the **Unix epoch**, not from power-on, which
> is what lets them be **rebuilt from the CSV at boot** — otherwise every reboot shows a device that
> has never seen a storm.
>
> **They fill left to right and only scroll once full.** A ring running two hours holds eight
> buckets, not ninety-six, and drawing the rest as empty columns to the *left* reads as "a day of
> silence, then this" — a different and wrong story. Column width comes from the ring's capacity
> rather than from how full it is, so bars keep their size as it fills.
>
> Auto-scaled as specified, with the **peak printed beside each label**: two charts drawn on
> different days are not comparable by height, and the number is what makes them comparable. Any
> non-zero bucket draws at least one pixel — the difference between "calm" and "no data" is the
> distinction a chart exists to make.

---

## 7. Power

Usually **USB-powered**, occasionally portable (2000 mAh). Power saving is a *nice-to-have*, not a
primary goal. Model: mostly-on when on USB; **light-sleep between events with IRQ + timer wake** when
on battery; the e-paper holds its image with no power, so the screen can persist while the MCU
sleeps. Keep WiFi off except for the brief NTP sync.

> #### ⚠⚠ AS BUILT: "nice-to-have" understates it by a factor of twenty
>
> On the 2000 mAh cell the difference between the default policy and a considered one is roughly
> **four days against eighty**. Two facts drive that, and the first was a bug:
>
> **`CONFIG_PM_ENABLE=y` does nothing on its own.** It was set in `sdkconfig.defaults` and
> `esp_pm_configure` was never called, so the chip sat at 160 MHz continuously with no DFS and no
> light sleep.
>
> **A 20 ms poll blocks light sleep.** The panel's BUSY wait polled at 20 ms against FreeRTOS's
> threshold of `CONFIG_FREERTOS_IDLE_TIME_BEFORE_SLEEP` = 3 ticks at 100 Hz = **30 ms**. It yielded,
> so the watchdog stayed quiet, but it never slept — holding the core awake at full clock through
> the single longest wait in the system. Now 100 ms.
>
> Two policies:
>
> | | Clock | Light sleep | |
> |---|---|---|---|
> | **Awake** | 160/160 MHz | off | five minutes after boot, and ten minutes after any console input |
> | **Frugal** | 80/10 MHz | on | otherwise |
>
> **80 MHz rather than 40**, chosen deliberately. 40 is the crystal frequency, so capping there
> would keep the BBPLL from starting — worth about a milliamp while awake. 80 keeps the PLL running
> and gives that back, and buys two things worth more: **WiFi works** (80 MHz APB is the radio's
> floor, below which it cannot operate at all, so the AP needs no separate policy or scheduled
> window), and so does USB Serial/JTAG. Against light sleep the milliamp is noise — at a 5 % awake
> fraction, 40 vs 80 is 124 days against 95, while sleeping vs not is 95 days against four.
>
> **⚠ Light sleep powers down the USB PHY**, and a board running it is unreachable: the port never
> comes back, because the awake windows are too short to re-enumerate. That is why the **grace
> period exists at all** — it is not a nicety, it is the only way in. Recovering a board without one
> cost an entire evening; see §8.
>
> **Supply detection is gone.** Three schemes were tried and each failed in a case that happens:
> evaluated on the redraw path it was fifteen minutes late; `CRATE` reads exactly `0.00 %/hr` for
> minutes after unplugging; and `usb_serial_jtag_is_connected()` never reports a disconnect at all.
> What replaced them is about a *person* rather than a cable — the grace period, and console
> activity.

---

## 8. Reference implementation

**Retired.** `DFRobot_AS3935_Lib.py` and `deep_demo.py` were the behavioural spec for §3–§4 and
have been deleted now that the port is complete and verified on hardware — the antenna self-test
reads 499–500 kHz through the ported registers, and disturber decoding drives the §4.2 auto-tune.

`src/as3935.rs` is the record. It carries the register map, the init sequence, and every operation
the reference had, including the ones not yet on the wake path (`set_min_strikes`,
`clear_statistics`, `power_down`) — ported before deletion precisely so that nothing lived only in
a file about to be removed.

Four things were deliberately **not** carried across, each noted at its call site:

1. **I2C reads inside the IRQ callback.** Fine in MicroPython, not allowed on esp-idf — an ISR may
   not block, and the handler has to wait ≥3 ms first. The ISR here only notifies.
2. **Distance as a raw 6-bit field.** `0x01` means *overhead* and `0x3F` means *out of range*;
   charting either as kilometres poisons every average built on it.
3. **Energy as a float.** `raw / 16777` on a `riscv32imc` core with no FPU is a software routine on
   every strike. Intensity is fixed-point here.
4. **The interrupt reason as 1/2/3.** The register holds `0x08`/`0x04`/`0x01`; both numbering
   schemes appear in this document, and mixing them turns lightning into "unknown".

The display/UI was designed fresh — there was no MicroPython UI to port.

---

### ⚠ Recovering a board that will not flash

The usual ESP32 advice — hold BOOT, replug USB, release BOOT — **does not work
on this board**, and the reason is worth writing down because the symptom looks
like dead hardware.

**The panel has its own 2000 mAh cell.** Unplugging USB therefore does not
power-cycle anything: the chip keeps running, never leaves reset, and never
samples the BOOT strap. The device stays enumerated, the port stays present, and
`espflash` hangs at "Connecting..." indefinitely.

With the board powered, the sequence that works is:

1. press and hold **BOOT**;
2. while holding, press and release **RESET**;
3. release BOOT.

**The reliable way out, when the board will not answer at all:** flash a
known-good firmware that never sleeps — **MicroPython for the ESP32-C3** is what
worked here — and come back afterwards. It holds the USB PHY up permanently, so
the port goes stable and the whole timing race disappears; `esptool erase-flash`
then has all the time it needs. This beats any amount of retrying against a
board whose USB windows are a few seconds long.

Note also that **`espflash` could not connect to this board at all** across
dozens of attempts and every `--before` combination, while a bare
`esptool erase-flash` connected first time. Use esptool to get in; espflash is
fine for writing once the chip is in the downloader.

Easier on this build: the shield board carries a **battery ON/OFF switch**.
Switch the cell OFF, unplug USB, then plug USB back in while holding BOOT — with
the cell disconnected the chip genuinely loses power, so the strap is sampled on
the way back up. That switch is the reliable recovery on this hardware and the
first thing to reach for.

This is needed whenever a build with **light sleep** enabled is running, because
light sleep powers down the USB PHY and takes both the console and the flasher
with it. That is also why §7's battery policy is gated on the fuel gauge rather
than applied unconditionally: an always-frugal build is an unflashable one.

## 9. Open items to verify

1. **Battery** — *resolved and built.* MAX17048 at 0x36, version `0x0012`. Reports voltage, SOC and
   `CRATE`; two runtime predictions (the gauge's own, and one from a **learned cell range** widened
   by the midpoint rule and kept in NVS). Confirmed charging live at `+0.33 %/hr`.
2. **Confirm display pins + BUSY polarity** — *resolved by measurement, and the spec was wrong:*
   BUSY is **not** inverted on this board. See §6.
3. **Partial-refresh support** on this 7.5" V2 panel — *resolved, and the answer is no.*
   `epd-waveshare` 0.6's `Epd7in5` implements `update_partial_frame` as a literal
   `unimplemented!()`, so calling it does not degrade to a full refresh — it **panics**.
   Full-only, therefore, and that sets the whole refresh policy: a measured **3.8 s** of
   panel-busy per redraw, which is why §6's nominal 5 s cadence is unusable and why the
   screen is change-gated with a 30 s floor.

   If partial is wanted later it means writing UC8179 support rather than configuring it —
   the controller does support it, the crate does not. Worth it only if something needs to
   update faster than every 30 s, and so far nothing does: strikes and the mode button are
   the only things that redraw on demand, and neither is a fast-moving quantity.
4. **WiFi provisioning flow** — *open, and the only major piece left.* Intended shape: long-hold
   BOOT raises an AP and shows a **QR code** on the panel (`WIFI:S:…;T:WPA;P:…;;`, which phones join
   from the camera); a second hold takes it down. One config page for network, indoor/outdoor, chart
   span and API enable, plus `GET /strikes.csv` streaming the log — which a browser can render with
   auto-refresh, so monitoring needs no second UI.

   ⚠ It should be **off by default**. The AS3935 listens at 500 kHz, and a bench beside an access
   point saturated the 31-rung ladder in seconds; an always-on radio on the same board is the one
   experiment guaranteed to degrade the sensor. Note also that `esp-idf-svc` is currently absent on
   purpose — adding it links the WiFi and BLE stacks, roughly doubling the binary.
5. **DB retention policy** at capacity — *deferred, deliberately.* The log **stops when full** rather
   than wrapping: that is the only choice that cannot lose data already recorded, and at ~40 000
   records the question is years away. Note the charts self-clean (24 h / 7 d / 30 d windows) while
   the file does not.
6. **A real strike.** Everything below `Interrupt::Lightning` has run only on synthetic input. A
   piezo lighter cannot provoke one — the AS3935 validates a waveform against a lightning signature,
   so a spark raises a *disturber* by design, which is the chip working correctly. Hence the
   `strike` console command; hence also that the chip's own classification remains unverified.

---

## 10. Build order

1. ✅ `esp-idf-template` C3 project → blink + serial over USB-C.
2. ✅ I2C on GPIO6/7 → scan → confirm AS3935 @ 0x03; port the register driver.
3. ✅ Wire IRQ — on **GPIO21 (D6)**, not GPIO20; ISR-notifies pattern; decode reason → distance and
   intensity on lightning.
4. ✅ Storm logic + noise auto-tune, as a 31-rung ladder (§4.2). Pure logic, host-tested.
5. ✅ CSV logging on LittleFS; clock over the console rather than SNTP; **rings rebuilt from the file
   on boot**.
6. ✅ e-paper bring-up → UI, change-gated refresh, day/week/month charts.
7. ✅ Battery gauge; light sleep on the frugal policy.
8. ⬜ WiFi AP + config + `/strikes.csv` (§9 item 4) — the remaining work.

**Toolchain note:** built against **ESP-IDF v5.3.4** with `esp-idf-hal` 0.46, not Appendix A's
suggested 0.44/0.49. Two API drifts caught immediately: `esp_idf_hal::prelude` no longer exists, and
`.Hz()` needs a `FromValueType` import (the plain `Hertz(..)` constructor avoids it).

**Testing:** `cargo test` cannot run in this crate — it only builds for `riscv32imc-esp-espidf` and
every dependency pulls in ESP-IDF. Pure logic is exercised on the host instead; see
`tests/host/README.md`. 37 checks across the defence ladder and the history rings.

---

## Appendix A — Rust toolchain setup for the ESP32-C3

The C3 is RISC-V, so you use **stock Rust** — **no `espup`/Xtensa toolchain**. (Linux/Debian shown;
macOS notes inline.)

**1. System dependencies** (bindgen needs clang; the ESP-IDF build needs the rest):
```bash
sudo apt update && sudo apt install -y git wget flex bison gperf \
  python3 python3-pip python3-venv cmake ninja-build ccache \
  libffi-dev libssl-dev dfu-util libusb-1.0-0 clang libclang-dev pkg-config
# macOS:  xcode-select --install ; brew install cmake ninja dfu-util libusb
```

**2. Rust + build tools:**
```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh   # install rustup
. "$HOME/.cargo/env"
cargo install cargo-generate ldproxy espflash cargo-espflash     # scaffolder, linker shim, flasher
```
`espflash` flashes and opens the serial monitor; `ldproxy` is the linker wrapper the build calls.

**3. Serial access (Linux):**
```bash
sudo usermod -aG dialout $USER    # then log out / back in
```

**4. Scaffold the project** (interactive prompts):
```bash
cargo generate esp-rs/esp-idf-template cargo
#   name : lightning-terminal
#   MCU  : esp32c3
#   std  : yes (default)      ESP-IDF : v5.x
```
This writes the correct `.cargo/config.toml` (target **`riscv32imc-esp-espidf`**, runner
`espflash flash --monitor`), a `rust-toolchain.toml` that pins the right channel automatically,
`sdkconfig.defaults`, and a hello-world `main.rs`. You do **not** need your existing ESP-IDF —
`esp-idf-sys` manages its own copy.

**5. Build, flash, monitor** (C3 plugged in via USB-C):
```bash
cd lightning-terminal
cargo run          # = build + flash + serial monitor
```
The **first build is slow** — it downloads and builds ESP-IDF once (several minutes); subsequent
builds are quick. If the port isn't auto-detected, add `--port /dev/ttyACM0`.

**6. Crates you'll pull in as you go** (check crates.io for current versions — pin what the template
suggests):
```toml
esp-idf-hal       = "0.44"   # I2C, GPIO, SPI, interrupts, sleep
esp-idf-svc       = "0.49"   # WiFi, SNTP, NVS, filesystem (LittleFS/FAT)
embedded-graphics = "0.8"
epd-waveshare     = "0.6"    # 7.5" V2 (UC8179)
```

The whole loop is: `cargo generate` once, then `cargo run` to iterate — same rhythm as
`arduino-cli compile/upload`, just `cargo run` does both.
