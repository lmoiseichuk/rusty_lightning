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

> #### ⚠⚠ AS BUILT: `CRATE` was scaled 100x low, and the constant looked right
>
> `crate_centi_per_hour` is *hundredths* of a percent per hour, so one LSB is 0.208 %/hr = **20.8**
> of them = exactly `104/5`. The firmware used `26/125` — which is also exactly 0.208, the correct
> constant for the **wrong unit**. It produced %/hr and stored it in a field whose name promises
> centi.
>
> Nothing looked wrong at the site: the ratio is exact, defensibly derived, and matches the
> datasheet figure quoted above. Two symptoms, neither pointing here:
>
> * **Charging displayed as `idle`.** A real 6.24 %/hr taper showed as `0.06 %/hr`, and a genuine
>   0.208 %/hr trickle truncated to `0.00`. Noticed from the bench side first: a USB meter reading
>   1.16 W against a board drawing ~0.17 W is ~200 mA going somewhere, and the screen said idle.
> * **"Days left" almost never appeared.** The runtime estimate discards rates below 5 centi-%/hr as
>   too small to divide by; under the old scale that threshold was really 5 **%/hr**, a rate this
>   device never reaches, so the estimate was suppressed essentially always.
>
> Found by printing the raw register beside the decoded value — `CRATE 0x0001` decoding to
> `0.00 %/hr` is not rounding. Reconciled against the bench: raw 30 is 6.24 %/hr, ~125 mA on a
> 2000 mAh cell, consistent with the meter where 1.2 mA was not.
>
> **The `battery` console command exists because of this**, and prints raw registers beside decoded
> values for exactly this reason. Its first version read them separately and the two lines disagreed
> by a few hundredths — which defeats the purpose, since raw values are printed *so the arithmetic
> can be checked against them*. One read now feeds both.
>
> Item 1 above is also now built, ahead of any MAX17043: `battery::Trend` tracks cell voltage
> against a five-minute anchor, because `CRATE` is heavily filtered and takes minutes to respond to
> a supply change — so a freshly plugged-in charger reads as `idle` even with correct scaling. On a
> MAX17043 that trend would not be a fallback but the only answer.

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

> #### ⚠⚠ AS BUILT: the sensor was hearing our own I2C bus — **100 kHz puts a harmonic on 500**
>
> The device detected nothing through two storms. The AS3935 receives at **500 kHz**, with a Q worth
> tuning to ±3.5 %. The I2C bus ran at **100 kHz**, and **5 × 100 kHz = 500 kHz exactly** — carried on
> wires that run to the sensor's own package. The bus rate was never a neutral choice on this board;
> it parked an interferer in the middle of the passband.
>
> Measured at maximum sensitivity, same board, minutes apart:
>
> | bus | `nf 0, wdth 0, srej 0` |
> |---|---|
> | 100 kHz | **8–10 `NoiseTooHigh` per second, continuously** |
> | **200 kHz** | **none at all** |
>
> **What this explains, and what it invalidates.** The "ambient noise floor" the chip kept reporting
> was self-inflicted. Every configuration that looked quiet at `WDTH 2` was quiet because the
> watchdog gate was rejecting the interferer — *and with it anything else weak enough to need that
> gate open, which is exactly what a distant strike is*. So the device was not insensitive by
> misconfiguration; it was jammed, and then gated against its own jamming.
>
> Ruled out along the way, each by measurement rather than argument:
>
> | suspected | test | result |
> |---|---|---|
> | MCU clock radiating | 160 MHz vs 80 MHz | 6.8/s vs 6.5/s — no effect |
> | USB charger / supply | run on battery, `-1.24 %/hr` | unchanged |
> | e-paper panel shielding the antenna | antenna self-test | 499 kHz, in tune — not detuned |
> | dead or faulty sensor | self-test drives the LC tank | oscillates on frequency; coil intact |
>
> The mounting suspicion in particular deserves retiring explicitly: the sensor is stuck to the back
> of the panel, which looked like an obvious culprit, and nothing ever supported it. Replacing the
> sensor — the next step under consideration — would have put a working part on a bus that was
> jamming it.
>
> **Why 200 and not 400.** 400 kHz is what the reference used and it fails outright here: the boot
> scan finds the MAX17048 at 0x36 and nothing at 0x03, `FATAL: no AS3935 answered a reset`.
>
> Which part drops out is the clue, and §2.1's own cascade explains it:
>
> ```
> XIAO C3 ── STEMMA QT ──► MAX17048 ── STEMMA-to-Gravity ──► AS3935
> ```
>
> **The sensor is the far end of the chain.** The gauge sits between it and the driver, so the
> AS3935 sees every segment's capacitance and the slowest edges on the bus — it is the first device
> that a rate increase should break, and it is the one that broke. The leads are well soldered and
> under 10 cm, so this is the topology rather than sloppy wiring, and it predicts that reordering
> the chain — sensor first, gauge behind it — would move the limit. Untested, and not worth testing
> for its own sake, because the rate was only ever wanted to dodge the harmonic.
>
> 200 kHz dodges it without asking: 500/200 = 2.5, so no harmonic lands on the passband, at 2× the
> rate known to work rather than the 4× known not to.
>
> **The general rule this board earns:** on any design carrying a narrowband receiver, every periodic
> digital signal near it is a candidate transmitter, and the ones to check first are the ones whose
> harmonics are integer multiples of the passband. A 100 kHz bus beside a 500 kHz receiver is a
> five-times multiplier, and nothing in either datasheet mentions the other part.

