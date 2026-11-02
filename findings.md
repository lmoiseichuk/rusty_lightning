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

## The storm of 2026-11-01 — what it measured, and what 0.21.0 does with it

Measured live on 0.20.1 at indoor gain, `nf 3`, `wd 0`, `sr 1`, antenna 501 kHz,
IRQ confirmed. One home per finding, as above: re-verifying one edits its row.

| ID | What was measured | Where | Status |
|---|---|---|---|
| L-S1 | **Indoors, awake (`freq 160`), disturbers unmasked: 6–10 disturbers a second, flat through audible thunder, 0 strikes.** `srej 4` and `wdth 2` did not move the rate. Event gaps median 110 ms, floor ~60–80 ms — a continuous local source, the chip re-triggering as soon as it recovers. No strike recorded indoors since 2026-09-24. Outside on the cell the same evening: 13 strikes in 11 minutes, 6–14 km plus six `overhead`, one matching thunder LM heard. | the room, or the board | **OPEN.** Two variables changed at once between those two runs — the room *and* the supply. The missing cell of that 2×2 is indoors on the cell. Note `session.rs`'s August table already has `200 kHz, battery, wd 0: 0 events` against `5–8/s` on USB, and the driver board's ETA9740 switches an inductor whenever USB is charging the cell — the AS3935 datasheet's first named noise source is "inductor based DC-DC converters". The panel's own step-up is the other suspect: L-S3. |
| L-S2 | **Unplugging USB reset the board** instead of running it on its cell (94 %, charging). | the driver board's power path | **HARDWARE, very probably — and now provable.** No firmware path acts on the supply. The cell reaches the rail only through the ETA9740 changing from charger to boost, which its datasheet gives no time for. Lowest IDF brownout level is already in use (~2.51 V, below the C3's 3.0 V and the flash's 2.7 V minimum); disabling it is not safe. 0.21.0 keeps the last four reset reasons in NVS and prints them at boot and in `health`, so the next unplug names itself: `BROWNOUT`, `power-on`, or firmware. Full argument: specs §7. |
| L-S3 | **The `overhead` strikes come 6–9 s after another strike.** 2026-09-16..24 logged 9,542 strikes, 99 % overhead, round the clock, gaps almost never under 5 s, peak 7–10 s. False. Suspect: the e-paper refresh (a strike earns a redraw five seconds after the last one, and a refresh is 3.8 s) read as a close strike — its step-up converter (L1 10 µH + Q1 on the driver board) is never powered off, because `epd-waveshare`'s `sleep` is never called — or just the redraw blocking the loop. | `display.rs`, `screen.rs` | **OPEN, now testable.** 0.21.0 powers the panel down after every refresh (`panel sleep on`, the default) and wakes it before the next; `panel sleep off` restores the old behaviour, saved and on the web page, so the A/B is one setting on one board. Wake cost from the driver: 190–220 ms of reset delays plus an unmeasured `PowerOn`, printed per redraw as `woke in N ms`. What it cannot separate: a false strike caused by the refresh waveform itself still happens with sleep on. If `overhead` strikes keep landing 6–9 s after real ones with the panel asleep between refreshes, the converter running idle is cleared and the refresh, or the loop blocked by it, is what is left. **0.22.0 (L-A1): whatever their cause, they no longer teach the device anything** -- still logged and shown. The cause is still open. |
| L-S4 | **RAM-only diagnostics are lost on every reset, and every port open resets** (`rst:0x15`, even with DTR/RTS low). `mask off`, `events N` and `freq 160` all vanish the moment anyone connects to look, and a test that needs the cable out loses them too. | `effects.rs`, `log.rs` | **FIXED in 0.21.0 for the two that are experiments.** `mask` is saved and applied at boot (default masked); an `events` budget is saved and counts down across restarts, written with the sync that writes the rows it paid for. Both are on the boot banner and in `status`, so neither runs silently. `freq` stays RAM-only on purpose: a board back from a power cut refusing to sleep would flatten the cell unnoticed. The one-shot `echo > port` recipes this made wrong are fixed in 0.21.1 — see L-R8. |
| L-S5 | **Asleep, the IRQ is not a wake source.** The build has `CONFIG_PM_SLP_DISABLE_GPIO` (selected by the C3's GPIO-reset workaround), so in automatic light sleep GPIO21 and GPIO9 are input-disabled and an event waits for the ~1 s timer wake. The AS3935 reports only the latest reason, so a second event in that second overwrites the first. | `main.rs`, `power.rs` | **FIXED in 0.21.0.** GPIO21 is a light-sleep wake source on a high level, exempted from the sleep isolation by `gpio_wakeup_enable` itself (read in the IDF 5.3.4 source; the isolation is applied once at start-up, so `esp_pm_configure` does not undo it). The C3 shares the trigger field, so the interrupt is level-triggered too — which also removes the missed-edge failure the rescue poll exists for. Cost: one wake per event, ~one 10 ms tick each — masked ≤ ~1/s (≤ ~1 % awake), unmasked 6–10/s (6–10 %, at worst about doubling the frugal draw). The button is left on its edge (DTR). Not verified on hardware: that the C3 actually wakes, and that the level ISR never runs away. The review of 0.21.0 found three holes in that reasoning, fixed in 0.21.1: the settle did not wait (L-R1), a failed read lowers nothing (L-R5), and the masked cost was a threshold read as a cap (L-R4). |
| L-S6 | **Web UI: `indoor gain`, `outdoor gain`, `sync`, `reboot` and the `quiet` field send words not in `query::COMMANDS`** and are silently dropped by the allow-list before the parser ever sees them; the boot check only verified the list, not the page. `open up and freeze` is exactly `sensitive on` — `nf 0` frozen, the setting the README measured at 300–480 noise/min and zero strikes indoors. The `tz` field said minutes and showed `-300`, which the console's `tz` (hours, −12..14) refuses. | `webui.rs`, `query.rs` | **FIXED in 0.21.0.** The page is built from `query::ACTIONS`, `FIELDS` and `TOGGLES`; a `const _: () = assert!(..)` over them refuses to **compile** an entry `COMMANDS` would drop (mutation-checked: adding a `reboot` action, or narrowing `sensitive` to `on`, stops the build), and the boot check now parses every line the page can send, not just the allow-list. Gain buttons send `mode indoor|outdoor`; `sync`, `reboot`, `quiet` removed — no such commands. The allow-list passes only `sensitive off`, and the page shows it only while frozen; `sensitive on` stays a console command. `tz` takes hours. New: `mask` and `panel sleep` toggles, the remaining `events` budget, and the restart reasons, so a board on its cell can be run without a cable. The rendered page itself is still untested over HTTP (the host's WiFi is off limits); it was rendered offline against a stub snapshot and the forms read back. |
| L-S7 | **Bare `events` armed 20,000 rows** (~58 B each, ~1.2 MB) against ~0.87 MB free: it could fill the log's filesystem, after which every write fails — strikes included. | `console.rs` | **FIXED in 0.21.0.** Bare `events` reports and arms nothing. Every budget — typed, from the web page, or restored at boot — is capped by `budget::capped` to the free space less 256 KB kept for strikes, at 65 B a row, the longest the format writes (the row is now built in `budget` so the size and the writer cannot drift). `tests/budget.rs` checks across 0–4 MB that no budget reaches the reserve; mutation-checked by dropping the reserve and by shrinking the row size, both caught. |
| L-S8 | **`tools/check.sh` had refused to build since `09a1696`**: `tests/defence.rs` still asserted the watchdog default of 2 that commit deliberately moved to 0, and the module map was three commits stale. The board's 0.20.1 is not a stamped release — `release/` stops at 0.20.0 — so it came from `flash.sh`, which does not run the checks. | `tests/defence.rs` | **FIXED** — the check follows the default, with the storm measurement beside it. |
| L-S9 | **The RC-oscillator calibration disagrees with the datasheet twice.** `power_up` pulses `0x08` bit 5 (`DISP_TRCO`) where the datasheet's procedure pulses bit 6 (`DISP_SRCO`); and it calibrates before the 120 pF tuning capacitance is written, where the datasheet recommends trimming the antenna first — so the oscillators are calibrated against the LC oscillator at 0 pF, off its 500 kHz. Signal validation is timing. **A third, found with L-R1:** its two "wait 2 ms" steps, and `reset`'s, are `FreeRtos::delay_ms(2)` — one tick, so 0–10 ms, and sometimes almost nothing. *(Corrected by L-V8: only the untimed gap between `CALIB_RCO` and the bit going up was short by chance; the timed step 3 followed the first delay's return on a tick and ran ~9.7 ms every boot. `reset`'s 2 ms did start at an arbitrary point.)* | `as3935.rs`, `boot.rs` | **FIXED in 0.22.0, on LM's request rather than a storm's evidence -- the bit and the order; the timed wait was already long enough and is now guaranteed (L-V8).** Verified against datasheet v1-04 (the PDF fetched this session, ams 2016-01-13): page 36 "Clock Generation" -- *"(REG0x08[5] =1 TRCO, while REG0x08[6] = 1 SRCO) ... It is recommended to first trim the receiver antenna before the calibration of both oscillators is done ... 1. Send Direct command CALIB_RCO 2. Modify REG0x08[6] = 1 3. Wait 2ms 4. Modify REG0x08[6] = 0"*; the register map, pages 18-19, bit 7 `DISP_LCO`, 6 `DISP_SRCO`, 5 `DISP_TRCO`; page 9, `LCOSUT` 2 ms and `TRCOCAL` 2 ms, *"starts after the LCO settles"*. The ScioSense reissue carries the same text one page later. `power_up(caps)` now: PWD clear, `TUN_CAP` 120 pF, 2 ms for the LC oscillator, `CALIB_RCO`, bit 6 up, 2 ms, bit 6 down; boot's trailing `set_tuning_caps` is gone. Every wait through the new `as3935::wait_at_least_ms` -- the IRQ settle's shape, one yielded tick and a timed busy-wait of the shortfall, so 2-10 ms and never under -- and `settle` is now that helper at 8 ms; `reset`'s 2 ms uses it too. The old comment at the constant ("TRCO is the bit `CALIB_RCO` actually requires", and a warning against "correcting" it to `0x40`) had no source and is replaced by the datasheet quotation. The banner keeps reading `0x3A`/`0x3B`, and `antenna_self_test` still runs after the calibration in `main`. **Check on the first boot:** 0.21.2 read `TRCO done, SRCO done (0x3A=0x9C 0x3B=0x9E)`; anything but done twice is the evidence to revert, and `MASK_DISPLAY_SRCO` is the bit change entire. No host check: it is register order over I2C, which nothing on the host can drive. Full note: specs §3. |

