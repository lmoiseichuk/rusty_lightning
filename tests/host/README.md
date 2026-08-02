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

**These are copies, not the real modules** — the honest limitation, carried over
from the moisture project. A change to `src/` will not fail them automatically;
re-sync when the logic they mirror changes.
