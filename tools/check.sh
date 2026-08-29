#!/usr/bin/env bash
#
# Run the host checks in tests/, then the release build.
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
# 2026-08-19 and `tests/defence.rs` went on asserting 1 for four days,
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
files=0

echo "== host checks (edition $EDITION) =="
for source in "$HERE"/tests/*.rs; do
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
    # Each test binary prints its own "N passed, M failed"; tee it through so
    # the per-file detail still scrolls past, and total it at the end. The
    # count is derived here rather than written down anywhere -- a number kept
    # by hand in a doc is a number that goes stale, which this repo has proved
    # about netlists and about this very figure.
    if ! "$BIN/$name" | tee "$BIN/$name.out"; then
        failed=1
    fi
    files=$(( files + 1 ))
done

total_passed=$(awk '/passed, /{p += $1} END {print p + 0}' "$BIN"/*.out 2>/dev/null)
total_failed=$(awk '/passed, /{f += $3} END {print f + 0}' "$BIN"/*.out 2>/dev/null)
echo
echo "== ${total_passed} checks across ${files} files, ${total_failed} failed =="

if (( failed )); then
    echo
    echo "host checks failed -- not building"
    exit 1
fi

# The module map is derived from the source, so it can go stale the moment a
# module is added. Checked rather than regenerated: a build step that quietly
# rewrites a tracked file makes `git status` lie about what the run did.
echo
echo "== module map =="
if ! python3 "$HERE/tools/modules.py" --check; then
    echo "run tools/modules.py and commit doc/modules.md" >&2
    exit 1
fi

echo
echo "== release build =="
cd "$HERE" || exit 1
cargo build --release || exit 1

echo
echo "all checks passed -- ./flash.sh to write it"
