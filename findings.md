## How to read this

This is working state for the analysis pass of **2026-08-28** — gitignored, never
committed. Each finding has an ID, a status, a location and an effort guess
(S = small, M = medium, L = large). Fold re-analysis into the tables; a finding
lives in exactly one table, so re-verifying it means editing its row, not
appending a note elsewhere.

The older narrative entries (clock skew, the disturber tuner) are preserved
below their own headings, resolved and marked as such.

---

## Open

### Bugs — will misbehave on hardware

| ID | What | Where | Effort |
|---|---|---|---|
| L-B1 | **`now_ms()` is a `u32` that wraps at ~49.7 days and freezes the loop.** `now_ms()` is `(esp_timer_get_time()/1000) as u32` (`main.rs:413-415`). Every interval compares against a stored sample via `.saturating_sub`; after the wrap the stored sample is near 2³² and the delta saturates to 0 forever, so `listen.rs:226` takes the `continue` branch permanently — no processing, no tuner, no screen, no log sync — while the ISR still posts notifications so the device looks alive. A plain `-` at `listen.rs:378` underflows (panic in debug, reboot-loop in release). | `main.rs:413`, `listen.rs:226,250,378`, `screen.rs:134`, `battery.rs:668` | M |
| L-B2 | **Day-chart time axis is mislabelled 3×.** `ChartPeriod::bucket_minutes(Day)` returns 15 (`ui.rs:975`) but the day chart draws from `history.day`, a `Ring<288>` of **5-minute** buckets (`FINE_MINUTES=5`). So `per_tick = 360/15 = 24` buckets/gridline = 120 real minutes labelled `-6h`, and the `-12h`/`-18h` labels point at columns the 6h40m window does not hold. The claim that they are "asserted equal in `tests/host/history.rs`" (`ui.rs:969-972`) is false — that test only checks `FINE_MINUTES == 5` and never links the UI constant. | `ui.rs:975`, `ui.rs:1048-1089`, `tests/host/history.rs:158-168` | S |
| L-B3 | **`sensitive on` leaves the gauge lying.** `effects.rs` sets `tuning.frozen = true` and programs the chip wide open via `session::force_max_sensitivity`, but that function never updates `tuning.point` (`session.rs:1028-1034`). So the change-test doesn't fire and the panel shows the stale pre-override level (e.g. `3/7 43%`) while the chip is actually at `nf 0, min strikes 1`. The noise caption and JAMMED warning go stale too (`totals.noise_per_min` only written by `tuning.step`, and frozen means it never runs). | `effects.rs:352-374`, `session.rs:1028`, `screen.rs:174`, `ui.rs:281-284` | M |
| L-B4 | **`log::sync` drops buffered records on a mid-write failure.** `for line in self.pending.drain(..) { writeln!(file, "{line}")?; }` — `drain` removes lines as the iterator advances, so a `writeln!` `Err` (filesystem full, I/O fault) returns immediately with every *remaining* buffered line already drained and silently lost. Contradicts the module's own "cannot lose data already recorded" promise. The log has no rotation, so "full" is a real end-state reached exactly during a storm. | `log.rs:316-321` | S |

### Gaps / robustness