## Review of 0.21.0, 2026-11-01 — before it was flashed

Nine findings against the 0.21.0 commits, each re-checked against the source,
the IDF 5.3.4 tree and the datasheet before acting. All nine real; one (L-R3)
is a measurement to make rather than code to change. Fixed in 0.21.1.

| ID | What | Where | Verdict |
|---|---|---|---|
| L-R1 | **The 8 ms settle could last almost nothing.** `FreeRtos::delay_ms(8)` → `TickType::new_millis(8)` = 1 tick at 100 Hz (esp-idf-hal 0.46.2 rounds up), and `vTaskDelay(1)` returns at the next tick: 0–10 ms, ~1 event in 5 inside the datasheet's 2 ms (v1-04 p.34). Always so on USB; on the cell it barely arose until the IRQ woke the chip. The batch-end poll also read with no settle while an armed level ISR was about to read the same event. | `session.rs`, `listen.rs` | **FIXED.** `as3935::settle`: one yielded tick, then a busy-wait of the shortfall on `esp_timer` — 8–10 ms, never under, spin ≤ 8 ms. `vTaskDelay(2)` was rejected: 10–20 ms awake per event on the cell, and further from the reference's 8 ms. The poll now runs only while INT is low; found with INT low, an event is a broken IRQ line, and the line says so. |
| L-R2 | **Panel wake and power-down lengthen every redraw's blind time, and their BUSY waits never time out.** Docs gave the quiet-weather cost only; in a storm (~400 redraws/h) it is 100 s/h or more, 43 % → 45 % of the loop. `epd-waveshare`'s `wait_until_idle_with_cmd` has no bound, and `PowerOn` now runs every redraw. | `display.rs` | **FIXED.** Every driver call (init, wake, frame, power-down) runs under `Panel::bounded`: `BusyPin` reads idle once `DRIVER_WAIT_LIMIT_MS` (2 s) has passed and stays idle for the rest of the call, and the console says which call gave up. The power-down is timed and printed (`powered down in N ms`). Storm cost stated in `display.rs`, specs §6 and console.md. The pre-existing unbounded wait before every frame is bounded by the same change. |
| L-R3 | **The level trigger removes the old cap on observed noise counts, and the tuner's thresholds were fitted to capped counts.** `climb` and `golden::is_swamped` (5×60/min) act on a count whose meaning changed. | `main.rs`, `tuning.rs`, `golden.rs` | **SETTLED in 0.22.0 by arithmetic and the day's logs -- half real, and the half that matters runs the other way.** What bounds the count is the chip's re-raise after 0x03 is read: on 0.21.x, 196 `NoiseTooHigh` gaps p10 74 / median 90 / p90 94 ms, against a loop that services an event in 8-10 ms, so ~11 a second, ~660/min. *Awake*, 0.20's edge re-armed microseconds after each read in an ordinary pass and caught every re-raise; it lost the rest of one batch (≤ 1 s, ~11 events) after a pass that blocked past ~90 ms -- a redraw (≤ 2 a minute without strikes, the 30 s floor), a sweep's 500 ms settle -- so it read ≥ 58/60 ≈ 97 % of the level count. Measured awake at 160 MHz, disturbers unmasked (same INT path): 0.20.1, 5,072 gaps, median 110 ms, p90 190, three ≥ 1 s; 0.21.x, 5,699 gaps, median 118, p90 220. *Asleep* (frugal, the unattended state even on USB once the console and storm holds lapse), the pad was isolated and not a wake source, the chip slept within 3 ticks of each batch's work, and each 1 s wake read 0x03 once: ≤ 60/min, **the quiet threshold exactly**, so an unattended 0.20 board could never call a window noisy. The thresholds were all fitted awake (60/min on 2026-08-13 from console sessions at 90-150/min; the drowned multiple from 595/min at nf 0, impossible under the asleep cap, and 300-480/min under `sensitive on`). The level counts the awake quantity asleep too, so **the tuner's input is unchanged**. Kept visible for field comparison: `status` prints the raw count since boot, the last window's rate, and how many of its batches heard noise (the 0.20-asleep observable); `events` rows carry arrival milliseconds. `tests/verdict.rs`: the verdict reads the raw rate (two a batch is noisy, one a batch is not, with the same batches noisy), the batch counts; mutation-checked (verdict on batches, every batch counted noisy, batches uncounted, noise added as batches -- each caught). scanning §4-5, specs §7 and `main.rs` rewritten to the arithmetic. |
| L-R4 | **The masked wake cost used the 60/min quiet threshold as if it capped the event rate.** INT_NH "persists as long as the noise level is above the threshold" (datasheet); sweeps, `sensitive on` and a room noisy at the top rung all give 5–10 wakes/s masked. | `power.rs`, specs §7 | **FIXED (doc).** The masked row is split: held under threshold (≤1 %), and not held (5–10 %, the unmasked figure). |
| L-R5 | **A failed reason read made the level ISR re-fire every pass: no sleep, a line every ~10 ms.** `read_and_handle` returned on I2C error with INT still high. Worse than the finding said: the *wake source* stays armed on a high level too, so even a disarmed interrupt would not let the chip light-sleep. | `session.rs`, `listen.rs`, `power.rs` | **FIXED.** `collect` returns whether it read; on a failure `listen` leaves GPIO21 disarmed **and** calls `power::hold_irq_wake` (`gpio_wakeup_disable`) until the batch closes, then re-enables both. Once a second while the fault lasts; the first failure and the recovery are each reported once. |
| L-R6 | **The light-sleep safety argument misread the IDF's GPIO-reset workaround.** IDF 5.3.4 Kconfig: the C3 resets at wake if an *input-only* pad gets a >6 V electrostatic pulse; floating is only mentioned as bad practice. GPIO21 is the one input-only pad now left un-isolated, on a lead, on a lightning detector. | `power.rs`, specs §7 | **FIXED (doc).** The argument now says what it relies on (a driven line makes the pulse unlikely, not impossible), what it costs (~15 s deaf, a restart that is neither BROWNOUT nor power-on) and the unplug triage names it. The trade is kept for the wake. |
| L-R7 | **A failed budget save was never retried once rows stopped**, so a stopped `events` window could come back after a restart; and the console said "(saved)" for a disarm regardless, while the arm path said "a restart will lose it" for a disarm it would *restore*. | `log.rs`, `commands.rs` | **FIXED.** `sync` saves a moved budget before its nothing-pending return (pending empty = every paid row on disk, the one moment the two may agree), through one `save_budget` that reports a failure. `arm_events` returns `Armed { rows, saved, on_disk }` and the command words the failure by direction. `Log::open`'s re-cap goes through the same path. |
| L-R8 | **The docs said every port open resets the board and still taught one-shot `echo > port`** — including the clock-set flash.sh prints after flashing. `echo "dump" > port > storm.csv` never worked at all: the second redirection wins. | `doc/console.md`, `README.md`, `doc/scanning.md`, `release/flash.sh`, `flash.sh`, `tools/watch.py` | **FIXED.** New `tools/send.py`: one held open, waits for the banner and then `con:  listening` (a new line the loop prints as it starts reading), or wakes a sleeping PHY with a newline; `--clock` reads the epoch at send time; `--until` for `dump`; refuses a port that is not the board. Tested against a pty that boots, and one that is already running. Every recipe uses it or a reader that holds the port; scanning §9's sweep is one held session (and says `defence`, not `place`, plus `mask off` and `sensitive on`, without which its curves were empty or walked away). `watch.py`'s "passive observer" claim corrected. Shipped in `release/` beside `board.sh`. **Not verified:** that a bare `echo` restarts the board — the recipes no longer depend on it either way. |
| L-R9 | **The unmasked banner warned of "missed edges", a mechanism the same release removed.** | `boot.rs`, `effects.rs`, `commands.rs` | **FIXED.** It names the two real costs: a disturber can replace a pending strike's reason before it is read, and on the cell each one is a wake. |