---

## 4. Behaviour

### 4.1 Indoor/outdoor
Set the AFE gain at startup (`0x24` indoor / `0x1C` outdoor). Selectable via config.

### 4.2 Noise-floor auto-tune — one packed 7-bit point
Asymmetric, per the reference: **any** disturber/noise IRQ in the ~1 s processing batch → **+1
immediately**; **60 s with no events** → **−1**. Quick to defend, slow to relax.

> #### ⚠⚠⚠ AS BUILT: the ladder is gone — the two volume knobs are one 7-bit number
>
> Everything below this block is the history of a state machine that no longer exists. It is kept
> because the *arguments* in it are still the arguments — what each register costs, and why the
> order they move in is the whole design. What changed is that the order is no longer enforced by
> code. It falls out of a bit layout.
>
> **The defect that forced the rewrite.** `new_ladder` stored *sensitivity* — `cur = max` meaning
> most sensitive, each writer subtracting on the way to the bus — while the rest of the crate was
> written in *defence*. So "this window was noisy, defend harder" raised `cur`, which made the
> receiver **more** sensitive, which produced more noise, which called it again. Observed as a device
> pinned at 70 % hearing nothing, then sliding to 20 % on a rising event count: silence ran the same
> loop the other way and walked the machine deaf, and because the cursor never retreated it could not
> come back. Two units were suspected and one was replaced before the fault was found to be
> configuration, both times.
>
> The fix was not to flip the sign. The registers are bit fields in two bytes, so the entire
> tunable state is 11 bits:
>
> ```
>   bit   6  5  4 | 3  2  1  0
>        NF_LEV   |    WDTH
>        (3 bits) |  (4 bits)
> ```
>
> The gauge weights rescaled with it — 10/40 for `NF_LEV`/`WDTH` became **20/80**, so the two
> survivors still span 0–100 % of harm rather than topping out at half.
>
> **Two registers have left the space**, taking the point from 13 bits to 7. Neither is a volume
> control, and the tuner could damage the device's purpose with both — in opposite directions.
>
> `MIN_NUM_LIGH` suppresses strikes outright until N arrive, so every notch hides the very events the
> device exists to report. Pinned at 1.
>
> `SREJ` rejects short man-made impulses and is the only knob that does. **The walk refunds the least
> valuable field first, and by the sensitivity weighting that is SREJ** — so every quiet spell walked
> it to zero, and at zero the chip validated an electric hammer on a neighbouring roof as lightning:
> 503 false strikes in three and a half hours (§9 item 6). Now a *setting* — default **1**, between
> the reference driver's 0 and the datasheet's 2, `srej <0-15>` on the console, stored in NVS,
> written at boot and on change.
>
> The spread it was chosen against, measured here: **0** gave 503 false strikes in 3.5 h; **2** gave
> five in eight hours but missed 5–6 real strikes an hour at 10+ km; **8** silenced everything
> man-made. One is a judgement between them rather than a measured optimum, on the reasoning that a
> detector reporting hammers is useless where one missing distant storms is merely limited. `session::apply` does
> not touch it, which is the actual fix: **what the tuner cannot write, it cannot spend.**
>
> #### ⚠⚠ The sweep could not have caught this, and that is a flaw in its objective
>
> `calibrate` searches for the most sensitive point that stays **quiet**. In a quiet room `sr 0`
> scores perfectly, because a knob that rejects nothing has nothing to reject. **Quiet is not
> correct** — a setting that admits every impulse and a setting that admits none both measure as
> silent when the room is silent. Removing SREJ from the space stops this particular consequence; the
> objective is still "minimise events", where what is wanted is "maximise true detections", and those
> differ whenever the interference is intermittent. Unresolved.
>
> **Ordering is free, because binary search resolves high bits first.** A bisection over 0..=127
> probes 64 first, which is a decision about `NF_LEV` alone; the last probes decide `WDTH`. So the one knob that cannot reject a strike is settled coarsely up front, and the one that
> can silence a storm moves last and least — the exact property the cursor, the per-register strides,
> the mixed-radix position and the never-retreat rule were all built to enforce by hand. All four are
> deleted. So is the original bug: the fields **are** the register values, so there is no second
> convention left to disagree with.
>
> A full sweep is **7 probes** rather than hundreds, which is what makes a 60 s probe window
> affordable. Measured on this board three times (before `MIN_NUM_LIGH` left the space, so the
> `min strikes` column below records what those sweeps chose rather than what a sweep can still
> change):
>
> | sweep | probe window | settled | min strikes |
> |---|---|---|---|
> | 1 | 10 s | 448 — `nf 0, wd 7, sr 0, ms 0` | 1, reports every strike |
> | 2 | 10 s | 448 — same | 1 |
> | 3 | 60 s | **478** — `nf 0, wd 7, sr 7, ms 2` | **9** |
>
> **The longer window settled deafer, and that is the predicate rather than the room.** A probe asks
> `count == 0`, and a longer window has strictly more chances to catch one stray event — so probes at
> 463, 471 and 475 each returned a single event, and each pushed the search one notch further. The
> sweep spent spike rejection and then reached `MIN_NUM_LIGH` on the strength of **one to two events
> per minute**, having rejected other points at 100+ per minute by the identical verdict.
>
> The bit ordering did its job throughout: `NF_LEV` was decided in probes 1–4, `WDTH` by 8, `SREJ` in
> 9–11, and `MIN_NUM_LIGH` only in 12–13 — last, as designed. But **a structural protection cannot
> survive a test that never accepts an answer**: once the cheap registers are exhausted, a
> zero-tolerance predicate forces the search into the expensive ones anyway.
>
> This is left as it stands, deliberately, because the ±1 walk repairs it and repairs it in the right
> order. `relaxed` steps the *least significant non-zero field*, which is `MIN_NUM_LIGH` first:
>
> ```
> 478  ms 2 (wait 9)   settled
> 477  ms 1 (wait 5)   after one quiet minute
> 476  ms 0 (wait 1)   after two — reporting every strike again
> 472  sr 6            then spike rejection, a notch a minute
> 448  sr 0            after nine — what the 10 s sweeps found
> ```
>
> So a calibration that overpays is refunded most-destructive-first within two minutes of quiet.
> Observed doing exactly that on the run above. **The signature to watch for** is a room with a
> persistent one-event-per-minute floor: the climb test is `window_events > 0`, so such a room never
> earns a quiet minute and would ratchet upward indefinitely. This board is not one — the same points
> read 0 and 1 on different probes, so it oscillates around the boundary instead.
>
> Two further defects fell out of the new shape, both found by host checks before hardware saw them:
>
> * **`SREJ`'s cap of 11 cannot coexist with a dense space.** Clamping on construction folded four of
>   every sixteen steps back onto 11, so a tuner walking up from raw 47 landed on 48, clamped to 44,
>   and cycled 44–47 forever — never able to pass spike rejection 11. The cap is unnecessary anyway:
>   the search bisects for the *lowest* quiet point, so it prefers weak spike rejection unprompted.
>   The judgement the cap encoded is now a property of the search rather than a wall in the space.
> * **The packed number is not monotonic in defence at a borrow.** Stepping "down" from the settled
>   448 (`wd 7, sr 0, ms 0`) gives 447, which is `wd 6, sr 15, ms 3` — from reporting every strike to
>   waiting for sixteen. The device then walked on down, because a chip waiting for sixteen strikes
>   hears nothing, which reads as "no noise, relax". Relaxing therefore steps the **least significant
>   non-zero field**, not the number: 448 → 384, gentler in one register and unchanged in the rest.
>
> **The sweep is driven by the main loop**, one probe per measurement window, rather than being a
> call that owns the loop for its duration. That is not a display detail. A blocking sweep could not
> use `listen`'s event counter, so it grew its own; could not use the ordinary redraw, so it grew its
> own progress screen; and left the panel on whatever was there when the command arrived — after a
> fresh boot, the logo, for fourteen minutes, which is indistinguishable from a device that hung
> during start-up. Driven from the loop, a probe is an ordinary window whose verdict goes to the
> search instead of to the ±1 walk, and the ordinary gauge shows the point under test.
>
> #### The walk between calibrations
>
> One decision a minute, on the same 60 s window a probe uses, and it is **asymmetric in three
> different ways** — each one measured rather than assumed.
>
> **Quiet is a rate, not zero.** `noise_per_min <= 12` by default, stored in NVS and settable as
> `calibrate <seconds> <per-min>`. Twelve a minute is one every five seconds. The measurement that
> forced this: a microwave door swing on this board took the band from **6/min to 94/min**, and the
> settled operating point reads 0–1/min against 13–17/min one notch below it — so the threshold sits
> in a genuine gap, where a literal zero sat on top of the sampling noise.
>
> **Up is proportional, down is one notch.** The number of notches is `noise_per_min /
> quiet_per_min` — "how many times over the line is this" — which needs no constant of its own and
> rescales automatically with the room's threshold. At one notch a minute the machine could not
> answer a real step change: observed at 102/min, it moved `323 → 324 → 325` over three minutes,
> thrashing `MIN_NUM_LIGH` in the bottom bits while `WDTH` — the knob that would actually have
> stopped it — sat untouched. It saturates naturally: a fully jammed band here is ~480/min, which is
> 40 notches against a ladder exactly 40 notches deep, so the worst case is "fully deaf in one
> minute" rather than an unbounded number nobody has budgeted for. Relaxing stays at one notch,
> because a storm's first strike should not arrive into a receiver that spent the afternoon
> sprinting back toward a floor it will have to climb again.
>
> **Up and down use opposite cost orders, and neither is `raw ± 1`.** Climbing spends the cheapest
> knob first — `NF_LEV`, `WDTH`, `SREJ`, `MIN_NUM_LIGH` — and relaxing refunds the dearest first, in
> the reverse order. Walking the packed number instead moves the *bottom* bits, so `raw + 1` answers
> a noisy minute by waiting for five strikes, and `raw - 1` borrows across three fields at once
> (448 → 447 turns `sr 0, ms 0` into `sr 15, ms 3`). Observed refunding correctly on hardware:
> `449 → 448` gave min strikes back first, then `384`, `320` walked the watchdog down.
>
> **A window that heard a strike never raises the defence.** A nearby strike is not a clean impulse
> — it throws harmonics that arrive as disturbers, so a storm close enough to matter looks like a
> noisy band to a counter that cannot tell them apart. Climbing on that would deafen the device at
> the moment it exists for, and each notch of `MIN_NUM_LIGH` would then hide the following strikes
> too: a loop that closes on itself. Such a window **holds** rather than relaxing, because it
> genuinely was noisy — this is a refusal to escalate, not evidence of quiet.
>
> A never-calibrated device starts from the two volume knobs mid-range with `SREJ` and
> `MIN_NUM_LIGH` at their most sensitive: neither of those is a volume control, so neither has any
> business being pre-set to a guess.

