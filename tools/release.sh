#!/usr/bin/env bash

# lightning project, Copyright (c) 2026 Leonid Moiseichuk under MIT License

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
#
# **`.cargo/config.toml` and `components_esp32c3.lock` are build inputs.** They
# were missing, and both change the binary: the first pins ESP_IDF_VERSION, the
# `espidf_time64` rustflag, the MCU and the globs that deliver partitions.csv
# into the generated IDF project; the second pins joltwallet/littlefs by hash
# under a floating `^1.14` in Cargo.toml. Because the dirty guard and
# `release/flash.sh`'s staleness check both read this list, editing either file
# passed the guard and produced an image stamped with a commit that did not
# contain it -- the same class as the untracked-files hole.
SOURCES="src tests Cargo.toml Cargo.lock build.rs partitions.csv sdkconfig.defaults rust-toolchain.toml littlefs_bindings.h assets .cargo/config.toml components_esp32c3.lock"

# `release/flash.sh` sources this, so a release tree carries its own identity
# check rather than depending on the checkout it was built in. `send.py` goes
# with it because flash.sh's last instruction -- set the clock -- names it, and
# it finds the board through the `board.sh` beside it.
#
# lightning's `devices.list` is its own, and never copied; its format is
# garden's `devices.list.example`, which goes in with garden's tools below.
SHARED_INTO_RELEASE="tools/board.sh tools/send.py"

# **garden's half of the check goes in too, at `release/garden/`.**
# `tools/board.sh` is lightning's wrapper around garden's `board.sh`, and
# `send.py` asks garden's `board.py` before it opens a port -- so a tree copied
# away from this checkout needs both. Laid out as the submodule is, each file at
# the same path under `release/` as under the checkout's root, so that each
# finds the list as it does from the submodule: two levels above `tools/`,
# which for the copy is the release tree's root. The example goes with them
# because garden's message for a missing list says to copy it from there.
#
# In lightning's checkout the wrapper uses the submodule, so the copy is used
# only by a tree that is not one. garden's `license` goes with its files, as
# lightning's `LICENSE` goes with lightning's (below).
GARDEN_INTO_RELEASE="garden/tools/board.sh garden/tools/board.py garden/devices.list.example garden/license"

# **The licence notices go in too, so a copy of release/ owes nothing to the
# checkout.** The images are lightning's code and other people's: the crates
# they link, the Rust standard library, and ESP-IDF underneath. Those licences
# ask for their texts to go with every copy of an image. Four things carry them:
#
#   release/LICENSE               lightning's own, copied from the root
#   release/<variant>/licences/   made by garden's licences.py from the build
#                                 just finished: every linked crate's licence
#                                 files, the standard library's, ESP-IDF's
#                                 with its linked components', and the notices
#                                 in the headers of the sources it links --
#                                 see below
#   release/notice.md             written by hand, kept in git: what the tool
#                                 cannot collect (the fonts, DFRobot's driver,
#                                 the crates it flags), LittleFS's texts as
#                                 well, and the logo
#   release/garden/license        garden's, for its files under release/garden/
NOTICES_INTO_RELEASE="LICENSE"
LICENCES_TOOL="garden/tools/licences.py"

# **The crates licences.py flags that release/notice.md answers by hand.** The
# tool exits 1 when it flags anything: a crate that ships no licence text, or
# one whose licence is given only as a file. Each one here has its text, or its
# reading, in release/notice.md. A flag not on this list stops the release --
# including one of these at a new version, whose text has to be checked again.
ANSWERED_FLAGS=(
  "u8g2-fonts 0.8.0"   # its licence is a file: MIT OR Apache-2.0, fonts under U8g2's terms
  "qrcodegen 1.8.0"    # ships no licence file: the notice is in its src/lib.rs
  "void 1.0.2"         # ships no licence file: its repository's LICENSE-MIT
)

TARGET_TRIPLE="riscv32imc-esp-espidf"
BINARY="lightning"

# --- the variants -----------------------------------------------------------
#
# variant | cargo flags | what it is for
VARIANTS=(
  "default|(default)|THE INSTRUMENT: light sleep on, so the battery lasts. The USB console is only reachable in the waking windows -- send \`freq 160\` to keep the port up until the next restart, or \`awake on\` to keep it up across restarts (saved; ~2.5 days on the cell, against up to a week asleep)."
  "no-light-sleep|--features no-light-sleep|RECOVERY BUILD: light sleep compiled out entirely, so the USB port never goes away. Costs the battery saving; buys a board that can always be talked to. Flash this before a long debugging session, and the ordinary build afterwards -- or keep the ordinary build and send \`awake on\`, which holds the port up across restarts without a reflash."
)

# Checked before anything is built, so a missing submodule costs no build.
for shared in $GARDEN_INTO_RELEASE; do
    if [[ ! -f "$HERE/$shared" ]]; then
        echo "missing $shared -- fetch the submodule: git submodule update --init" >&2
        exit 1
    fi
