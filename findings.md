## How to read this

This is working state for the analysis passes of **2026-08-28** (the second pass of
that date put the shell tooling in scope) — gitignored, never committed. Each
finding has an ID, a status, a location and an effort guess (S = small, M =
medium, L = large). Fold re-analysis into the tables; a finding lives in exactly
one table, so re-verifying it means editing its row, not appending a note
elsewhere. The tooling (`tools/`, `*.sh`, `build.rs`, `.cargo/`) is analysed on
the same footing as `src/` — findings there carry `L-T` IDs.

The older narrative entries (clock skew, the disturber tuner) are preserved
below their own headings, resolved and marked as such.

---

## Open

> **Swept 2026-08-28.** Every entry below carries a verdict. Fixed items are kept
> rather than deleted: the reasoning is why the fix is the shape it is, and a
> deleted finding is one somebody re-raises in six months.


### Bugs — will misbehave on hardware

| ID | What | Where | Effort |
|---|---|---|---|
| ~~L-B1~~ | **[FIXED]** *(fixed — see L-C3)* **`now_ms()` is a `u32` that wraps at ~49.7 days and freezes the loop.** `now_ms()` is `(esp_timer_get_time()/1000) as u32` (`main.rs:413-415`). Every interval compares against a stored sample via `.saturating_sub`; after the wrap the stored sample is near 2³² and the delta saturates to 0 forever, so `listen.rs:226` takes the `continue` branch permanently — no processing, no tuner, no screen, no log sync — while the ISR still posts notifications so the device looks alive. A plain `-` at `listen.rs:378` underflows (panic in debug, reboot-loop in release). | `main.rs:413`, `listen.rs:226,250,378`, `screen.rs:134`, `battery.rs:668` | M |
| L-B2 | **[FIXED]** **Day-chart time axis is mislabelled 3×.** `ChartPeriod::bucket_minutes(Day)` returns 15 (`ui.rs:975`) but the day chart draws from `history.day`, a `Ring<288>` of **5-minute** buckets (`FINE_MINUTES=5`). So `per_tick = 360/15 = 24` buckets/gridline = 120 real minutes labelled `-6h`, and the `-12h`/`-18h` labels point at columns the 6h40m window does not hold. The claim that they are "asserted equal in `tests/history.rs`" (`ui.rs:969-972`) is false — that test only checks `FINE_MINUTES == 5` and never links the UI constant. | `ui.rs:975`, `ui.rs:1048-1089`, `tests/history.rs:158-168` | S |
| L-B3 | **[FIXED]** **`sensitive on` leaves the gauge lying.** `effects.rs` sets `tuning.frozen = true` and programs the chip wide open via `session::force_max_sensitivity`, but that function never updates `tuning.point` (`session.rs:1028-1034`). So the change-test doesn't fire and the panel shows the stale pre-override level (e.g. `3/7 43%`) while the chip is actually at `nf 0, min strikes 1`. The noise caption and JAMMED warning go stale too (`totals.noise_per_min` only written by `tuning.step`, and frozen means it never runs). | `effects.rs:352-374`, `session.rs:1028`, `screen.rs:174`, `ui.rs:281-284` | M |
| L-B4 | **[FIXED]** **`log::sync` drops buffered records on a mid-write failure.** `for line in self.pending.drain(..) { writeln!(file, "{line}")?; }` — `drain` removes lines as the iterator advances, so a `writeln!` `Err` (filesystem full, I/O fault) returns immediately with every *remaining* buffered line already drained and silently lost. Contradicts the module's own "cannot lose data already recorded" promise. The log has no rotation, so "full" is a real end-state reached exactly during a storm. | `log.rs:316-321` | S |

### Gaps / robustness