> #### ⚠⚠ AS BUILT: the ladder was 31 rungs and is now 7 — the extra 24 rejected lightning
>
> The three registers below are not three grades of the same thing, and treating them as one integer
> was the error:
>
> * **`NF_LEV`** is a *noise-floor* gate. It decides when the chip complains the band is noisy, and
>   **cannot reject a lightning waveform** — which is why sweeping its full range is safe, and why
>   the MicroPython reference sweeps exactly this and nothing else.
> * **`WDTH`** is the watchdog *amplitude* gate. Raising it discards weaker arrivals, distant
>   strikes first.
> * **`SREJ`** compares the signal against the chip's lightning *waveform template*. Raising it
>   discards anything imperfectly shaped.
>
> So rungs 8–31 tuned only knobs that can throw lightning away, and the climb rule guaranteed they
> would be reached: +1 for any batch with activity, −1 only after a *full minute* of total silence.
> A storm never grants that minute. Neither did the 100 kHz bus harmonic above, which produced
> batches of noise on a clear day — so the ladder ratcheted to maximum rejection and stayed there.
>
> `MAX_LEVEL` is now the noise floor's own range. `WDTH` and `SREJ` hold at the power-on defaults
> the reference leaves them at. Deliberate over-sensitivity is still reachable by hand — `sensitive
> on` — which is the right shape for it: a visible, temporary act rather than an automatic ratchet.
>
> **Is 31 worth restoring now the harmonic is fixed?** No, and the reason is stronger than the one
> that prompted the cap. The cap was made urgent by constant self-inflicted noise; with that gone
> the ladder sits at 0 and would rarely climb at all. But the *argument* never depended on the noise
> source — `NF_LEV` is simply the only one of the three that buys rejection without spending
> sensitivity. Extra headroom bought by going deaf to lightning is not headroom, and this device's
> own note below already says so: a detector that hears nothing is worse than one that hears noise,
> because the noise is at least visible.

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
> This is the reasoning as it stood, and the block above is what became of it. The saturation
> complaint it was solving is real — `NF_LEV` genuinely does run out beside an access point — but
> walking into `WDTH` and `SREJ` answered "the detector has stopped defending" by making it stop
> detecting. The right response to a band too noisy for the chip is to say so, which the
> *"cannot defend further"* line already did.
>
> **The step is per batch, not per event.** An earlier version escalated per interrupt and saturated
> the whole ladder in under two seconds — a counter racing the interrupt rate, not tuning.