| ID | What | Where | Effort |
|---|---|---|---|
| L-G1 | **`recover.sh` waits forever for a port that never appears.** The wait loop `continue`s without incrementing `attempt` (`recover.sh:87-92`), so a missing port loops forever — directly contradicting its own header ("stops with a diagnosis rather than looping forever"), and making the script's "port never appeared" diagnosis unreachable. | `tools/recover.sh:87-92` | S |
| L-G2 | **`release.sh`'s dirty-tree guard cannot see untracked files.** `git diff --quiet HEAD -- $SOURCES` ignores untracked files, so a tree whose only changes are *new* files (the current `src/csv.rs`, `tests/host/csv.rs`) passes the guard and gets stamped despite sources being unrecoverable. | `tools/release.sh:63` | S |
| L-G3 | **`recover.sh` is the repo's only whole-chip-erase script with no board identity check.** It takes a bare `/dev/ttyACM0` default with no by-id MAC lookup and no confirmation, unlike `release/flash.sh:51-57`. A port shuffle erases the wrong board's log. | `tools/recover.sh:55,100` | M |
| L-G4 | **The merger has zero host coverage.** `Merger::observe`, `take_due`, `set_window_ms`, `Accumulator::finish` are pure arithmetic over `Strike`/`Distance` but live in `session.rs`, which imports `esp_idf_hal` and can't be built on the workstation. Untested danger zones: window 0 producing a per-stroke flash with `strokes: 0`, expiry via `saturating_sub`, `set_window_ms` flushing, finish distance resolution. | `src/session.rs:436-624` | M |
| L-G5 | **Clock restore arithmetic is untested.** `restore()` falls back to the NVS epoch (up to `SAVE_INTERVAL_S` = 15 min stale on a true power cut) with no compensation — the exact mechanism `findings.md` implicates in the ~55-min skew. `now()` and `format()` (Hinnant civil-from-days) are pure and untestable today because the module imports IDF. | `src/clock.rs:82-161` | S |
| L-G6 | **Strike-hold fires only on *noisy* windows, but both docs promise unconditional hold.** `tuning.rs` holds only when a window counts as noisy, whereas `console.md:278` and `specs.md:474` promise "contains a strike → hold, never escalate". A window of strikes with no noise is exactly the case that matters most (nearby strike → harmonics as disturbers). | `tuning.rs:274`, vs `console.md:278`, `specs.md:474` | S |
| L-G7 | **`as3935.rs` comment claims SREJ is "deliberately uncalled" and the reference "never writes it" — both false.** `boot.rs:133-138` and `deep_demo.py:138` do write SREJ. Acting on the comment would silently re-break storm detection. | `as3935.rs:399-404`, `boot.rs:133-138` | S |
| L-G8 | **`cargo run` recreates the flash trap `flash.sh` exists to avoid.** `.cargo/config.toml`'s runner is `espflash flash --partition-table partitions.csv --monitor` — it never passes `--bootloader`, so it flashes under whatever bootloader espflash ships (the vendor-mismatch trap `release/flash.sh:10-19` documents). Point the runner at `flash.sh`'s full argument set or at the script. | `.cargo/config.toml:6` | S |
| L-G9 | **The switch to `sensitive on` (glass supported in `effects.rs`) never restarts the tuning window**, so up to one window (~60 s) of old-gain evidence is judged under the new gain after `gain_changed()`. | `tuning.rs:559-594` | S |

### Documentation drift

| ID | What | Where | Effort |
|---|---|---|---|
| L-D1 | **The docs still describe a packed 7-bit / 0–127 / 7-probe tuning point; the code is a 3-bit single field (`NF_LEV`, `BITS=3`, `MAX=7`).** `specs.md:314,318,374,381,463-467,499,1108` and `console.md:226,241,276-278,331` all present the 7-bit world as current. Recent commits (08553c1, 45e5518, 3923359) moved to the 3-bit design; the docs and the build-order changelog (`specs.md:1108`) have not caught up. | `defence.rs:225,237` vs `specs.md`, `console.md` | M |
| L-D2 | **`history.rs` and `specs.md` still say 15-minute day buckets.** `history.rs:13-17` ("Fine 15 min / 24 h"), `history.rs:365` ("four fifteen-minute buckets"), `clock.rs:12` ("bucketed at fifteen minutes"), `specs.md:759,785` — all stale vs `FINE_MINUTES=5`, `FINE_LEN=288`. | `history.rs:13-17,365`, `clock.rs:12`, `specs.md:759,785` | S |
| L-D3 | **Stale capacity figures disagree by ~3×.** "~30 bytes/record" is wrong for the current 11-column row (~90 B). 2,031,616 B / 90 B ≈ ~22-23k records, not `specs.md:681` "~70 000", `specs.md:1006` "~40 000", `partitions.csv` "~65 000". Real capacity is still years of storms, but the numbers should be reconciled at the real ~90 B. | `log.rs:93`, `specs.md:681,1006`, `partitions.csv`, `console.md:194` | S |
| L-D4 | **`specs.md:642-647` AS-BUILT log shows the old 8-column header** missing `millis,kind,nf` that the current code writes. | `specs.md:642-647` vs `log.rs:56-57` | S |
| L-D5 | **`screen.rs` comments carry stale layout facts.** "the day ring alone is 96 buckets" (`screen.rs:96`) — buffer is `LONGEST_LEN=288`; "any day bucket that has data" etc. Also `screen.rs:98-99` computes `counts` with no UI consumer. `ui.rs:1092-1098` "Two stacked charts…draws both" — it draws one score chart; `ui.rs:198` "Strikes per bucket — the count chart" — now the strike-log table. | `screen.rs:96-99`, `ui.rs:198,1092-1098` | S |
| L-D6 | **13-bit / 11-bit / 2048-point / 13-probe archaeology** survives in `tuning.rs` comments, `session.rs:1046-1047`, `settings.rs:143`, `session.rs:1022` ("Every rejection knob at zero" — it sets only `nf 0, min strikes 1`). `effects.rs:359-369` already corrects the wording, so fixes have been landing one file at a time. | `tuning.rs`, `session.rs:1022,1046`, `settings.rs:143` | S |
| L-D7 | **`tests/host/history.rs:3-4` still carries the copy-era boilerplate** ("A copy of `src/history.rs`'s logic") — the README (and lines 10-15 of the same file) says the copy era is gone and real modules are `#[path]`-included. The edition instructions in test headers and the README disagree (2021 vs 2024) though `check.sh` reads the right one. | `tests/host/history.rs:3-4`, `tests/host/*.rs:20,12,16`, `README.md:9` | S |
| L-D8 | **`specs.md:1123` "83 checks" is stale** — the two files it names hold 67-68 checks (not 83) and it omits `csv.rs`/`verdict.rs` (25 checks). Derive the count at render time or say "host checks in `tests/host/`". | `specs.md:1123` | S |

