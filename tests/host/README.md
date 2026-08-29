# Host checks

`cargo test` cannot run in this crate: it only builds for
`riscv32imc-esp-espidf`, and every dependency pulls in ESP-IDF. So the logic
worth testing is exercised here instead, on the host, with `rustc` directly.

`tools/check.sh` runs them all and refuses to build if any fails. It reads the
edition out of `Cargo.toml` rather than naming one, because these compile
modules straight out of `src/` and so must use the edition the crate ships
under; a hard-coded edition here is a thing that goes stale silently.

```sh
tools/check.sh          # all of them, then the release build
```

One at a time, while working on it. **The edition in these lines is written out
and so can go stale** — `check.sh` is the authority, and it reads the edition
from `Cargo.toml` rather than naming one:

```sh
cd tests/host && rustc --edition 2024 -A dead_code -o /tmp/civil civil.rs && /tmp/civil
```

| File | Covers |
|---|---|
| `civil.rs` | the calendar: leap years, the century rules, month lengths, and that the time of day never leaks into the date |
| `csv.rs` | reading the log back: the header decides the columns, so a new column cannot silently shift the old ones |
| `defence.rs` | §4.2's noise-rejection ladder: rung boundaries, monotonicity, saturation |
| `history.rs` | §4.3's strike rings: the score formula against the spec's observed figures, bucketing, gap clearing, and that "overhead"/"out of range" are counted but not averaged |
| `merger.rs` | folding the strokes of one flash into a single strike, and what a merged flash reports |
| `press.rs` | the button gesture boundaries: too short, gain flip, portal, stuck |
| `uptime.rs` | interval arithmetic across the 49.7-day millisecond wrap |
| `verdict.rs` | the noise verdict: what counts as quiet, and that only noise feeds it |

## These compile the real modules, not copies of them

Each file `#[path]`-includes the actual source it tests:

```rust
#[path = "../../src/defence.rs"]
mod defence;
```

So a change to `src/` that breaks a check **fails here**, rather than leaving
two copies to drift apart. (An earlier version did copy the logic, carried over
from the moisture project, with a note asking for manual re-sync — a step that
was going to be skipped exactly once.)

What makes it possible is that the modules under test import nothing from
ESP-IDF. That is a constraint on them, not an accident:

* `defence` is pure arithmetic; the register writes it implies live in
  `session::apply_defence`.
* `history` names its one dependency as `crate::strike`, and `strike` holds the
  `Distance`/`Strike` types with no driver attached — which is why they are not
  in `as3935`, though `as3935` re-exports them so callers are unaffected.

Declaring `mod strike;` at the test binary's root is what makes `history`'s
`crate::strike` path resolve, so keep those module names as they are.

**If you add an ESP-IDF import to either module, these stop compiling.** That is
the intended failure: move the hardware half out rather than reaching for a copy.