| ID | What | Where | Effort |
|---|---|---|---|
| L-G1 | **[FIXED]** **`recover.sh` waits forever for a port that never appears.** The wait loop `continue`s without incrementing `attempt` (`recover.sh:87-92`), so a missing port loops forever — directly contradicting its own header ("stops with a diagnosis rather than looping forever"), and making the script's "port never appeared" diagnosis unreachable. | `tools/recover.sh:87-92` | S |
| L-G2 | **[FIXED]** **`release.sh`'s dirty-tree guard cannot see untracked files.** `git diff --quiet HEAD -- $SOURCES` ignores untracked files, so a tree whose only changes are *new* files (the current `src/csv.rs`, `tests/csv.rs`) passes the guard and gets stamped despite sources being unrecoverable. | `tools/release.sh:63` | S |
| L-G3 | **[FIXED]** **`recover.sh` is the repo's only whole-chip-erase script with no board identity check.** It takes a bare `/dev/ttyACM0` default with no by-id MAC lookup and no confirmation, unlike `release/flash.sh:51-57`. A port shuffle erases the wrong board's log. | `tools/recover.sh:55,100` | M |
| L-G4 | **[FIXED]** **The merger has zero host coverage.** `Merger::observe`, `take_due`, `set_window_ms`, `Accumulator::finish` are pure arithmetic over `Strike`/`Distance` but live in `session.rs`, which imports `esp_idf_hal` and can't be built on the workstation. Untested danger zones: window 0 producing a per-stroke flash with `strokes: 0`, expiry via `saturating_sub`, `set_window_ms` flushing, finish distance resolution. | `src/session.rs:436-624` | M |
| L-G5 | **[FIXED — the arithmetic; restore rejected]** The calendar arithmetic is extracted to `src/civil.rs`, free of ESP-IDF *and* of every crate, and covered by 40 host checks: leap years, the century rules (1900/2000/2100/2400), month lengths derived rather than tabulated, and all 86,400 seconds of a leap day. `restore()` is rejected: re-reading it, there is no arithmetic there to test. It is three lines of policy — prefer a running RTC, else take NVS if it passes `PLAUSIBLE_EPOCH` — and the "no compensation" the finding names is a deliberate decision, not an untested calculation. Testing it would mean inventing a seam whose only caller is the test. | `src/civil.rs`, `tests/civil.rs` | S |
| L-G6 | **[FIXED]** **Strike-hold fires only on *noisy* windows, but both docs promise unconditional hold.** `tuning.rs` holds only when a window counts as noisy, whereas `console.md:278` and `specs.md:474` promise "contains a strike → hold, never escalate". A window of strikes with no noise is exactly the case that matters most (nearby strike → harmonics as disturbers). | `tuning.rs:274`, vs `console.md:278`, `specs.md:474` | S |
| L-G7 | **[FIXED]** **`as3935.rs` comment claims SREJ is "deliberately uncalled" and the reference "never writes it" — both false.** `boot.rs:133-138` and `deep_demo.py:138` do write SREJ. Acting on the comment would silently re-break storm detection. | `as3935.rs:399-404`, `boot.rs:133-138` | S |
| L-G8 | **[FIXED]** **`cargo run` recreates the flash trap `flash.sh` exists to avoid.** `.cargo/config.toml`'s runner is `espflash flash --partition-table partitions.csv --monitor` — it never passes `--bootloader`, so it flashes under whatever bootloader espflash ships (the vendor-mismatch trap `release/flash.sh:10-19` documents). Point the runner at `flash.sh`'s full argument set or at the script. | `.cargo/config.toml:6` | S |
| L-G9 | **[FIXED]** **The switch to `sensitive on` (glass supported in `effects.rs`) never restarts the tuning window**, so up to one window (~60 s) of old-gain evidence is judged under the new gain after `gain_changed()`. | `tuning.rs:559-594` | S |

### Documentation drift

| ID | What | Where | Effort |
|---|---|---|---|
| L-D1 | **[FIXED]** **The docs still describe a packed 7-bit / 0–127 / 7-probe tuning point; the code is a 3-bit single field (`NF_LEV`, `BITS=3`, `MAX=7`).** `specs.md:314,318,374,381,463-467,499,1108` and `console.md:226,241,276-278,331` all present the 7-bit world as current. Recent commits (08553c1, 45e5518, 3923359) moved to the 3-bit design; the docs and the build-order changelog (`specs.md:1108`) have not caught up. | `defence.rs:225,237` vs `specs.md`, `console.md` | M |
| L-D2 | **[FIXED]** **`history.rs` and `specs.md` still say 15-minute day buckets.** `history.rs:13-17` ("Fine 15 min / 24 h"), `history.rs:365` ("four fifteen-minute buckets"), `clock.rs:12` ("bucketed at fifteen minutes"), `specs.md:759,785` — all stale vs `FINE_MINUTES=5`, `FINE_LEN=288`. | `history.rs:13-17,365`, `clock.rs:12`, `specs.md:759,785` | S |
| L-D3 | **[FIXED]** **Stale capacity figures disagree by ~3×.** "~30 bytes/record" is wrong for the current 11-column row (~90 B). 2,031,616 B / 90 B ≈ ~22-23k records, not `specs.md:681` "~70 000", `specs.md:1006` "~40 000", `partitions.csv` "~65 000". Real capacity is still years of storms, but the numbers should be reconciled at the real ~90 B. | `log.rs:93`, `specs.md:681,1006`, `partitions.csv`, `console.md:194` | S |
| L-D4 | **[FIXED]** **`specs.md:642-647` AS-BUILT log shows the old 8-column header** missing `millis,kind,nf` that the current code writes. | `specs.md:642-647` vs `log.rs:56-57` | S |
| L-D5 | **[FIXED]** **`screen.rs` comments carry stale layout facts.** "the day ring alone is 96 buckets" (`screen.rs:96`) — buffer is `LONGEST_LEN=288`; "any day bucket that has data" etc. Also `screen.rs:98-99` computes `counts` with no UI consumer. `ui.rs:1092-1098` "Two stacked charts…draws both" — it draws one score chart; `ui.rs:198` "Strikes per bucket — the count chart" — now the strike-log table. | `screen.rs:96-99`, `ui.rs:198,1092-1098` | S |
| L-D6 | **[FIXED]** **13-bit / 11-bit / 2048-point / 13-probe archaeology** survives in `tuning.rs` comments, `session.rs:1046-1047`, `settings.rs:143`, `session.rs:1022` ("Every rejection knob at zero" — it sets only `nf 0, min strikes 1`). `effects.rs:359-369` already corrects the wording, so fixes have been landing one file at a time. | `tuning.rs`, `session.rs:1022,1046`, `settings.rs:143` | S |
| L-D7 | **[FIXED]** **`tests/history.rs:3-4` still carries the copy-era boilerplate** ("A copy of `src/history.rs`'s logic") — the README (and lines 10-15 of the same file) says the copy era is gone and real modules are `#[path]`-included. The edition instructions in test headers and the README disagree (2021 vs 2024) though `check.sh` reads the right one. | `tests/history.rs:3-4`, `tests/*.rs:20,12,16`, `README.md:9` | S |
| L-D8 | **[FIXED]** **`specs.md:1123` "83 checks" is stale** — the two files it names hold 67-68 checks (not 83) and it omits `csv.rs`/`verdict.rs` (25 checks). Derive the count at render time or say "host checks in `tests/`". | `specs.md:1123` | S |