## The task watchdog in a long walk, 0.21.1 on the board, 2026-11-01

Measured on 0.21.1 (`055ae8a`), awake and pinned at 160 MHz, read through
`tools/send.py`. The watchdog is `CONFIG_ESP_TASK_WDT_TIMEOUT_S` = 5 with
`CONFIG_ESP_TASK_WDT_PANIC`, and it watches the idle task only, which runs only
when every other task is blocked. Fixed in 0.21.2.

| ID | What | Where | Status |
|---|---|---|---|
| L-W1 | **`dump` reset the board through the task watchdog once the log passed ~1.32 MB.** 15:38: a dump of 17,727 rows (1,311,568 bytes) finished. 15:48, 18,938 records (18,880 on disk, 58 pending): the output stopped mid-row after 17,855 rows (1,319,116 bytes), a register dump followed, and the board came back with `boot: TASK WATCHDOG`. The same day 0.20.1 had dumped 9,634, 12,474 and 14,726 rows (up to 1,146,834 bytes). All five fit one rate, ~264,000 bytes a second (~3,570 of that log's rows, 280 µs each), against the 5 s limit -- the 15:38 dump finished just inside it, the 15:48 one needed 5.3 s -- so it is a size the growing log crossed, not a 0.21 regression. The register dump (`ELF file SHA256: 7a9478b7b`, the stamped 0.21.1 image) puts the core in `usbjtag_tx_char_via_driver` under `println!`'s flush: writing, not blocked. A console write blocks only when the driver's 256-byte TX ring is full -- IDF 5.3.4's `usb_serial_jtag_vfs.c` offers each character without waiting first -- and a host that keeps up never lets it fill, so whether a dump survived depended on the reader as much as on the log. | `log.rs` `dump_csv` | **FIXED in 0.21.2.** New `src/pace.rs` (pure, `tests/pace.rs`): every walk over the whole file blocks for one tick (`vTaskDelay(1)`, at most 10 ms) once it has kept the CPU 500 ms -- a tenth of the timeout, which the test reads from `sdkconfig.defaults` (now written out there) and holds the interval to; the test also holds the block to at least one tick (zero is a yield, which never lets the idle task in) and under `CONFIG_FREERTOS_IDLE_TIME_BEFORE_SLEEP`, so it cannot become a light sleep that drops the port. Timed rather than counted in rows, because the rate moves with the clock and the reader. The idle task is then kept out ~500 ms at most however long the log; the cost is at most one tick in fifty, ~2 %. The output is byte-identical, `# end` included. **Expected:** time ≈ bytes ÷ 264,000 + 2 % -- 5.4 s for the 18,938 records above, ~6.5 s at ~23,700 rows if that `events` window has run out since, under 8 s for a full partition. Mutation-checked: an interval of 1 s, a block of 0 or 3 ticks, a pacer that never restarts or never fires, `saturating_sub` for the wrap, and the timeout deleted from or shortened in `sdkconfig.defaults` each fail a check. **Not yet verified on the board.** |
| L-W2 | **The other walks over the whole log, and every other long loop, checked against the same limit.** | `log.rs`, `portal.rs` | **Three paced, three cannot reach it.** *Paced:* the record count in `Log::open` and the replay into the charts, both at boot -- 0.20.1's stamped boots (`storm_20261101.log`) show each walking ~0.94 MB inside one host second, so a full partition (~1.97 MB usable) is under ~2.1 s, inside the limit on that evidence; but the evidence is one-second resolution and a reset there repeats on every boot, so they are paced at no cost to a short log (a walk under 500 ms never blocks). And the web page's `/log.csv`, on the server task at priority 5 above the loop's 1: it blocks when lwIP's 5,760-byte send buffer is full, which depends on the link, and nothing has measured the transfer. *Not paced:* `sync` writes only the pending rows -- unmasked indoors at 6–10 events a second it synced 471–488 rows at a time on 0.20.1 with the 1 s batch lines carrying on either side, and the chip's ~60–80 ms floor between events (L-S1) caps a minute near 1,000 rows, ~60 KB; the IDF also blocks a tick during flash erases (`CONFIG_SPI_FLASH_YIELD_DURING_ERASE`), which littlefs does before each 4 KB block it writes. `clear` is one littlefs metadata commit (`lfs_remove_` writes a delete tag and never visits the file's blocks), a header row and a one-line count, whatever the log's size. The `events` budget is arithmetic over `free_bytes()`, which is `lfs_fs_size` walking the block list -- at most 496 blocks (2,031,616 / 4,096) of small reads, milliseconds, and paid on every redraw already. Also checked: the panel's BUSY waits poll at 100 ms, the antenna self-test spins for 100 ms, the IRQ settle blocks a tick before it spins, console tables are bounded. |
| L-W3 | **The record count never ended on a read that kept failing.** It was `lines().skip(1).filter(..).count()`, and `lines()` does not stop at an error: it returns it and tries the same read again, so a persistent read error counted for ever -- a watchdog reset at every boot unpaced, and a boot that never finished once paced. | `log.rs` `recount` | **FIXED in 0.21.2, with L-W1.** Now a loop: a non-UTF-8 line is skipped, as the count always did (`read_line` has consumed it); any other error ends the count, as the dump and the replay already stop at their first. Never observed -- it is the pacing that made it matter. |

