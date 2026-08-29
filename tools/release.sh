#!/usr/bin/env bash
#
# Rebuild every image in release/ from the current commit.
#
#     ./tools/release.sh                 # every variant
#     ./tools/release.sh default         # just one
#     ./tools/release.sh no-light-sleep
#
# ## Why this exists
#
# `./flash.sh` builds and writes whatever is in `target/` right now. That is the
# right tool while developing and the wrong one for anything you want to keep:
# nothing records which feature flags produced a binary, so a build cannot be
# reproduced, and an image on disk cannot be told apart from a newer one.
#
# The moisture project learned that the expensive way -- a stale image shipped
# and would have erased a field board's log. The same shape of accident is
# available here: a recovery build and an ordinary build differ only in whether
# light sleep is compiled in, and they are indistinguishable once written.
#
# So the flags live in one table below, and every image carries the commit it was
# built from together with the list of sources it was built *from*.
# `release/flash.sh` refuses an image whose sources have changed since.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$HERE"

# --- what this crate is built from ------------------------------------------
#
# Written into every image's VERSION so `release/flash.sh` can judge staleness
# without knowing anything about the layout here. One crate, so this is short --
# but it is still a list rather than "everything", because committing the
# release/ tree must not make every image stale.
SOURCES="src tests Cargo.toml Cargo.lock build.rs partitions.csv sdkconfig.defaults rust-toolchain.toml littlefs_bindings.h assets"

TARGET_TRIPLE="riscv32imc-esp-espidf"
BINARY="lightning"

# --- the variants -----------------------------------------------------------
#
# variant | cargo flags | what it is for
VARIANTS=(
  "default|(default)|THE INSTRUMENT: light sleep on, so the battery lasts. The USB console is only reachable in the waking windows -- send \`freq 160\` to pin the clock and keep the port up while debugging."
  "no-light-sleep|--features no-light-sleep|RECOVERY BUILD: light sleep compiled out entirely, so the USB port never goes away. Costs the battery saving; buys a board that can always be talked to. Flash this before a long debugging session, and the ordinary build afterwards."
)

want=("$@")
built=0

for row in "${VARIANTS[@]}"; do
    IFS='|' read -r variant flags purpose <<<"$row"

    if [[ ${#want[@]} -gt 0 ]]; then
        match=0
        for w in "${want[@]}"; do [[ "$w" == "$variant" ]] && match=1; done
        [[ $match -eq 1 ]] || continue
    fi

    # **Refuse to stamp a dirty tree.** The commit in a VERSION file is a promise
    # that the sources are recoverable; building from uncommitted work breaks it
    # silently, and the image looks exactly as trustworthy as any other.
    # **`git diff` cannot see a file git has never heard of.**
    #
    # This checked `git diff --quiet HEAD` alone, which ignores untracked files
    # entirely -- so a tree whose only change was a NEW source file passed the
    # guard and got stamped with a commit that does not contain it. The image
    # would then be unreproducible, and `flash.sh` would call it fresh, because
    # its staleness check compares the same set.
    #
    # Not hypothetical: `src/csv.rs` and `tests/host/csv.rs` were both untracked
    # when this was found, and both are compiled into the binary.
    #
    # `--porcelain` reports tracked and untracked alike, so the guard now sees
    # what the compiler sees.
    dirty="$(git status --porcelain -- $SOURCES 2>/dev/null)"
    if [[ -n "$dirty" ]]; then
        echo "working tree is dirty -- commit first." >&2
        echo "Every image is stamped with a commit, and flash.sh trusts that stamp." >&2
        echo "$dirty" >&2
        exit 1
    fi

    echo "== $variant =="
    if [[ "$flags" == "(default)" ]]; then
        cargo build --release
    else
        # shellcheck disable=SC2086
        cargo build --release $flags
    fi

    out="$HERE/release/$variant"
    mkdir -p "$out"
    from="$HERE/target/$TARGET_TRIPLE/release"
    for f in bootloader.bin partition-table.bin "$BINARY"; do
        [[ -f "$from/$f" ]] || { echo "missing build artifact: $from/$f" >&2; exit 1; }
        cp "$from/$f" "$out/$f"
    done

    version="$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)"
    cat > "$out/VERSION" <<EOF
build   : $variant
version : $version
commit  : $(git rev-parse --short=12 HEAD)
built   : $(date -u +%Y-%m-%dT%H:%M:%SZ)
flags   : $flags
image   : $BINARY
target  : $TARGET_TRIPLE
sources : $SOURCES
purpose : $purpose
EOF
    echo "built from $(git rev-parse --short=12 HEAD)"
    built=$((built + 1))
done

if [[ $built -eq 0 ]]; then
    echo "no variant matched. Known: default no-light-sleep" >&2
    exit 1
fi

echo
echo "Commit the release/ tree so the images and the stamp travel together."