---

## Verdicts from the 2026-08-28 sweep

| ID | Verdict |
|---|---|
| L-B1 | FIXED 0.13.3 — 19 sites moved to `uptime::due`/`since` (wrapping_sub). Not u64: L-X3 stands, owner confirmed. `tests/uptime.rs`, 18 checks. |
| L-B2 | FIXED 0.13.4 — `bucket_minutes` now imports `history`'s constants instead of copying them, so the day axis divides by 5 not 15. The comment claiming they were "asserted equal" was false; importing removes the way to disagree. |
| L-B3 | FIXED 0.13.4 — `sensitive on` now calls `Tuning::observe_forced_point`, so the gauge shows where the chip actually is. The stored point is deliberately untouched: an override is for a storm now and must not survive a power cut. |
| L-B4 | FIXED 0.13.4 — `sync` writes from a borrow and drains only what was written, so a mid-write error keeps everything it could not write for the next attempt. |
| L-G1 | FIXED 0.13.4 — a bounded `waited` counter with its own five-minute limit and its own diagnosis, which now names the power-switch cause. |
| L-G2 | FIXED 0.13.4 — `git status --porcelain` sees untracked files where `git diff` cannot. My own bug, written the same day. |
| L-G3 | FIXED 0.13.4 — looks the board up by MAC and refuses a by-id path that is not it. This is the one script that erases a whole chip. |
| L-G4 | FIXED 0.13.5 — `Merger` moved to `src/merger.rs` (pure) with 20 host checks. One of them corrected my own assumption rather than the code. |
| L-G5 | FIXED (calendar) / REJECTED (restore) — `civil.rs` extracted and host-tested; `restore()` holds a policy, not arithmetic. |
| L-G6 | FIXED 0.13.4 — the hold arm is now `strikes > 0`, matching both documents. A window that heard a strike describes the weather, not the room, so stepping on it in either direction acts on the wrong measurement. |
| L-G7 | FIXED 0.13.4 — the comment was false in both halves and its danger was the invitation: "so let us start writing it" is the change that once cost strike validation. The warning is kept, the false claim removed. |
| L-G8 | FIXED 0.13.4 — the runner points at `flash.sh`, which now tolerates cargo handing it a binary path. |
| L-G9 | FIXED 0.13.4 — a gain change now clears the window and restarts the chip statistics, not just the sweep, dip, point and span. |
| L-D1..L-D8 | ALL FIXED — swept 2026-08-28. Verdicts below. |
| L-K1..L-K9 | Re-checked, still safe, not re-raised. |
| L-X1..L-X3 | Still declined. L-X3's reasoning was used verbatim for L-B1 and the owner confirmed it: modular arithmetic, not a wider integer. |

### Raised by the sweep

| ID | What |
|---|---|
| L-N1 | **A merged flash mixing `Overhead` with kilometre readings reports the average of the kilometre ones and discards the sentinel.** `merger::Accumulator::finish` uses `overhead` only when there were no kilometre samples. Since "overhead" means *nearer than 5 km*, a flash containing one is arguably nearer than the average suggests, and the device reports the further of the two. Self-consistent and documented, so recorded rather than changed — altering it moves every mixed record and that is a decision. `tests/merger.rs` pins the current contract either way. |

## Second pass — re-read the post-fix tree **and the tools**, 2026-08-28

A follow-up pass after the 0.13.x fixes, and the first that put the shell tooling in
scope. It verified every sweep verdict against the clean tree at `a40abd8e5f87`
(all CONFIRMED), and turned up new items. This is the same "one home per finding"
rule: new IDs, folded here, not re-raised elsewhere.

### Code — new