## What the device learns from, 0.22.0, 2026-11-01

The evidence is the day's logs (`after_0.21.2.txt` holds the board's whole log
as of 18:00: 20,629 rows, 9,650 of them strikes) and the measurements in the
storm table above. Four changes, most valuable first, each its own commit.

**The first 0.22.x boot on the board as it stands** (rewritten for 0.22.1 --
L-V7) -- old record most likely `nf 3 wd 0 sr 1 (indoor), 2 strikes`, `srej 1`
saved, mask on, events off. *Not 14*: the record read 14 at 14:36
(`boot_0.21.1.txt`), and 0.21's replace-on-change rule then rewrote it from the
afternoon's three `overhead` -- 15:19:00 at nf 4 made `nf 4, 1`, 15:33:13 and
15:34:31 at nf 3 made `nf 3, 1` and then `nf 3, 2` -- with no later strike in
`after_0.21.2.txt`. So the nf 3 floor that 0.22 drops rested on two detections
tied to electrical events, and keeping the old record as an untrusted floor is
declined. Expected, in order: `as: RCO calibration -- TRCO …, SRCO …` (compare
with 0.21.2's `done, done (0x3A=0x9C 0x3B=0x9E)`; anything but done twice is the
evidence to revert the bit, and the order is the other candidate -- the step-3
wait is not, L-V8); `disturbers masked (saved)`; `gold: the record from before
0.22.0 (nf 3 wd 0 sr 1, 2 strike(s)) counted `overhead`` and `gold:   as
lightning -- set aside; …`; `resumed defence point from NVS` (no record, so no
floor and no raise); the warning that spike rejection 1 was saved by hand and
sits below the default of 2, its `srej 2` line, and since 0.22.1 a third: `type
it before two strikes with a distance arrive at 1, or the record they make
brings 1 back at every restart` (L-V6); then `spike rejection 1`; `watchdog
threshold 0` (every boot logged since the morning's `wdth 2` test
reads 0, the last in `after_0.21.2.txt`); the
self-test as before. Then the walk runs with no heard-at floor until a strike
with a distance writes a new record (trusted at the second); `overhead` strikes
print `[unconfirmed -- not learned from]`, **hold the walk** (0.22.1, L-V1) and
do nothing else; and `status` carries the noise line. **No command is
required.** `tools/send.py --clock` after flashing, as always. `srej 2` is the
one to type to rest at the new default -- **before** two strikes with a distance
have been heard at 1, or after `golden clear` (L-V6); `golden` shows the
set-aside record.