### 4.3 Storm tracking
Per lightning strike collect `distance`, `intensity`, `score`. Estimate the situation from the trend:
coming in (distance ↓), moving out (distance ↑), stronger (same distance, higher intensity), weaker,
**aggravating** (closer *and* higher score), **fading** (farther, lower score).

> #### ⚠⚠ AS BUILT (0.6.0): the sensor's own statistics are cleared at storm end
>
> The AS3935 does not estimate distance from one strike. It accumulates statistics over a storm and
> reports the distance to the storm *head*, and `CL_STAT` — register `0x02` bit 6, edge-triggered
> high–low–high — is the only way to discard them. `As3935::clear_statistics` implemented that
> correctly and **nothing called it until 0.6.0**: the only occurrence of the name in the source was
> its own definition, carrying `#[allow(dead_code)]` and a comment saying it was waiting for this
> section to be built.
>
> **A reboot is not a substitute.** `find` issues `PRESET_DEFAULT`, which restores registers; the
> datasheet treats clearing the statistics as a separate operation. So the estimator accumulated
> from the sensor's first power-on onward, through every power cycle since.
>
> `session::StormWatch` steps once per tuner window and clears after **thirty consecutive windows
> with no new strike** — thirty because that is the public-safety rule for calling a storm over, so
> the number is borrowed rather than invented. A bus error deliberately does not mark the lull
> handled, so the next window retries rather than recording a success that did not happen.
>
> **Cleared at every discontinuity, not just at storm end (0.8.0).** The statistics describe the
> instrument that gathered them, so anything changing what the energies *mean* invalidates the whole
> accumulation: boot, a gain change (indoor is ~4× outdoor, so every stored figure was measured on a
> different scale), the start of a calibration sweep and each of its probes, a point set by hand, and
> the sensitivity override going on or coming off. Not the ±1 walk — one notch a minute barely moves
> the receiver and does not move the storm, and clearing on every step would leave the estimator with
> nothing to estimate from.
>
> **A run of nearest-bin readings also resets it (0.8.1).** Three in a row is ambiguous in a way no
> single reading can resolve — the storm is overhead, or the estimator has stopped tracking — and a
> false trigger is free: if the cell really is overhead the estimate rebuilds and says so again. It
> fires **once per run** and re-arms only on a kilometre reading, because a plain counter would clear
> every third strike of a genuinely overhead cell, and because a freshly cleared estimator has no
> data, so falling back to the nearest bin would cause the very condition that triggers clearing.
>
> #### ⚠ What 2026-08-13 did and did not establish
>
> **Weaker evidence than it first appeared.** Of 27 strikes, **three** reported a kilometre figure
> and 24 the nearest bin. The three fell in a 70-second window (16:06:52–16:08:02) directly after a
> boot cleared the statistics; everything from 16:08:22 onward was `overhead`.
>
> That is consistent with "clearing works, then the estimate saturates within a few strikes" — and
> equally consistent with the estimate simply being **correct**, because the cell was overhead for
> most of that hour: visible, at a five-second pace, and confirmed from outside. The two cannot be
> separated from this record.
>
> What remains genuinely unexplained is 2026-08-12: 917 consecutive nearest-bin readings while the
> thunder was delayed 15–30 s, which is 5–10 km. That is the anomaly the clearing was written for,
> and it is still the thing a future storm has to confirm.
>
> **One misreading to avoid**, recorded because it wasted time: `closest` on the screen and in
> `status` is the **minimum over the last hour**, not the current distance. A receding storm keeps
> reporting `nearby < 5 km` for an hour afterwards and is right to.