| ID | What | Where | Effort |
|---|---|---|---|
| L-P1 | **[FIXED — reproduced first] `power.rs:161` misses the wrap the L-B1 sweep fixed everywhere else — a device nobody uses wedges permanently Awake after 49.7 days.** `Some(seen) if uptime_s.saturating_sub(seen) < CONSOLE_AWAKE_S => Policy::Awake`. After the u32-seconds wrap, `uptime_s` is small and `seen` (pre-wrap) is near 2³², so the subtraction saturates to 0 forever → Policy::Awake for the rest of the wrap cycle: 160 MHz, no light sleep, ~0.170 W vs ~0.013 W (`power.rs:7-9`). The *opposite* symptom of the L-B1 freeze, at a site the sweep missed. Fix: `uptime::since`-style wrapping subtraction.** | `src/power.rs:161` | S |
| L-P2 | **[FIXED] `press.rs` is a whole host-tested subsystem with zero firmware callers — the documented stuck-DTR protection is not shipped.** `Press::sample`/`classify`/`Gesture` run only in `tests/press.rs`; the runtime button path is `listen.rs:266-305` + `boot.rs:256-266`, implementing only a 1.5 s floor vs a 2 s ceiling. The module's own doc ("anything past the ceiling is a cable… refused and reported") is not true in the firmware.** Decide deliberately: wire it into the listen button path, or delete it and correct the claim.** | `src/press.rs`, `src/listen.rs:266`, `src/boot.rs:256` | L |
| L-P3 | **[FIXED] `csv.rs` does not parse the `strokes` column it writes.** `Columns` (`csv.rs:28`) has no strokes member; `parse_row` (`csv.rs:76`) returns only epoch/energy/distance; the boot replay hardcodes `strokes: 1` (`listen.rs:161-167`) with a comment saying it's "not recoverable from the columns" — but the header and writer carry `strokes` as the 11th column (`log.rs:57,355`). Replayed storms undercount stroke counts on the per-strike table (`ui.rs:831-844`), and the comment misleads a future reader.** | `src/csv.rs:28,76`, `src/listen.rs:161`, `src/log.rs:57` | M |
| L-P4 | **[FIXED] `log.rs` `events 1` budgets are effectively N−1: the event that spends the last row is reduced to 0 and returned without appending.** `src/log.rs:294-304` — the off-by-one understates every armed "how many did I catch" total by one at its tail.** | `src/log.rs:294-304` | S |
| L-P5 | **[FIXED] `commands.rs:154` `strike` intensity overflows u32.** `energy_raw: intensity_milli * 16777 / 1000` overflows above ~255,934 (`console.rs:348-351` accepts any u32, default 4000). Debug: panic inside the wake loop — a hard reboot, violating `listen.rs`'s "nothing here may panic". Release: silently wraps and feeds garbage into the rings/score/CSV. Real ceiling ~62,500 (2²⁰−1 energy). Fix: u64 intermediate or clamp.** | `src/commands.rs:154` | S |
| L-P6 | **[FIXED] `defence <raw>` silently overrides a frozen `sensitive on` override.** `tuning.rs:786-802` `place()` programs the chip to the new point and moves the tuner's idea of it while frozen, contradicting the console's own "auto-tune frozen" claim; the next `sensitive off` discards it. Should be announced as an explicit operator override.** | `src/tuning.rs:786`, `src/commands.rs:249` | S |
| L-P7 | **[FIXED] `log.rs:407` `#[allow(dead_code)]` on `clear()` is stale** — still reached via `Command::Clear`. The allow would mask a future removal.** | `src/log.rs:407` | S |
| L-P8 | **[FIXED] The noise caption and the JAMMED warning still freeze during `sensitive on` — the second half of the original L-B3 never shipped.** While frozen, `tuning.due()` is false, so `listen.rs:484` skips `tuning.step`, and `totals.noise_per_min` is written only there; `sensitive on` before the first ~60 s step leaves `0/min` (Totals derives Default) and the JAMMED test (`NOISE_JAMMED_PER_MIN=60`) can never trip. L-B3's verdict claimed only the *gauge*; the caption/warning half is open.** | `src/listen.rs:484`, `src/tuning.rs:248`, `src/ui.rs:259` | M |

### Code — raised during the verification pass, 2026-08-28

Found while grounding L-D12 by listing every remaining `saturating_sub`. Neither
pass raised them; both are the same class as L-P1.

| ID | What | Where | Effort |
|---|---|---|---|
| L-P9 | **[FIXED] `battery.rs` `span_s()` had L-P1's bug in a milder form.** `latest_s.saturating_sub(anchor_s)`, both from the wrapping uptime counter, so across the wrap the trend window reports a zero-length span and `verdict()` — which refuses to judge a span shorter than the window — returns `Unknown`. The battery rate display goes blank until the anchor next rolls, so it heals itself, unlike L-P1. Notable because `roll()` on the line above *already* used `uptime::due`: the wrap sweep fixed the loop condition and left the accessor. | `src/battery.rs:558` | S |
| L-P10 | **[FIXED] `listen.rs:411` was the last bare `a - b` interval in the firmware** — not even saturating. `now_ms() / 1000 - last_clock_save_s >= SAVE_INTERVAL_S` underflows across the wrap: a debug build panics **inside the wake loop**, where this module's own rule is that nothing may panic, and a release build wraps to a huge number that reads as due and spends one unasked-for NVS write. Now `uptime::due`. | `src/listen.rs:411` | S |

### Tools — new (in scope this pass)