| ID | What | Where | Status |
|---|---|---|---|
| L-A1 | **`overhead` taught the device as much as a strike with a distance.** The `overhead` that could be checked was false on these days -- and real on others, which this row first missed (L-V2): 2026-08-14 at `srej 0`, 503 from an electric hammer, all `overhead`; 2026-09-16..24, 9,542 strikes, 99 % `overhead` (9,553 of the 9,623 September rows), round the clock, 7-10 s apart -- and they wrote the golden record ("840 strikes heard here"); 2026-11-01 outside on the cell, 13 strikes in 11 minutes, the 7 at 6-14 km including one LM heard, the 6 `overhead`, three of them 6-9 s after another strike and three 44-66 s after (this row first said all six were 6-9 s; L-V2), and three more `overhead` that afternoon matching a USB plug-in to the second, the end of a short run on the cell and one mid-run, none matching thunder. The nine `overhead` that day carried 98,724-514,552 raw (eight above 290,000) against 12,842-150,818 for the seven with a distance. That day's record went to "trusted, 14 strikes" partly on them. **Every place a strike fed learning, read:** `session::commit_merged` -> `remember_working_point` (the record and its count, so its trust); `Tuning::observe` -> the window's strikes (the hold, `heard_strikes` that skips the dip, `judge_dip` keeping a dip open) and `last_strike_ms` (the rescue's twenty minutes of silence); `Tuning::new` and `boot::configure` seeding the floor and the start point from the record; `Tuning::open` returning to it. Not learning, and left counting every strike: the totals, charts, table and log; the screen's strike floor; `power::decide`'s awake hold; `StormWatch` (it clears the chip's own statistics, which hold those detections, and gates the sweep, where a doubtful strike errs toward not mis-setting). | `golden.rs`, `verdict.rs`, `session.rs`, `tuning.rs`, `settings.rs` | **FIXED in 0.22.0.** One rule, `golden::is_evidence`: `overhead` is not evidence, a kilometre reading and `far` are (`far` is an answer about distance, and not one of the 9,650 strikes on the board reads it). `verdict::Batch` (moved from `session`) splits every lightning interrupt into a strike for the console and, when its distance is evidence, a confirmed one; an unreadable distance counts as unconfirmed. `Window::fold_batch` carries only confirmed strikes into `Window::strikes`, the one count the dip and the rescue read; the rest ride in `Window::unconfirmed`. **Revised in 0.22.1 (L-V1): the hold reads both again** -- `Window::walk` holds for any lightning interrupt, because a hold cannot deafen and not holding under a real storm overhead can. The record takes a flash through `golden::may_record`, judged on its *recorded* distance: an `overhead` stroke merged with a kilometre stroke is recorded at kilometres and counts, as the kilometre stroke would alone under `merge 0`; an `overhead`-only flash, or `overhead` with `far`, is recorded as `overhead` and does not -- contradictory readings inside one window, and it keeps `distance_km` an exact record of what taught the device. **Revised in 0.22.1 (L-V5):** a flash counts if any of its strokes is evidence, so `overhead` with `far` counts for the `far` stroke. **The log is unchanged**: the column already says `overhead` for exactly the rows that taught nothing, and a new column would trip `ensure_header`'s stale-header warning on every existing log and break any reader of a dump whose 12-column header sits over 13-field rows. The console marks each one `[unconfirmed -- not learned from]`; the batch line adds the count at its end. **The old record is set aside**, not migrated: its count cannot be split into located and `overhead` strikes, so 0.22.0 keeps its own under `golden2`/`golden2_n` (renamed as `defence3` was) and leaves `golden`/`golden_n` in place for `golden` to print and for a board taken back to 0.21; boot says so when the old record is the only one; `golden clear` removes both. Host checks: `tests/golden.rs` (the rule, the mixed flashes through the real merger, 840 September `overhead` writing no record, the November afternoon making a record of 7, fifty `overhead` after one located strike staying at 1) and `tests/verdict.rs` (the split, the window, an hour of September's stream never reading as lightning). Mutation-checked: `overhead` made evidence, `far` made not, `may_record` dropping either half, an unreadable distance made evidence, `confirmed()` returning every strike, `fold_batch` passing `batch.strikes`, `fold_batch` dropping the unconfirmed count, `heard_lightning` counting unconfirmed, and the split removed -- each caught. **Not verified on the board.** |
| L-A2 | **The resting spike rejection was 1, and 1 let a man-made stream through for nine days.** The default was "the least rejection that is not none", chosen between measurements rather than at one: at 0 the hammer's 503 in 3.5 h (all `overhead`); at 2, in August, five false in eight hours and 5-6 real strikes an hour missed at 10+ km. At 1, 2026-09-16..24, 9,542 strikes, 99 % `overhead`, round the clock; and the 2026-11-01 afternoon at 1 that heard seven real ones logged nine `overhead` beside them. `git log` of the constant: 2 (392335916765, leaving the tuning space) -> 1 (45e55184c746) -> 0 (aa155a727975, nine days, by accident) -> 1 (39577cae0dae). | `defence.rs`, `boot.rs`, `effects.rs`, `query.rs` | **CHANGED in 0.22.0: the default is the chip's power-on 2** (`SPIKE_REJECTION_POWER_ON`, datasheet v1-04 page 19), and `tests/defence.rs` holds the default to the power-on value rather than to a number. **A stored value is left alone, and boot warns** -- the conservative choice of the two offered, because only `srej <n>` ever writes the key (`store_spike_rejection` has one caller), so a stored 1 is a person's 1; the device's own rule since 39577cae0dae (2026-08-29) is that it restores what a person set and never chooses for them; a one-time migration could not tell a deliberate 1 from a forgotten one, would need a marker in NVS for ever, and would put a second variable into every comparison across the flash -- the 1-against-2 comparison LM is about to make in the field above all. Leaving it costs one boot line (`as: ⚠ spike rejection 1 -- saved by `srej 1`, below the default of 2 since 0.22.0`) and one command to change it. The record's half: the boot rule is now `defence::spike_rejection_at_boot`, host-tested -- the more permissive of stored and a trusted record's, as before -- and the record that feeds it comes only from `settings::golden`, which since L-A1 does not read the record from before 0.22.0, so that record's `sr 1` cannot drag a board back to 1. A 0.22.0 record earned at 1 still lowers a later `srej 2` at the next restart, by the documented rule; boot now says so (`lowered from 2, lightning with a distance has been heard at it`, with `golden clear` as the way out), and so does `srej <n>` at the moment it is typed -- every console attach being a restart, that is the trap for a field comparison. `srej` alone reports the value in force and where it came from. Web page hint 2. Host checks in `tests/defence.rs`; mutation-checked (default back to 1, power-on 1, the record clamp removed, `max` for `min`, the record ignored, the record not filling an unset value, the warning taking 0 or 2) -- each caught. |
| L-A3 | **Every reader of "the srej" read NVS, and boot can program something else.** Boot has taken a trusted record's lower spike rejection over the stored one since 2629a2d19aeb, but the log's `srej` column, the golden record (`remember_working_point`), `describe` (`[wd N sr N fixed]`) and the rescue's comparison all read `spike_rejection_for_autorun(settings::spike_rejection())`. With NVS at 2 and the chip at a record's 1, a strike went into the record as `sr 2`, which the next boot then believed. Found reading the boot path for L-A2. | `session.rs`, `tuning.rs`, `boot.rs`, `effects.rs` | **FIXED in 0.22.0.** `session::SPIKE_REJECTION`, beside `CURRENT_NF` and `DISTURBERS_MASKED`: the value the chip holds, written only where the register is -- boot, after its write lands, and `srej <n>` -- starting at the power-on 2, which is what the register holds if boot's write fails. All four readers use it, so the log's `srej` column is now the value in force, which is what a provenance column is for. Not host-testable (an atomic beside an I2C write); the rule it reports is. |
| L-A4 | **Boot's comment claimed a record made at `srej 0` could not reopen the scar; nothing enforced it.** The record's value went through `spike_rejection_for_autorun`, which returns any stored value unchanged, 0 included -- so a record made during a person's `srej 0` experiment would lower a later `srej 1` back to 0, the device choosing zero. | `boot.rs`, `defence.rs` | **FIXED in 0.22.0**, in `spike_rejection_at_boot`: a record's contribution is clamped to `SPIKE_REJECTION_AUTO_MIN`; a person's own stored 0 is still honoured. Two host checks, mutation-checked. Never observed: no record on this board was made at 0. |
| L-A5 | **A trusted record's lower watchdog lasts only until the tuner's first move.** Boot calls `session::apply` (which writes `settings::watchdog()`) and *then* writes `min(stored, record.wdth)`; every later `apply` -- each walk step, dip, rescue, probe -- writes the stored value again. So the restore 2629a2d19aeb added for the measured `wd 2: 0 strikes / wd 0: 2 strikes` case is undone within minutes whenever stored and record differ. `describe` prints the stored value, so the console agrees with what `apply` writes and not with boot's banner. | `boot.rs`, `session.rs` | **RAISED, not changed** -- found beside L-A3, outside this pass's four items. Harmless on this board as far as the logs show: the record's `wd 0` was written with the stored watchdog at 0 (the record reads the stored value), and the default is 0. The fix is L-A3's shape: a `session::WATCHDOG` written where the register is, read by `apply`. |