done
# **No notice, no release.** An image copied away without its licence texts is
# the one thing this step exists to prevent, so a garden without the tool
# refuses, rather than releasing the image bare.
if [[ ! -f "$HERE/$LICENCES_TOOL" ]]; then
    echo "missing $LICENCES_TOOL -- move the garden submodule to a commit that has it" >&2
    exit 1
fi

# **Every variant is built before anything goes into release/.** licences.py
# names the commit it read, and says so when the checkout has changes git can
# see -- and an image already copied into release/ is such a change. Copying
# each variant in as it was built would make every later variant's notice say
# its checkout had changes, when the only change was the earlier image. So each
# variant is built, read and stamped into target/release-staging/ (target/ is
# gitignored), and release/ is written only once the last one has succeeded --
# which also means a failed build leaves release/ as it was.
STAGING="$HERE/target/release-staging"
rm -rf "$STAGING"

want=("$@")
built_variants=()

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
    # Not hypothetical: `src/csv.rs` and `tests/csv.rs` were both untracked
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
    # The table writes "(default)" for no flags at all; cargo and licences.py
    # are both given the flags themselves, which for that row is nothing.
    if [[ "$flags" == "(default)" ]]; then
        cargo_flags=""
    else
        cargo_flags="$flags"
    fi
    # shellcheck disable=SC2086
    cargo build --release $cargo_flags

    staged="$STAGING/$variant"
    mkdir -p "$staged"
    from="$HERE/target/$TARGET_TRIPLE/release"
    for f in bootloader.bin partition-table.bin "$BINARY"; do
        [[ -f "$from/$f" ]] || { echo "missing build artifact: $from/$f" >&2; exit 1; }
        cp "$from/$f" "$staged/$f"
    done

    # **The image's licence notice, from the build just finished.** target/
    # holds only the last variant built, so this is the one moment the build
    # beside the image is this image's. licences.py reads the image itself:
    # its ELF symbols decide which ESP-IDF components it links. The notice goes
    # to licences/licences.md; its standard output, the same text, is not needed.
    #
    # Exit status 0 is nothing flagged, 1 something flagged, 2 no notice made.
    status=0
    # shellcheck disable=SC2086
    python3 "$HERE/$LICENCES_TOOL" "$HERE" $cargo_flags --collect "$staged/licences" >/dev/null || status=$?
    if (( status >= 2 )); then
        echo "licences.py could not make $variant's notice -- see its message above" >&2
        exit 1
    fi
    if (( status == 1 )); then
        # The flags are the "- <crate> <version>: <why>" lines under the
        # notice's "## Flagged" heading. Each must be one release/notice.md
        # answers; finding none at all means the format changed, which is
        # refused too rather than read as "nothing flagged".
        found=0
        unanswered=""
        while IFS= read -r line; do
            found=$((found + 1))
            what="${line#- }"
            what="${what%%:*}"
            answered=0
            for known in "${ANSWERED_FLAGS[@]}"; do
                if [[ "$what" == "$known" ]]; then
                    answered=1
                fi
            done
            if (( answered == 0 )); then
                unanswered+="    $line"$'\n'
            fi
        done < <(sed -n '/^## Flagged$/,$p' "$staged/licences/licences.md" | grep '^- ')
        if (( found == 0 )); then
            echo "licences.py flagged something in $variant's notice, and no flag could be read from it" >&2
            exit 1
        fi
        if [[ -n "$unanswered" ]]; then
            echo "licences.py flagged what release/notice.md does not answer, in $variant's notice:" >&2
            printf '%s' "$unanswered" >&2
            echo "Add its text, or its reading, to release/notice.md, then its name and version to" >&2
            echo "ANSWERED_FLAGS in this script." >&2
            exit 1
        fi
    fi

    version="$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)"
    cat > "$staged/VERSION" <<EOF
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
    built_variants+=("$variant")
done

if [[ ${#built_variants[@]} -eq 0 ]]; then
    echo "no variant matched. Known: default no-light-sleep" >&2
    exit 1
fi

# --- into release/ ----------------------------------------------------------
#
# Each built variant replaces its folder's files. `licences/` is removed first,
# so a crate the lock file has dropped leaves no text behind.
for variant in "${built_variants[@]}"; do
    out="$HERE/release/$variant"
    mkdir -p "$out"
    rm -rf "$out/licences"
    cp -R "$STAGING/$variant/." "$out/"
done

# The identity check travels with the images, so `release/flash.sh` refuses
# the wrong board even in a tree copied to another machine -- and the licences
# travel with them too.
for shared in $SHARED_INTO_RELEASE $NOTICES_INTO_RELEASE; do
    cp "$HERE/$shared" "$HERE/release/$(basename "$shared")"
done
for shared in $GARDEN_INTO_RELEASE; do
    mkdir -p "$HERE/release/$(dirname "$shared")"
    cp "$HERE/$shared" "$HERE/release/$shared"
done
rm -rf "$STAGING"

echo
echo "Commit the release/ tree so the images, their stamps and their licence texts travel together."
echo "release/notice.md is written by hand: it carries what licences.py cannot collect -- DFRobot's"
echo "driver, the fonts, the flagged crates -- and LittleFS's texts too, and says which logo each image draws. If"
echo "any of those changed, or an image's version or commit it names did, update it."