| ID | What | Where | Effort |
|---|---|---|---|
| L-T1 | **[FIXED] `recover.sh`'s explicit-port guard only matches `/dev/serial/by-id/*`.** A bare `/dev/ttyACM0` or any other non-by-id path sails straight into the whole-chip `esptool erase-flash` (line 147) with zero identity check, despite the comment (77-78) claiming "being explicit is not the same as being right". On a host where ttyACM numbering shuffles, this erases the wrong board's log. Require a by-id path matching the MAC, or verify `esptool chip-id` first.** | `tools/recover.sh:77-82,147` | M |
| L-T2 | **[FIXED] `recover.sh` flashes whatever is in `target/` with no check it is the recovery build — and the natural sequence makes it fatal.** The header's "build the no-light-sleep image first" advice (`recover.sh:91-92`) is unenforced; `flash.sh` builds the default light-sleep ELF into `target/` before flashing, so a failed flash leaves `target/` holding exactly the light-sleep binary that broke the console, and `recover.sh` writes it back. Write `release/no-light-sleep/` artifacts, or build `--features no-light-sleep` as step 1, or refuse unless the ELF is the recovery build.** | `tools/recover.sh:86,158` | M |
| L-T3 | **[FIXED] `recover.sh`'s write path is text-gated, not exit-code-gated — asymmetric with its own erase path.** Success is `grep -q 'Flashing has completed'` on captured text (line 163) with the espflash exit status discarded (line 158); the erase path (`:147`) is `if !`-gated. If the espflash wording changes, a *successful* write is reported failed, the loop erases the just-written flash again, and after 8 attempts reports FAILED on a recovered board. Gate on exit status; keep the grep as display.** | `tools/recover.sh:158,163` | S |
| L-T4 | **[FIXED] `release.sh`'s `SOURCES` omits two tracked files that change the produced binary.** `.cargo/config.toml` (pins ESP_IDF_VERSION, partitions.csv delivery, `espidf_time64` rustflag, MCU) and `components_esp32c3.lock` (pins joltwallet/littlefs 1.22.3 under a floating `^1.14` in Cargo.toml). Because the dirty guard and the release/flash.sh staleness check both consume `SOURCES`, a change to either passes the guard and gets an image stamped with a commit that does not contain it. Same class as the just-fixed untracked-files bug.** | `tools/release.sh:35` | M |
| L-T5 | **[FIXED] `flash.sh` (both root and release) lets an explicit `/dev/ttyACM<n>` bypass the by-id MAC check.** The guard only runs in the by-id default branch; `flash.sh`'s own usage advertises `./flash.sh /dev/ttyACM1`. The board-identity guarantee the repo's release script engineered around is silently dropped on the explicit path.** | `flash.sh:61-73`, `release/flash.sh:34,54-68` | M |
| L-T6 | **[FIXED] `watch.py` raw-crashes on bad input and on the common port error.** `int(sys.argv[2])` raises an unhandled ValueError for non-numeric input (`watch.py:21`), and `s.open()` sits *outside* the try (`watch.py:29`), so the most common runtime error (port missing/busy) produces a raw traceback instead of the script's "port dropped" message. Wrap arg parsing and include `open()` in the try.** | `tools/watch.py:21,29` | S |
| L-T7 | **[FIXED] `build.rs` PBM parser panics with unhelpful slice messages on malformed assets.** `assert_eq!(&raw[..2], b"P4", …)` (line 82) panics with "range end index 2" on an empty/1-byte file instead of the friendly not-a-PBM message; a CRLF-terminated header (line 108-114) leaves `\n` as the first payload byte and fails the length assert with a misleading message. Use `raw.starts_with(b"P4")`, and treat `\r\n` as one separator.** | `build.rs:82,108-114` | S |
| L-T8 | **[FIXED] `flash.sh`'s usage line claims "build + flash + monitor" but it never monitors** — it stops and prints "Monitor with: espflash monitor". Stale leftover from the pre-runner `--monitor` era.** | `flash.sh:5,113-114` | S |

### Doc drift — new

| ID | What | Where | Effort |
|---|---|---|---|
| L-D9 | **[FIXED] The "every rejection knob wide open" over-promise L-N2 corrected in `session.rs`/`effects.rs` status lines was **not** carried to every operator surface.** On-device help `console.rs:395`, `doc/console.md:165`, and `doc/console.md:376` still say `sensitive on` "opens every knob" — but `Point::OPEN` is `NF_LEV=0` alone; `WDTH`, `SREJ` and `MIN_NUM_LIGH` keep their settings. An operator told the override opens everything is misled. Same class as the L-D sweep; these three were missed. (`console.md:372` also says the tuner "starts mid-range on the two volume knobs (NF_LEV, WDTH)" — stale.)** | `src/console.rs:395`, `doc/console.md:165,372,376` | S |
| L-D10 | **[FIXED] `defence.rs` method docs still describe the dead 4-register space** after the L-D1/L-D6 header sweep fixed only the module header. `tightened()` (`defence.rs:347-364`) describes the cheapest-first walk across NF_LEV/WDTH/MIN_NUM_LIGH and "climbing off `wd 7`"; `relaxed()` (`defence.rs:377-392`) says "settled at 448 (`wd 7, sr 0, ms 0`)" and "447 = `wd 6, sr 15, ms 3`"; `percent()` (`defence.rs:406-423`) gives "the same point now reads 25%" and "MIN_NUM_LIGH overrides everything". The implementation is one 3-bit `NF_LEV`, raw+1/raw−1, `100·nf/7`, monotonic. A reader will confidently diagnose states that cannot exist.** | `src/defence.rs:347-423` | M |
| L-D11 | **[FIXED] `tuning.rs` prose still carries the old 13-notch ladder.** `climb()`'s doc (`tuning.rs:429-443`) says "40 notches against a ladder exactly 40 notches deep" (the ladder is 8; behavior is right — `notches` caps at MAX — only the numbers are stale); `hold()` (`tuning.rs:407-414`) says "each notch of MIN_NUM_LIGH hides the following strikes" (pinned at 1; only NF_LEV is walked).** | `src/tuning.rs:407-443` | S |
| L-D12 | **[FIXED] `uptime.rs:10-12` doctrine line is now false.** "Every interval in this firmware is `now.saturating_sub(then)`" — everything moved to `due`/`since` and this site (`listen.rs:411`, `power.rs:161`, `battery.rs:558`) disproves it.** | `src/uptime.rs:10-12` | S |
| L-D13 | **[FIXED] `press.rs` constant comments moved with the constants but not the words.** `ACCEPT_MS` doc (`press.rs:43-46`) says "the long gesture at ten" (LONG_MS is 5_000, `press.rs:61`); `STUCK_MS` (`press.rs:65-67`) says "ten seconds of slack over LONG_MS" (actual slack 25 s).** | `src/press.rs:43-67` | S |
| L-D14 | **[FIXED] `specs.md:1176-1178` says "`tools/check.sh` prints" the check count, but the script prints none on success** (only FAIL lines and "all checks passed"). Minor, and the count now changes with every test added.** | `doc/specs.md:1176-1178` | S |