---

## Closed

| ID | What | Closed by |
|---|---|---|
| L-C1 | **Clock skew ~55 min — resolved 2026-08-26.** Set to host time, holds across a reboot. Root cause was us: the fallback to the NVS epoch on a *true power cut* (no RTC compensation) plus the battery-OFF flashing cycle. Complaint: the moisture panel prints `clock: restored from reset, outage assumed, +2 s` while lightning prints only `time: restored` — worth diffing those restore paths if it ever recurs. | See "Clock" narrative below; `clock.rs:82-110` |
| L-C2 | **The tuner climbs on disturbers — fixed 2026-08-28 (not yet flashed at the time).** `observe` now folds only `noise` into the verdict; disturbers are counted/reported but not evidence about the noise floor. Moved to `verdict.rs`, host-tested. | `verdict.rs`, `tuning.rs:203`, `tests/host/verdict.rs` |

---

## Stale — checked and no longer true, do not re-raise

No entries from this pass. (The earlier *verbal* `FINE_MINUTES` 5-vs-15 confusion is not a stale finding — it is real and tracked as L-B2 / L-D2.)

---

## Checked and safe — do not re-raise

| ID | Looks like | Why it is fine |
|---|---|---|
| L-K1 | **BUSY polarity / single 100 ms poll** | `display.rs` inverts correctly (the spec's inverting adapter was written on the strength of the doc, hung every refresh, and was removed; LOW=busy is stock Waveshare and matches `epd-waveshare`'s `IS_BUSY_LOW`). `wait_for_refresh` is two-phase (BUSY fall then rise); the `0` return is the honest "nothing drawn". |
| L-K2 | **Console aliases missing** | All six exist (`date`, `scope`, `defence`, `calibrate`, `sens`, `batt`) matching `console.md:122`. |
| L-K3 | **Notification bits tested as values** | `main.rs:100-112` documents and the code tests `bits & NOTIFY_BUTTON` as bits, not `==2`. |
| L-K4 | **Refresh policy** | Change-gated, 30 s floor, 5 min baseline backstop, strike/button bypasses — matches spec (`screen.rs:133-148`). |
| L-K5 | **battery CRATE scaling** | `raw * 104 / 5` matches the documented centi-%/hr; the 100× error was already fixed. `hours_from_drain` uses u64 intermediates and saturating ops. No overflow on riscv32imc. |
| L-K6 | **CSV no escaping** | Safe today only because the writer's vocabulary is comma-free (`iso_local` + digits + fixed tokens). Worth a comment pinning that invariant (see L-B/L-G area) but not a runtime bug now. |
| L-K7 | **Panic survey** | Only two `unwrap`s in `src/`, both `NonZeroU32::new(...).unwrap()` on constants (`main.rs:307,338`) — cannot fail. NVS handle closed on drop. |
| L-K8 | **`build.rs` staleness/duplication** | Output in `OUT_DIR` with `cargo:rerun-if-changed` on the `.pbm`; nothing generated is committed; `release.sh` includes `assets`. |
| L-K9 | **Clock restore prefers live RTC** | `restore()` uses the running RTC when present and only falls back to NVS on a true power cut (`clock.rs:99-109`) — that is the documented (and now understood) cost, not a bug. |

---

## Declined — with the reasoning

| ID | What | Why not |
|---|---|---|
| L-X1 | **Lower `SAVE_INTERVAL_S` 15 → 5 min** | Trades ~35k NVS writes/yr for ~105k to cut the worst-case per power-cycle by ⅔; only matters on a *true* power cut (dev cycle), not worth a reflash that needs the very power cycle it mitigates. |
| L-X2 | **Ro the log / add capacity-when-full alarm** | The real problem is the drain-on-Err drop (L-B4), not the size; a ~22k record log is years of storms. |
| L-X3 | **Move `now_ms` to full u64 throughout** | Correct modular fix for intervals < 2³² ms is `wrapping_sub`, not a wider type; u64 touches every call site for one wrap every ~50 days. |

