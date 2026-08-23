#!/usr/bin/env bash
#
# Run the host checks in tests/host, then the release build.
#
#     ./tools/check.sh
#
# ## Why this exists
#
# `cargo test` cannot run in this crate: it only builds for
# `riscv32imc-esp-espidf` and every dependency pulls in ESP-IDF. So the logic
# worth testing is compiled directly with `rustc` instead, against the real
# modules under `src/` rather than copies of them.
#
# That arrangement only works if somebody runs it. Nobody did:
# `SPIKE_REJECTION_DEFAULT` changed from 1 to 0 in `src/defence.rs` on
# 2026-08-19 and `tests/host/defence.rs` went on asserting 1 for four days,
# through two commits and a flash. The checks were not wrong — they were never
# run, which is the same thing with better manners.
#
# One command, and it is the one to run before `flash.sh`.
set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN="$(mktemp -d)"
trap 'rm -rf "$BIN"' EXIT

# **Read from Cargo.toml, never written here.** These checks compile the real
# modules out of `src/`, so they have to use the edition the crate ships under
# -- a second copy of that number is a second thing to forget, which is how the
# checks came to be stale in the first place.
EDITION="$(sed -n 's/^edition *= *"\(.*\)"/\1/p' "$HERE/Cargo.toml" | head -1)"
if [[ -z "$EDITION" ]]; then
    echo "could not read edition from Cargo.toml" >&2
    exit 1
fi

failed=0

echo "== host checks (edition $EDITION) =="
for source in "$HERE"/tests/host/*.rs; do
    name="$(basename "$source" .rs)"
    # A fresh binary every time, in a temp directory. Building into /tmp under a
    # fixed name meant a compile failure left the *previous* binary in place, so
    # the run that followed reported the old file's results as if they were
    # this one's -- a green tick for code that did not compile.
    if ! rustc --edition "$EDITION" -A dead_code -o "$BIN/$name" "$source"; then
        echo "  FAIL $name did not compile"
        failed=1
        continue
    fi
    if ! "$BIN/$name"; then
        failed=1
    fi
done

if (( failed )); then
    echo
    echo "host checks failed -- not building"
    exit 1
fi

echo
echo "== release build =="
cd "$HERE" || exit 1
cargo build --release || exit 1

echo
echo "all checks passed -- ./flash.sh to write it"