### Corrections to my own first-pass estimates (from the owner's 0.13.x sweep, confirmed)

- **L-D3 — my ~90 B/record was high.** The measured strike row is **74 B**, event row 57 B → the 2,031,616 B partition holds **~27 000 strikes**, not my ~22-23k. The owner's numbers are right.
- **L-G5 — I over-reached on `restore()`.** It is genuinely policy (prefer RTC, else NVS if `>= PLAUSIBLE_EPOCH`), not arithmetic worth a seam. `civil.rs` covers the real calendar math. Agreed, declined as I proposed.
- **L-G4 — the merger host test "corrected my own assumption rather than the code"**, and L-N1 (the Overhead-vs-kilometre average) is real and recorded. Agreed.
- **L-D6/L-N2 — `force_max_sensitivity` sets only `NF_LEV=0`; "every knob at zero" was an over-promise** in the code doc too. This pass found the operator-facing leftover (L-D9).

## Closed

| ID | What | Closed by |
|---|---|---|
| L-C1 | **Clock skew ~55 min — resolved 2026-08-26.** Set to host time, holds across a reboot. Root cause was us: the fallback to the NVS epoch on a *true power cut* (no RTC compensation) plus the battery-OFF flashing cycle. Complaint: the moisture panel prints `clock: restored from reset, outage assumed, +2 s` while lightning prints only `time: restored` — worth diffing those restore paths if it ever recurs. | See "Clock" narrative below; `clock.rs:82-110` |
| L-C3 | **L-B1, the 49.7-day freeze — fixed 2026-08-28.** Nineteen interval comparisons moved from `saturating_sub` to `uptime::due`/`since`, which is `wrapping_sub` underneath. Not widened to `u64`: L-X3's reasoning stands and the owner confirmed it. `tests/uptime.rs` walks the wrap, including the case that used to fail — the same comparison under `saturating_sub` reads zero for ever. | `uptime.rs`, `tests/uptime.rs`, 19 sites |
| L-C2 | **The tuner climbs on disturbers — fixed 2026-08-28 (not yet flashed at the time).** `observe` now folds only `noise` into the verdict; disturbers are counted/reported but not evidence about the noise floor. Moved to `verdict.rs`, host-tested. | `verdict.rs`, `tuning.rs:203`, `tests/verdict.rs` |

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

The whole first pass (L-B1..L-B4, L-G1..L-G9, L-D1..L-D8) is **fixed** as of 0.13.x.
Working order for the second pass's findings:

1. **L-P1** (power.rs wrap — a real field-power bug that silently disables light sleep after ~50 days) — one-line `uptime::since` fix, hot path.
2. **L-T1, L-T5, L-T3** (recover.sh by-id bypass, flash.sh by-id bypass, recover.sh text-gated write) — destructive-path guardrails, cheap.
3. **L-T4** (release.sh SOURCES omission — same class as the untracked-files bug, tracked-but-unlisted build inputs).
4. **L-P3** (strokes column unparsed — replayed storms undercount the per-strike table) — M.
5. **L-P2** (press.rs dead subsystem) — deliberate decision: wire in or delete.
6. **L-P8** (noise caption/JAMMED freeze during `sensitive on`) — the unfinished half of L-B3.
7. **L-P5, L-P6, L-P4, L-P7** (intensity overflow, defence-while-frozen, log N−1 tail, stale allow) — small correctness/behaviour items.
8. **L-D9..L-D14, L-T2, L-T6, L-T7, L-T8** — doc drift and tooling cosmetics in one sweep.

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
workstation, so the one claim worth checking was untestable. `tests/verdict.rs`
now compiles the real module and asserts the negative directly — *a window of
disturbers alone is quiet*. Nothing in the type system says that, and folding the
two together compiles and looks reasonable, which is how it shipped.


## Verdicts from the documentation sweep, 2026-08-28