> #### ⚠⚠ AS BUILT (0.7.0): strokes are merged into flashes
>
> One lightning **flash** is normally three to four **return strokes** down the same ionised
> channel, tens to hundreds of milliseconds apart, and the AS3935 validates and reports each one.
> So a device counting interrupts counts *strokes*, and calls them strikes.
>
> That is what made 2026-08-12 read **930** when the sky produced perhaps a third of that. The check
> is the ear: flashes were audible every 10–30 s — 2–6/min — while the log recorded a mean of
> 4.4/min with a peak of 15. The log was counting something finer than what a person sees.
>
> `session::Merger` folds every stroke inside a **merge window** into one record: energies summed,
> distance averaged over *measured* kilometres, and a `strokes` column saying how many went in.
> Default 1000 ms, set by `merge <ms>` and stored in NVS, `0` to switch merging off and log every
> stroke as before.
>
> **The window runs from the first stroke, not the last.** A sliding window would let a long enough
> train merge without limit, so a storm directly overhead could collapse into one record hours long
> — the opposite of the intent. The ceiling is 10 s because the busiest minute of 2026-08-12
> averaged a strike every four seconds, so a window near that would merge genuinely separate flashes.
>
> Two deliberate asymmetries:
>
> * **`Totals::strikes` counts flashes; `Batch::strikes` still counts strokes.** The batch figure is
>   §4.2's evidence that lightning is present, and the strike-hold rule wants all of it; the totals
>   figure is what a person reads.
> * **Overhead is not averaged into the distance.** With no measured kilometres in the window,
>   overhead wins if any stroke reported it — folding it in as a number is the bug `history::Bucket`
>   documents at length.

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
> timestamp,iso_local,distance_km,energy_raw,intensity_milli,score_milli,simulated,strokes
> 1785727634,2026-08-02 23:27:14,6,50331,3000,500,0,1
> ```
>
> Two columns were added after the fact, both to remove an ambiguity the file could not otherwise
> answer about itself. **`simulated`** distinguishes a `strike`-injected record from a detected one,
> because four records of unknown provenance once made "has this device ever seen real lightning?"
> unanswerable. **`strokes`** (0.7.0) says how many return strokes §4.3's merge window folded into
> the row — without it a merged record is indistinguishable from a single strike, and `energy_raw`
> is a *sum* across them.
>
> **A format change never reaches an existing log.** The header is written only to an empty file, so
> a device upgraded in place keeps the header it was created with while new rows carry the new
> columns. `Log::ensure_header` detects the mismatch at boot and says so rather than rewriting the
> file — rewriting would discard records to correct a label. `clear` is the deliberate way to start
> again, and a log dumped beforehand keeps everything.
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

> #### ⚠⚠ AS BUILT: "nice-to-have" understates it by an order of magnitude
>
> **Measured at the USB port with a power meter:**
>
> | Policy | Measured | Implied |
> |---|---|---|
> | **Awake** — 160 MHz, no light sleep | 0.170 W ≈ 33.5 mA | ~2.5 days |
> | **Frugal** — 80 MHz + light sleep | 0.0127–0.0152 W ≈ **2.5–3.0 mA** | **~28–33 days** |
>
> **13× lower**, and this is the first end-to-end proof that light sleep actually *engages*.
> `esp_pm_get_configuration` cannot distinguish "configured to sleep" from "configured to sleep and
> blocked by a 20 ms poll" — it reports the same config either way, which is exactly how the bug
> below survived. 2.5 mA cannot happen unless the core is genuinely sleeping between interrupts.
>
> The figures are taken at the USB port, so they include the charge controller's quiescent draw and
> the panel board, not just the MCU. That makes them pessimistic against a cell-side measurement and
> they are still the ones to trust: earlier estimates of *four days against eighty* were modelled
> from datasheet currents for the MCU alone.
>
> Two facts drive the improvement, and the first was a bug:
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

> #### ⚠⚠ RETRACTED: the "30 ms settle" was based on a line that is not in the reference
>
> **This block previously claimed** that the reference reads the reason register after
> `utime.sleep(0.03)` — "30 ms, ten times what its own comment claims" — and the settle was raised
> from 3 ms to 30 ms to match. **No such line exists.** Recovered from `a4e0b42bd67c^`, the file
> says:
>
> ```python
> def getInterruptSrc(self) -> int:
>     utime.sleep_ms(3) #wait 3ms before reading (min 2ms per pg 22 of datasheet)
> ```
>
> `sleep_ms(3)` is three milliseconds and the comment agrees with the code. There was never a
> divergence. The reasoning built on it — that 3 ms leaves the chip mid-classification so a strike
> presents as a disturber — was plausible, was never tested against lightning, and rested on a
> quotation that was not there.
>
> Restored to **3 ms**, the reference's value and the datasheet's 2 ms minimum plus margin. Measured
> consequence of the 30 ms version: through half an hour of audible thunder the device produced no
> interrupt of any kind.
>
> **The lesson survives its own example, and is sharper for it.** A behavioural spec is what the code
> *does* — and checking that means reading the file, not remembering it. This block was itself the
> failure it warned about: an assertion about executed behaviour, written from recollection, that
> then justified a change and stood for four commits. The reference is in git; quote it from there.
>
> #### The full re-audit that lesson prompted
>
> The upstream bundle (Arduino C++, MicroPython and Raspberry Pi ports) was re-cloned and **every
> register operation compared against `src/as3935.rs`**, on the theory that if one comment lied,
> others might. Energy assembly (`(MMSB & 0x1F) << 16 | MSB << 8 | LSB`), distance masking, noise
> floor, watchdog, spike rejection, min-strikes encoding, clear-statistics pulsing and tuning-cap
> conversion all match exactly. Two findings, both the same shape:
>
> 1. **The 30 ms settle above.**
> 2. **`0x08` bit 5 is `DISP_TRCO`, not `DISP_SRCO`.** The reference pulses it during power-up under
>    the comment `#set DISP_SRCO to 1`, while its own `setIrqOutputSource` maps `0x20`→TRCO,
>    `0x40`→SRCO, `0x80`→LCO — matching the datasheet. The *value* was ported correctly, so RCO
>    calibration has always worked; only the name was wrong. That is a live trap rather than a
>    cosmetic one: checking `MASK_DISPLAY_SRCO = 0x20` against the datasheet invites "correcting" it
>    to `0x40`, which pulses the wrong oscillator, leaves the RCO uncalibrated, and breaks strike
>    validation into exactly the disturbers-but-never-lightning symptom — with nothing looking wrong.
>    Now named `MASK_DISPLAY_TRCO`, with the reasoning at the constant.
>
> The bundle was then deleted again. `src/as3935.rs` remains the record.

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
6. **A real strike** — ***reopened on 2026-08-14, and the count is unknown.*** This item was marked
   answered on the strength of 909 records in three and a half hours. It should not have been.

   > #### ⚠⚠⚠ The device was reporting man-made impulses as lightning
   >
   > On 2026-08-14 it logged **503 strikes between 07:00 and 10:36 with no lightning within range** —
   > confirmed from outside and against the public network. Roofers next door, electric hammers.
   >
   > The overnight records are **statistically indistinguishable** from them, and from the storm at
   > 22:00 the night before that was directly observed:
   >
   > | period | n | energy median | gap median | gap CV |
   > |---|---|---|---|---|
   > | 22:00–00:00, observed real | 299 | 317,789 | 16 s | 1.04 |
   > | 00:00–06:00 | 651 | 290,366 | 21 s | 1.12 |
   > | 07:00–10:36, known false | 503 | 285,990 | 18 s | 0.96 |
   >
   > A coefficient of variation near 1.0 is Poisson — random arrivals — in every period including the
   > one that was a hammer. **Nothing in the record distinguishes them**, so the log cannot be
   > labelled, and the 909 from 2026-08-12 are in the same doubt.
   >
   > **The cause was a setting this project chose.** `SREJ` is the only knob that rejects short
   > man-made impulses, and §4.2's relaxation refunds the least valuable field first — which by the
   > sensitivity weighting is SREJ. Every quiet spell walked it to zero. Measured directly: setting
   > `sr 8` by hand stopped the false strikes dead, none in five minutes where they had been arriving
   > every 10–40 seconds, and the walk then refunded it 8→7→6→5→4 at one notch a minute.
   >
   > It also explains every `Overhead` reading without appeal to stale statistics: a hammer twenty
   > metres away genuinely is inside the nearest bin.
   >
   > Fixed in 0.10.0 — SREJ left the tuning space (§4.2). What this item now needs is a storm whose
   > records can be **labelled**, which means §9's Blitzortung cross-reference rather than another
   > count.

   The three defects found while chasing a first strike — the 100 kHz bus harmonic on the 500 kHz
   passband (§3); the auto-tune climbing into `WDTH`/`SREJ` (§4.2); the IRQ settle raised to 30 ms on
   a misquotation (§8) — were each real and each is fixed. None of them is validated by a record that
   might not be lightning.

   **The counts cross-check against the sky.** The device logged 4–9 strikes a minute while a flash
   was audible outside every 10–30 s. A single flash carries three to four return strokes, so a
   detector counting individual strokes *should* read a small multiple of the audible flash rate —
   and it read a conservative one. That is an independent check that these are detections rather
   than noise passing validation.

   **§4.2's strike-hold rule is what kept it listening, and it was observed doing so.** At 208–299
   events/min, far over the 120/min this room needs, the tuner would ordinarily have climbed one to
   two notches a minute and deafened itself. Every window containing a strike held instead, pinning
   the point wide open for the duration:

   ```
   tune: holding at 0/2047 (0%) -- 7 strike(s) this window, 208/min
   ```

   The failure mode it guards against is real and was avoided by a margin: a storm throws disturbers
   ahead of itself, so a tuner that climbs on those is deaf before the first strike arrives, and the
   hold rule can never arm.

   **2026-08-13 added a second storm, and a caution.** 27 strikes over an hour from a cell that was
   *overhead* — visible, five-second pace — against 917 the night before from one at 5–10 km. The
   detection rate collapsed by two orders of magnitude, and the two settings that recovered anything
   were **outdoor gain** and leaving the auto-tune alone. That is §6's saturation warning in the
   field: a strike close enough to see overloads the front end, fails the chip's waveform validation,
   and is counted as a **disturber**. 103,000 disturbers in an afternoon were not noise — they were
   the storm, arriving too strong to classify. So the instrument is at its worst directly underneath,
   which is the opposite of the intuition, and no software lever fixes it: outdoor is the lowest gain
   the AS3935 has.

   **Three lessons about observability, all paid for the same afternoon.** `batch.strikes` was not
   printed, so a decoded Lightning interrupt was invisible unless it survived all the way to a
   record — which made "the chip is rejecting them" and "something downstream is losing them"
   indistinguishable from the console. `mode` from the console did not persist, so a reset silently
   restored the gain the *button* had last chosen. And two host processes reading one serial port
   split the byte stream between them, which removed strike lines from the log and was twice
   diagnosed as the firmware losing strikes. Only the first two were the device's fault; all three
   cost the same hour.

   **What is NOT established is the chip's distance estimate.** All 909 reported the nearest bin,
   with no variation as the cell approached, sat overhead and departed, while thunder was audibly
   5–10 km away. Inside that single bin the energy field spanned **243×** (2,126 to 515,563) at 49 %
   of full scale — so the front end is neither saturating nor usable as a distance proxy, because
   strike intensity varies more than distance does. The decode was checked against the reference
   driver and is identical. The cause is believed to be §4.3's statistics, never cleared before
   0.6.0; that is a hypothesis with a test attached, not a diagnosis.

   Also established: the sensor is electrically sound (self-test drives its LC tank and reads
   499 kHz), the IRQ wire is confirmed, and the log distinguishes injected records from real ones by
   a `simulated` column — added because four historical records of unknown provenance made this very
   question unanswerable from the device's own data. Those four are now identifiable in the log by
   exactly that: they are the records with a kilometre distance and no provenance column at all.

---

## 10. Build order

1. ✅ `esp-idf-template` C3 project → blink + serial over USB-C.
2. ✅ I2C on GPIO6/7 → scan → confirm AS3935 @ 0x03; port the register driver.
3. ✅ Wire IRQ — on **GPIO21 (D6)**, not GPIO20; ISR-notifies pattern; decode reason → distance and
   intensity on lightning.
4. ✅ Storm logic + noise auto-tune (§4.2 — now one packed 7-bit point bisected in 7 probes; it was
   a 31-rung ladder, then 7, and the state machine in between had the sign inverted). Pure logic,
   host-tested.
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
`tests/host/README.md`. 83 checks across the packed defence point and the history rings.

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
