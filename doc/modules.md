# The modules

<!-- The regions marked `generated` below are written by tools/modules.py from
     the source. Do not hand-edit them; the next run overwrites them. Prose
     outside the markers is preserved. -->

`src/` is one binary crate, not a workspace: every file is a module of it,
declared in `main.rs`. There is one device here, so there is no second binary to
share code with — which is why this project has modules where the moisture
project next door has crates.

<!-- generated: map -->
**34 modules**, of which **12 are free of ESP-IDF** and so can be
compiled and tested by bare `rustc` on this machine:

```
host-testable (no esp_idf_hal)      needs the device
----------------------------------   --------------------
civil                                as3935
csv                                  battery
defence                              boot
golden                               clock
history                              commands
merger                               console
policy                               credentials
press                                display
query                                effects
strike                               i2c_scan
uptime                               listen
verdict                              log
                                     portal
                                     power
                                     screen
                                     session
                                     settings
                                     storage
                                     system
                                     tuning
                                     ui
                                     webui
```

**Every one of them is covered** by the 11 files in `tests/` — a module that can be host-tested is.

`tools/check.sh` prints the check total. It is deliberately not repeated
here: a count kept in two places is a count that disagrees with itself,
which is the argument this project already makes about netlists.
<!-- end generated -->

## The line that matters

**`cargo test` cannot run in this crate.** It only builds for
`riscv32imc-esp-espidf`, and every dependency pulls in ESP-IDF. So the logic
worth testing is kept in modules that import nothing from ESP-IDF, and
`tests/` compiles those modules — the real ones, by `#[path]`, never
copies — with bare `rustc`. `tools/check.sh` runs them and refuses to build if
any fails.

That constraint is what shapes the module list. Whenever a decision turned out
to be worth testing, it was **moved out of the module that owns the hardware
and into one that owns only the arithmetic**, and the hardware module kept the
registers and re-exported the rest:

| the hardware half | the pure half it was split from |
|---|---|
| `power` — `esp_pm_configure`, the frequency ceilings | `policy` — which policy to run |
| `clock` — the RTC and NVS | `civil` — a timestamp to a calendar date |
| `webui` — rendering the page | `query` — reading what came back from it |
| `session` — the bus and the interrupt | `merger`, `verdict` — folding and judging |
| `as3935` — the registers | `strike`, `defence` — the values and the ladder |

Each of those splits was paid for by a bug. `policy` exists because a wrapping
subtraction pinned the device awake for 49.7 days at thirteen times the current;
`civil` because a wrong date is written into every row of the strike log and
nothing downstream can detect it.

## What each module owns

Summaries are the module's own first line — the source is the authority, and
this table is generated from it.

<!-- generated: roles -->
### Entry and the loop

| module | what it owns | tested | lines |
|---|---|---|---|
| `listen` | The event loop. | -- | 796 |

### The sensor and what it hears

| module | what it owns | tested | lines |
|---|---|---|---|
| `as3935` | AS3935 franklin lightning sensor — register driver (§3). | -- | 543 |
| `defence` | How hard the sensor is trying to reject noise (§4.2), as **one 3-bit number**. | pure, `tests/defence.rs` | 474 |
| `merger` | Folding return strokes into flashes (§4.3). | pure, `tests/merger.rs` | 213 |
| `session` | What a wake loop iteration accumulates, and what the screen is redrawn for. | -- | 1076 |
| `strike` | What a strike is, with no hardware attached. | pure, `tests/history.rs`, `tests/merger.rs` | 93 |
| `tuning` | §4.2's noise decision: one window, one verdict, one step. | -- | 1035 |
| `verdict` | What one tuning window saw, and the verdict it supports. | pure, `tests/verdict.rs` | 70 |

### Keeping it

| module | what it owns | tested | lines |
|---|---|---|---|
| `civil` | Unix timestamp to calendar date, with no hardware and no library. | pure, `tests/civil.rs` | 88 |
| `clock` | Wall-clock time without a network (§5). | -- | 194 |
| `csv` | Reading a log row back, whatever shape the file is. | pure, `tests/csv.rs` | 144 |
| `history` | Strike history, bucketed by time (§4.3, §6). | pure, `tests/history.rs` | 509 |
| `log` | The strike log: a CSV file on LittleFS, surviving power cuts (§5). | -- | 534 |
| `settings` | What survives a power cut, and nothing else. | -- | 330 |
| `storage` | Non-volatile storage — thin wrappers over ESP-IDF's `nvs_*` API. | -- | 188 |

### The glass

| module | what it owns | tested | lines |
|---|---|---|---|
| `display` | The 7.5" panel — hardware only (§6). | -- | 335 |
| `screen` | When the panel is redrawn, and what goes on it. | -- | 270 |
| `ui` | Screen layout (§6). | -- | 1452 |

### Talking to a person

| module | what it owns | tested | lines |
|---|---|---|---|
| `commands` | What the console commands actually do (§5). | -- | 482 |
| `console` | Commands in over USB, and the awake signal that comes with them. | -- | 531 |
| `credentials` | The access point's name and password. | -- | 145 |
| `effects` | Applying the console's hardware effects. | -- | 407 |
| `portal` | The access point and the web server it carries. | -- | 615 |
| `press` | Telling a fingertip from a USB host, and one gesture from another. | pure, `tests/press.rs` | 162 |
| `query` | Reading a query string, and escaping what goes back out. | pure, `tests/query.rs` | 202 |
| `webui` | The page the access point serves. | -- | 245 |