| ID | Verdict | Detail |
|---|---|---|
| L-D1 | FIXED | A superseding `⚠⚠⚠⚠ AS BUILT` callout heads §4.2 recording the 3-bit world, why each of the other three knobs left, and that the two callouts below it are history. `console.md` rewritten for 0–7 and three probes; `specs.md`'s build-order line and the console help text corrected. The superseded callouts keep their arguments, as the repo's convention intends — they are now labelled as superseded rather than reading as current. |
| L-D2 | FIXED | Fine buckets are 5 minutes everywhere: `history.rs` table, the 8640-bucket hypothetical, the `live_len` example, `last_hour`, `clock.rs`, and `specs.md`'s ring list. `last_hour` now says it derives from `FINE_MINUTES` so the same drift cannot recur. The unrelated fifteen-minute facts — `SAVE_INTERVAL_S`, the battery window, the screen cadence — were checked and left. |
| L-D3 | FIXED | Measured rather than estimated: a strike row in the 11-column format is **74 B**, an event row 57 B, so the 2 031 616 B partition holds **~27 000 strikes**. Corrected in `partitions.csv`, `specs.md:681` and `specs.md:1006`. The finding's own ~90 B was high. `log.rs` and `console.md` were already right — they size on the event row, and the "about an hour" figure checks out at 8 events/s. |
| L-D4 | FIXED | The AS-BUILT sample carries all 11 columns and a second line showing a non-strike row. The prose now explains `millis`, `kind` and `nf` alongside `simulated` and `strokes`, and states why the strike-only columns are empty rather than zero — and that `distance_km` holds the words `overhead` and `far`. |
| L-D5 | FIXED | The count chart became the strike table, so the comments describing two stacked charts are rewritten to say which one survived and why. **The dead `counts` buffer is removed**, not documented: 576 B held across redraws and a `copy_from_slice` per redraw with no reader. `series_of` still reports counts — the host checks cover them — but they now die with the stack frame. Also removed a stray duplicate doc line on `Frame::recent`. |
| L-D6 | FIXED | The 13-bit/11-bit/8192/13-probe archaeology corrected in `defence.rs`, `settings.rs`, `console.rs`, `tuning.rs` and `session.rs`, each rewritten to keep the argument and fix the arithmetic. `defence.rs`'s "it began as one 13-bit integer" is left — it is under "Why the space kept shrinking" and is true as history. **One real error found beyond the finding:** `force_max_sensitivity` claimed "every rejection knob at zero" when `Point::OPEN` is `NF_LEV = 0` alone — `WDTH` and `SREJ` keep their settings. `sensitive on` is less open than its own documentation promised. |
| L-D7 | FIXED | The copy-era line is gone from `tests/history.rs`, which contradicted itself six lines later. `defence.rs` moved to edition 2024 with the rest; `history.rs` gained the run line it lacked. The README now marks the written-out editions as the stale-able copy and names `check.sh` as the authority, and its file table lists all eight tests rather than two. |
| L-D8 | FIXED | The hand-written "83 checks" is replaced by a note saying the count is deliberately not kept here because `check.sh` prints it — which is the same argument the project makes about netlist counts. The suite is now 209 checks across eight files; the stale figure named two. |

Two things the sweep turned up that were not in the findings:

* **`force_max_sensitivity` over-promised.** Its doc said "every rejection knob at zero"; `Point::OPEN` sets `NF_LEV = 0` and leaves `WDTH`, `SREJ` and `MIN_NUM_LIGH` at their stored settings. Documented as it is rather than changed — opening the others is what `srej 0` and `wdth 0` are for, and widening `sensitive on` silently would be a behaviour change hiding in a doc fix. **Raised as L-N2.**
* **`wdth` had no row in `console.md`'s command table** though the console has accepted it, aliased `watchdog`, for some time. Added.

## Verification pass, 2026-08-28 — grounds before fixes

The owner asked for strong grounds before acting on the second pass. Every one of
its 22 findings was re-checked against the source independently rather than taken
on report. **All 22 confirmed**; two were sharpened and two more were found.

| Finding | Ground |
|---|---|
| L-P1 | **Reproduced, not argued.** `power::decide` extracted to a pure `src/policy.rs`, and `tests/policy.rs` written against the unchanged logic first: it failed 3 of 12 — Awake an hour, a day and a week past the wrap. The fix turns those green. Traced the caller to confirm the wrap is real: `power::decide(now_ms() / 1000, …)` and `now_ms()` is a u32 millisecond counter, so `uptime_s` wraps at 49.7 days. |
| L-P5 | Threshold computed exactly: overflow above **256 003**, not ~255 934. The console parses an unbounded `u32` (`console.rs:349`). Fixed by clamping to the physical ceiling — a 20-bit energy field, 62 500 milli — rather than widening to `u64`, because a faithful `u64` answer is still an energy no strike can have. |
| L-P7 | Proved by removing the `#[allow(dead_code)]` and rebuilding: no warning appeared, so `clear()` is live via `Command::Clear`. |
| L-P4 | Read the arm: budget 1 prints the notice and returns **before** appending, so `events 1` logs nothing and `events N` logs N−1. |
| L-P3, L-P6, L-P8 | Confirmed at the named lines. L-P8's mechanism verified end to end: `due()` is `!self.frozen && …`, `noise_per_min` is written only inside `step`, and the screen reads it at `screen.rs:181` → `ui.rs:259,285`. |
| L-T1 | Confirmed and it is the dangerous one. The guard is `[[ "$PORT" == /dev/serial/by-id/* && "$PORT" != *"$BOARD_MAC"* ]]` — **both** must hold to refuse, so a bare `/dev/ttyACM0` fails the first and proceeds to `esptool erase-flash`. The comment directly above claims the opposite. |
| L-T2 | Confirmed the fatal sequence: `flash.sh:77` runs a bare `cargo build --release` into the same `target/…/release` that `recover.sh:86` reads, so a failed flash leaves exactly the light-sleep binary that broke the console, and recover writes it back. |
| L-T3, L-T5, L-T6, L-T7, L-T8 | Confirmed at the named lines. |
| L-T4 | Confirmed both omissions matter: `.cargo/config.toml` pins `ESP_IDF_VERSION`, the `espidf_time64` rustflag, the MCU and the partitions.csv delivery globs; `components_esp32c3.lock` pins littlefs by hash. Both tracked, neither in `SOURCES`. |
| L-D9..L-D11, L-D13 | Confirmed. L-D9, L-D10 and L-D11 are **misses in the 0.13.6 sweep** — it fixed module headers and one method each, leaving the rest. |
| L-D14 | Confirmed, and it is a **regression introduced by the 0.13.6 sweep**: `check.sh` prints per-file counts and no total. |