---

## Suggested order

1. **L-B1** (49.7-day freeze — real runtime bug on any long deployment) and **L-B4** (storm-time data loss on a full FS) first — both are correctness, under S/M effort.
2. **L-B2** (day-axis) — pure constant fix plus an honest host test tying `bucket_minutes()` to `FINE_MINUTES`.
3. **L-G1** and **L-G2** (recover.sh infinite loop, release.sh untracked-files hole) — tooling, cheap.
4. **L-B3** (sensitive-on gauge) — panel honesty; has the console vocabulary to copy.
5. **L-G4 / L-G5** (merge clock, verdict) — extract and host-test, the highest-leverage test additions.
6. **L-D1** through **L-D8** — a single doc sweep updating the 7-bit→3-bit world, the 15-min→5-min buckets, capacity, and the stale comments.

---

## The older narrative, preserved

### Clock skew, measured 2026-08-26

| observed | panel said | real | behind by |
|---|---|---|---|
| 2026-08-23 | 13:54:55 | 14:50 | ~55 min |
| 2026-08-26 | 11:03 | 11:54 | ~51 min |
| 2026-08-26 12:28 | 11:31:58 | 12:28:42 | **57 min** |

**Roughly constant, not growing.** That matters: a stored epoch that never gets
re-saved would fall further behind every day, and this does not. So the epoch
*is* being written; what is lost is bounded and recurring, which points at the
restore arithmetic or at a device that reboots often enough to keep paying the
same penalty.

`clock::SAVE_INTERVAL_S` is 15 min, so a single reboot should cost at most 15
min plus the time actually off. Losing ~55 every time is about four of those.

The moisture panel does not do this, and prints `clock: restored from reset,
outage assumed, +2 s` where lightning prints only `time: restored <stamp>` —
diff those two restore paths before theorising further.

**This storm's strike log will be stamped ~57 min early.** The offset is known,
so the records are correctable; not worth a reboot mid-storm to fix.

### Clock: resolved 2026-08-26, and the cause was us

Set to the host's time: panel 12:54:48 against host 12:54:42, six seconds fast,
down from 56 minutes behind. `time: restored` confirms it persists across a
reboot.

**The drift was not a bug in the running device.** `restore()` prefers a live
clock and only falls back to the stored epoch when there is nothing running —
and the RTC keeps counting across resets, so ordinary reboots cost nothing. The
fallback is reached only on a *true power cut*, and there it costs (up to
`SAVE_INTERVAL_S`, 15 min) plus however long the board was actually off, with no
compensation for either.

That is precisely the "battery OFF, USB out, wait, back" cycle this board needs
to reach its bootloader, and today's flashing did several. The skew grew 51 to
57 minutes across them, which is about one cycle's worth each.

So: in the field, on battery, it should hold. It drifts when somebody power
cycles it, which is a development cost rather than an operational one.

### The tuner climbs on disturbers, and NF_LEV cannot answer them

Watched live, 2026-08-26, indoor, through a storm:

    nf 2:  11 noise, 0 disturbers
    nf 3:   0 noise, 5-8 disturbers      <- one notch, and the band opened
    nf 4:   still climbing

The first step is the system working: `NF_LEV` is exactly the knob for a chip
drowning in its own noise floor, and `[wd 2 sr 0 fixed]` shows the two
strike-costing knobs held still while it moved. That is 0.11.0 earning its keep.

**The steps after it are pointless.** `Tuning::observe` counted
`noise + disturbers` into `events`, so a storm throwing disturbers read as a
noisy band — and the tuner answered with `NF_LEV`, which gates the *noise floor*
and does nothing to a validated disturber. It would climb to 7, stay noisy, and
hand over to the stuck detector.

Harmless for detection, because `NF_LEV` cannot reject a lightning waveform
either. But it is work that cannot succeed, and it burns the ladder's whole
range before the stuck detector notices.

**Fixed 2026-08-28, not yet flashed.** Took the option the note already called
closer to the truth and smaller: `observe` folds only `noise` into the verdict.
Disturbers are still counted and still reported — they are simply not evidence
about the noise floor.

The decision moved to `src/verdict.rs`, free of ESP-IDF like `defence` and for
the same reason: `tuning` drives an I²C sensor and cannot be built on a
workstation, so the one claim worth checking was untestable. `tests/host/verdict.rs`
now compiles the real module and asserts the negative directly — *a window of
disturbers alone is quiet*. Nothing in the type system says that, and folding the
two together compiles and looks reasonable, which is how it shipped.
