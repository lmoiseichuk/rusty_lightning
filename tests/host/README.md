# Host checks

`cargo test` cannot run in this crate: it only builds for
`riscv32imc-esp-espidf`, and every dependency pulls in ESP-IDF. So the logic
worth testing is exercised here instead, on the host, with `rustc` directly.

```sh
cd tests/host
for t in *.rs; do rustc --edition 2021 -A dead_code -o /tmp/${t%.rs} "$t" && /tmp/${t%.rs}; done
```

| File | Covers |
|---|---|
| `defence.rs` | §4.2's noise-rejection ladder: rung boundaries, monotonicity, saturation |
| `history.rs` | §4.3's strike rings: the score formula against the spec's observed figures, bucketing, gap clearing, and that "overhead"/"out of range" are counted but not averaged |

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