Corrections to the second pass's own figures: L-P5's overflow threshold is 256 003,
not ~255 934. Everything else it reported was accurate.

## L-T1, demonstrated rather than reasoned about

The guard was `[[ "$PORT" == /dev/serial/by-id/* && "$PORT" != *"$BOARD_MAC"* ]]`
— both halves had to hold to refuse, so a bare `/dev/ttyACM0` failed the first
and went straight to `esptool erase-flash`.

Run against the live bench while fixing it, `/dev/ttyACM0` was **the C5 panel
candidate**, `10:BD:A3:CF:3D:A0` — not the lightning board, which was on
`ttyACM2`. The old path would have erased the C5's whole flash. The new guard
resolves any given path back to its by-id name and names both boards when it
refuses. `RECOVER_SKIP_IDENTITY_CHECK=1` remains as a deliberate override for a
board udev has not named.

The same bypass existed in `flash.sh` and `release/flash.sh` — both advertised
`./flash.sh /dev/ttyACM1` in their own usage — and is closed the same way.

## The access point, 2026-08-29

L-P2 is closed by wiring `press` into the listen loop, which was the prerequisite
for the long-press gesture the access point needs. The loop now samples the pin
every 50 ms while it is down; it previously measured no duration at all.

Two defects found while bringing it up, both by checking rather than by running:

* **`httpd_config_t.task_caps` was zero.** Filling the struct by hand defaults
  every unnamed field to zero, and this one is the capability mask the server
  task's stack is allocated with. `httpd_start` returned `ESP_ERR_HTTPD_TASK`,
  which reads as "out of memory" -- the heap print added to diagnose it showed
  **166 080 B free**. The IDF's own `HTTPD_DEFAULT_CONFIG` uses
  `MALLOC_CAP_INTERNAL | MALLOC_CAP_8BIT`.
* **The join screen's credentials line would have been silently truncated.**
  An SSID may be 31 characters and a password 63; both on one line is 117, which
  is 1053 px in a 9-px font on an 800-px panel *and* cut to 64 by a
  `heapless::String<64>` whose `push_str` error is discarded. Split onto two
  lines and measured: the longest is 657 px. Found by following the project's
  own rule about measuring a layout before flashing it -- which had already been
  broken once by flashing this screen without a preview.

QR geometry was checked the same way rather than by eye: the real `WIFI:`
payload is version 4, 33 modules, drawn at 5 px per module; the URL is version 2
at 7 px; the worst case a 31-character SSID and 63-character password can
produce is version 7 at 4 px. All three fit the 240 px box at an integer scale
with a four-module quiet zone.

**Not verified: the page itself over HTTP.** Fetching it would mean joining this
host to the device's access point, and `CLAUDE.md` forbids touching the host's
WiFi configuration. What is confirmed is that the server task starts, the
credentials are generated and shown, the join screen renders, and the window
closes on time. The query parser has 29 host checks. The rendered page has none.

## L-N3 — an old header silently drops every new row

**[RAISED AND PARTLY FIXED 2026-08-29.]** Found while checking the device before
a forecast storm, from a count that did not add up: `2092 records, ... replayed
2090`.

`Columns::from_header` takes its positions from the header **on disk**. A file
created before `millis,kind,nf` were added has `distance_km` at index 2; in a
row written now, index 2 is `kind`. `"lightning"` fails the `u8` parse and the
row becomes `Row::Skip`. So a device whose log predates the format change
replays its *old* strikes and silently drops every strike recorded since —
they are in the file, and invisible to the charts, the recent-strike table and
anything else fed by the replay.

The boot warning said "rows still parse; use `clear` to start fresh", which is
the opposite of true and reads as cosmetic. It now names the consequence, both
column counts, and the `dump`-then-`clear` procedure.

**Not changed: the behaviour.** `ensure_header` deliberately refuses to rewrite
a file to correct a label, because that would discard records; `clear` is the
documented way out and a `dump` beforehand keeps everything. Making the reader
fall back to counting columns per row would be a second parsing convention
living alongside the header, which is the drift this design exists to prevent.

The live device is affected: 2092 records against a 2091-row 8-column body and
2 new-format rows. Backed up to `~/lightning-backups/` before any clear.