## Review of 0.22.0, 2026-11-01 — before it was flashed

Nine findings against the 0.22.0 commits from two reviews, two of them the same
defect (merged here as L-V1). Each re-checked against the source, the board's
log (`after_0.21.2.txt`, 9,650 strikes), esp-idf-hal 0.46.2 and datasheet v1-04
before acting. All real; L-V6 is real as advice and not as code. Fixed in
0.22.1, most valuable first, one commit each. **Not verified on the board.**

| ID | What | Where | Verdict |
|---|---|---|---|
| L-V1 | **A real storm overhead no longer held the walk.** 0.22.0 took `overhead` out of the hold along with the record, and the repo's own records say that bin has held real lightning: 2026-08-12, 917 nearest-bin detections while thunder was audible every 10-30 s, held at 208-299 noise/min against 120 for 3.5 h -- "the strike-hold rule is what kept it listening" (specs §9). Through the real `verdict`: a window of 8 `overhead` at 210 noise/min had `heard_lightning` false and `quiet` false, so 0.22.0 climbed 210/60 = 3 notches a window to nf 7 within two or three; a dip hearing only `overhead` restored the defended point; no record was written, so no rescue could fire. | `tuning.rs`, `verdict.rs` | **FIXED in 0.22.1.** `verdict::Window::walk` decides hold/climb/relax, host-tested, and holds for **any** lightning interrupt (`heard_any_lightning`: confirmed or not); `tuning::step` only acts on its answer. A hold only refuses to step, so holding for the hammer costs a window the walk did not take, where not holding under a storm overhead is the climb into deafness. Everything that commits the device stays on evidence: the dip staying open, the dip gate (so September's stream cannot stop the dips), the rescue's silence and the record. The `tune:` lines say which happened. Checks in `tests/verdict.rs` (the overhead storm at 210/min holds; an hour of September holds every window and reads as lightning in none; a located strike, a quiet strike window and an unreadable one hold; no lightning walks by noise). Mutation-checked: hold on evidence only, `heard_any_lightning` dropping the unconfirmed, `heard_lightning` counting them, climb and relax swapped, the hold removed -- each caught. |
| L-V2 | **The evidence for the rule was misstated in code, docs and L-A1.** "Every `overhead` that could be checked was man-made" -- the repo's records say otherwise: 2026-08-12 above; 2026-08-13, 24 of 27 from a cell visibly overhead (specs §4.3: "equally consistent with the estimate simply being correct"); 2026-09-10, the two strikes of 2629a2d19aeb ("one audible strike every thirty seconds ... wd 0 (record) 2 strikes") are the log's 12:27:08 and 12:27:32 rows, 22 and 46 s after a boot (millis 22273, 45563), both `overhead`, and 12:19-13:59 holds 3 kilometre readings against 63 `overhead`. "The six `overhead` each came 6-9 s after another strike": the CSV millis give 9.20, 65.75, 9.15, 43.55, 6.13 and 64.77 s. "A lone impulse reads `overhead` by construction": no source -- datasheet p33 describes only the stored energies and timing correlated with a look-up table, purged when old -- and that morning's first detection, 39.5 s after boot cleared the statistics, read 14 km, as 2026-08-13's first three did. And the energy contrast is not independent: p33 derives the distance from those same energies. | `golden.rs`, `session.rs`, `README.md`, `doc/console.md`, `doc/scanning.md`, `tests/golden.rs` | **FIXED in 0.22.1**, with L-V1: `golden::is_evidence` now lists the days the reading was man-made and the days it was lightning, says the November outing settles neither way, and gives the rule's real ground -- the reading has been both and nothing in it says which. The rule itself stands: leaving `overhead` out of the record is the conservative half. The commit messages of 0.22.0 keep the old claim; they cannot be amended. |
| L-V3 | **A rescue that could never finish fired every window and froze the walk.** `fall_back_to` returned `Some` while `current.at_least_as_open_as(record)` was false, and the rescue moved only `nf` -- so with the chip's `srej` (or the stored `wdth`, L-A5) deafer than a trusted record's, nothing it did could make that true. Scenario on LM's board: a 0.22 record trusted at `nf 3 sr 1`, then `srej 2`, which 0.22.0's boot warning invites; twenty minutes after the last located strike `fall_back_to(nf3 sr2, record nf3 sr1, 25)` is `Some`, and every window prints "returning to nf 3", re-applies, restarts the window and `return`s before the hold, climb, relax, dip, stuck detection and persist -- frozen until a located strike or a restart, even if nf 3 drowns. Older than 0.22 (0.21 compared NVS's srej); 0.22.0 led into it. A second failure of the same comparison: with `nf` already more open than the record's and `srej` higher, it "returned" the point *up* to the record's rung. | `golden.rs`, `tuning.rs` | **FIXED in 0.22.1.** The rescue compares what it moves: `fall_back_to` returns the record's `nf` only when the current `nf` is deafer (same gain, trusted, patience run out), and `None` otherwise. `wdth` and `srej` are a person's -- `wdth <n>`/`srej <n>` write them, boot reconciles them with a trusted record -- and the rescue does not overrule a person mid-session. `Combo::at_least_as_open_as`, whose only caller this was, is gone with its six checks. Four new checks in `tests/golden.rs` (deafer srej at the record's nf: nothing; stored wd above the record's: nothing; a more open nf with a higher srej is never pulled up; deafer in nf and srej: nf comes back). Mutation-checked: all three knobs compared again, `<` for `<=`, the current rung returned, the gain check removed -- each caught. |
| L-V4 | **The rule's wiring was untested: reverting all three call sites passed 439/439.** The rule ran in `session` and `tuning`, which no host check compiles, and `tests/golden.rs`'s "840 overhead strikes write no record" ran a `fold` closure that re-implemented `remember_working_point`'s gate -- the copy-instead-of-module pattern `tests/README.md` forbids. Three mutations in a scratch copy -- `let learned = !merged.simulated;` in `commit_merged`, `if batch.strikes > 0` in `Tuning::observe`, `batch.lightning(Some(Distance::Km(5)))` in `read_and_handle` -- restored 0.21 in every learning path and passed every check. 0.22.0's "all mutations caught" covered the pure helpers only. | `session.rs`, `tuning.rs`, `tests/golden.rs` | **FIXED in 0.22.1.** The decisions moved into the pure modules and the call sites only hand values over: `golden::fold_flash(record, merged, at)` makes the whole record decision on the flash itself (`remember_working_point` supplies `at` and stores the answer); `verdict::Silence` is the rescue's clock (`observe` restarts it on a confirmed strike only, `minutes` reads it across the wrap); `Window::dip_stays_open` and `Window::dip_may_start` are the two dip questions, with `DIP_STRIKES_TO_BELIEVE` moved beside them; `Batch::lightning` takes the detail read as `As3935::strike` returned it, so `read_and_handle` passes it untouched. `tests/golden.rs` folds real merger flashes through `fold_flash`; `tests/verdict.rs` checks the dip and the silence (overhead, unreadable, noise and disturbers do not restart it; an hour of September's stream does not; it counts across the wrap). The finding's three mutations no longer exist as one-line changes; their equivalents in the moved code -- the gate as `!merged.simulated`, the silence restarted by every strike or by anything, a read taken as evidence whatever it said, a failed read as evidence, a dip kept open or stopped by `overhead`, the silence not wrap-safe -- each caught. What no host check can reach is now the hand-over itself: one line per call site, and `tests/README.md` says so. |
| L-V5 | **An `overhead`+`far` flash counted for the walk but not for the record, and the record then depended on the merge setting.** Per stroke, `Batch::lightning` took the `far` stroke as confirmed -- it held, kept a dip open, restarted the rescue's silence -- while the merger recorded the flash as `overhead` (`merger.rs`, `(0, true) => Overhead`) and `may_record` judged that merged distance, so the record did not learn; with `merge 0` the `far` stroke alone would have been recorded. Two learning paths disagreed about the same strokes, and the record depended on a display setting, against 0.22.0's own claim that a merge window must not throw evidence away. Never happened: none of the 9,650 strikes on the board reads `far`. | `golden.rs`, `merger.rs` | **FIXED in 0.22.1.** `Merged::evidence` counts the strokes `golden::is_evidence` accepts, one by one as the merger folds them in, and `may_record(merged)` asks whether any was. The merged distance is unchanged -- `overhead` wins with no kilometres, the nearest reading, for the screen -- so the one row `distance_km` alone cannot settle is `overhead`+`far`, which the console still marks and docs now say. Rejected: making `far` not evidence (it would make the column exact, but reverses 0.22.0's deliberate reading of `far` on no new evidence) and recording the pair as `far` (changes what the screen warns of to suit the record). `tests/golden.rs`: the single-stroke cases through real flashes, the pair counting, and every sequence of up to three strokes from {km, `overhead`, `far`} -- 39 -- giving one answer through the tuner's batch, the merged record and `merge 0`. Mutation-checked: judged on the merged distance again, the merger counting every stroke or none, `finish` dropping the count, `simulated` ignored, `far` made not evidence -- each caught. |
| L-V6 | **Once a 0.22 record is trusted at `sr 1`, `srej 2` lasts only until the next console attach** -- `spike_rejection_at_boot(Some(2), Some(1))` = 1, then strikes at 1 add to the sr-1 record, and the only way out, `golden clear`, also drops the nf and wd evidence and the trust. The console warned when `srej 2` was typed after such a record, but the 0.22.0 advice ("`srej 2` only if LM wants to rest at the new default now") did not say the order matters, and L-A2's ground for keeping a stored 1 -- "a stored value is a person's and is left alone" -- sits beside a rule that overrules a person's stored 2. | `boot.rs`, `defence.rs`, `doc/console.md` | **REAL AS ADVICE, not as code; the rule stands.** "The more permissive of stored and a trusted record's" is 2629a2d19aeb's, measured (`wd 2` stored: 0 strikes in 100 s; `wd 0` record: 2 in 90 s, a live storm), and it is the one place a measurement outranks a person; changing it for a field test would be a third variable in the comparison it serves. Fixed the advice where it is acted on: the boot warning that offers `srej 2` now adds which case applies -- "type it before two strikes with a distance arrive at 1, or the record they make brings 1 back at every restart", or, with a trusted record at 1 already, "`golden clear` first, or the next restart brings 1 back". The principle is restated in `defence::SPIKE_REJECTION_DEFAULT` and console.md: the device does not change a stored value on its own account; a trusted record, a measurement, is the one thing that can, at boot, toward the more permissive. A `srej 2` typed before the record is trusted stands, because the strikes after it are heard at 2 and the record follows them. |
| L-V7 | **The first-boot checklist expected the legacy record at 14 strikes; NVS most likely holds 2, both `overhead`.** 14 was read at 14:36 (`boot_0.21.1.txt`); 0.21's replace-on-change then rewrote it from 15:19:00 (nf 4 -> `nf 4, 1`), 15:33:13 and 15:34:31 (nf 3 -> `nf 3, 1`, `nf 3, 2`), all `overhead`, and nothing later in `after_0.21.2.txt`. So the nf 3 floor 0.22 drops rested on two detections tied to electrical events. | `findings.md` | **FIXED** -- the checklist above is rewritten for 0.22.1. Not a fault, and the offer to keep the old record as an untrusted floor is declined: losing that floor costs nothing real. |
| L-V8 | **The docs said the old step-3 wait was sometimes almost nothing; it was about 10 ms every boot.** esp-idf-hal 0.46.2: `FreeRtos::delay_ms(2)` = `vTaskDelay(TickType::new_millis(2))` = `vTaskDelay(1)` at `CONFIG_FREERTOS_HZ` 100 (checked in the generated `sdkconfig.h`). The old second delay -- bit up to bit down, the procedure's step 3 -- began ~0.3 ms of I2C after the first returned on a tick, so it ran to the next tick, ~9.7 ms; only the untimed gap between `CALIB_RCO` and bit-up varied, 0-10 ms. The new step 3 is 7.5-10 ms for the same reason. Of the three datasheet disagreements, the timing one never applied to the timed step. | `as3935.rs`, `doc/specs.md`, L-S9 | **FIXED (docs).** `LCO_STARTUP_MS`'s and `power_up`'s docs and the specs §3 0.22.0 note now say what the old delays did and that the bit and the order are what changed what the chip sees; the banner's revert advice names those two and not the wait. The 0.22.0 commit message keeps the old claim. `reset`'s 2 ms did start at an arbitrary point, so that fix stands as written. |

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
candidate**, the C5 panel candidate — not the lightning board, which was on
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