### The board underneath

| module | what it owns | tested | lines |
|---|---|---|---|
| `battery` | MAX17048 LiPo fuel gauge (§2.1). | -- | 772 |
| `boot` | Bring-up: everything that happens once, before the loop. | -- | 274 |
| `i2c_scan` | Who is on the bus, and is it the right who. | -- | 100 |
| `policy` | Which clock-and-sleep policy to run, as pure arithmetic. | pure, `tests/policy.rs` | 69 |
| `power` | Clock and sleep policy (§7). | -- | 126 |
| `system` | What the device can say about itself: clock, die temperature, free heap. | -- | 165 |
| `uptime` | Comparing times on a counter that wraps. | pure, `tests/merger.rs`, `tests/policy.rs`, `tests/uptime.rs` | 70 |

**Unclassified** (add them to `ROLES` in `tools/modules.py`): `golden`

<!-- end generated -->

## The dependency structure

<!-- generated: deps -->
Read down: a module is listed with what it names. `main` and `listen`
name almost everything and are omitted, since 'the entry point uses the
program' is not information.

| module | names |
|---|---|
| `as3935` | `strike` |
| `battery` | `settings` `uptime` |
| `boot` | `as3935` `defence` `golden` `session` `settings` |
| `clock` | `civil` `storage` |
| `commands` | `as3935` `battery` `clock` `console` `credentials` `golden` `history` `log` `power` `session` `settings` `strike` `system` `ui` |
| `console` | `defence` `query` `session` `strike` |
| `credentials` | `storage` |
| `effects` | `as3935` `battery` `commands` `console` `defence` `history` `log` `power` `screen` `session` `settings` `system` `tuning` |
| `history` | `strike` |
| `log` | `as3935` `clock` `csv` `history` `settings` |
| `merger` | `strike` `uptime` |
| `policy` | `uptime` |
| `portal` | `clock` `credentials` `history` `log` `session` `strike` `system` `tuning` `uptime` `webui` |
| `power` | `policy` `storage` |
| `screen` | `as3935` `battery` `clock` `defence` `display` `history` `power` `session` `system` `ui` `uptime` |
| `session` | `as3935` `clock` `defence` `golden` `history` `log` `merger` `settings` `uptime` |
| `settings` | `as3935` `battery` `defence` `golden` `storage` |
| `tuning` | `as3935` `defence` `golden` `listen` `session` `settings` `uptime` `verdict` |
| `ui` | `as3935` `battery` `clock` `display` `history` `session` `system` |
| `webui` | `portal` `query` |

**Depends on nothing in this crate** — the bottom of the stack, and
where a change is cheapest: `civil`, `csv`, `defence`, `display`, `golden`, `i2c_scan`, `press`, `query`, `storage`, `strike`, `system`, `uptime`, `verdict`.
<!-- end generated -->

## How a strike travels

1. The AS3935 pulls GPIO21 high. `listen`'s notification wakes the loop.
2. `session::collect` reads the interrupt register over I2C and classifies it:
   lightning, disturber, or noise. `as3935` owns those registers; `strike` owns
   the values they decode to.
3. A lightning event goes to `merger`, which folds return strokes arriving
   inside the merge window into one flash. **Strokes are not strikes** — the
   tuner counts strokes as evidence, a person reads flashes.
4. The flash is scored by `history::score_milli`, pushed into the three rings
   (`history`) and appended to the CSV (`log`).
5. Noise and disturbers go to `tuning`, which every window decides whether the
   band is quiet enough to relax the noise floor or loud enough to raise it.
   `defence` holds the ladder; `verdict` decides what "quiet" means.
6. `screen` decides whether anything changed enough to be worth 3.8 s of panel
   time, and `ui` draws it.

At boot the same path runs backwards: `log::for_each` reads the CSV through
`csv`, and replays every record into the rings and the recent-strike table, so a
reboot does not empty the charts.

## Two interfaces, one command table

The console and the web UI are the same thing entered two ways. `console`
parses a line into a `Command`; `commands::run` turns it into an `Effects`
describing what should happen; `effects::handle` applies the parts that need
hardware, and hands the rest back to the loop, which owns the things a command
must not reach for itself — the `Portal`, the sensor, the log.

The web UI **composes a console command line** and queues it. It has no command
table of its own, no range checks of its own, and cannot express anything
`help` does not list. That is deliberate: two interfaces with two parsers drift,
and the second one is always the one that gets the range check wrong.

## Where to put a new thing

* **A decision, a rule, a piece of arithmetic** — a new pure module, plus a file
  in `tests/`. If it needs the device to test it, it is in the wrong place.
* **A register, a pin, a peripheral** — the module that already owns that
  hardware. It should expose values, not registers.
* **A console command** — a variant in `console::Command`, an arm in
  `commands::run`, and an arm in `effects::handle` if it needs hardware. Add it
  to `query::command_from_query`'s allow-list to reach it from the web UI, and
  to `doc/console.md`.
* **Anything drawn** — `ui`, and measure it first. The panel is 800x480 and the
  fonts are fixed-width, so a label's real width is arithmetic, not a guess.

## Regenerating this file

    tools/modules.py            # rewrite the generated regions
    tools/modules.py --check    # exit non-zero if it is out of date
