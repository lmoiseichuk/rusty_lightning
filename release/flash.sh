#!/usr/bin/env bash
#
# Flash a built image from release/, without building anything.
#
#     ./release/flash.sh default
#     ./release/flash.sh no-light-sleep /dev/ttyACM2
#     ./release/flash.sh default --force        # flash a stale image on purpose
#
# ## Why this is not ../flash.sh
#
# `../flash.sh` builds and writes whatever is in `target/` — the right thing
# while developing. This one writes an image that was built, stamped and
# committed, and it refuses to write one whose sources have changed since.
#
# ⚠ **All three artifacts, every time.** `espflash flash <app>` writes only the
# application and leaves whatever bootloader and partition table are already on
# the board. On first bring-up here that was a vendor image with no `storage`
# partition, and the app loaded and immediately reset while printing nothing --
# which reads exactly like a firmware bug. See ../flash.sh for the full story.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/.." && pwd)"

TARGET="${1:-}"
[[ -n "$TARGET" ]] || { echo "usage: release/flash.sh <variant> [port] [--force]" >&2; ls "$HERE" | grep -v flash.sh >&2; exit 1; }
shift

PORT=""
FORCE=0
for arg in "$@"; do
    case "$arg" in
        --force) FORCE=1 ;;
        *) PORT="$arg" ;;
    esac
done

DIR="$HERE/$TARGET"
[[ -d "$DIR" ]] || { echo "no such variant: $TARGET" >&2; exit 1; }
[[ -f "$DIR/VERSION" ]] || { echo "$TARGET has no VERSION stamp -- rebuild it" >&2; exit 1; }

field() { grep -m1 "^$1 " "$DIR/VERSION" | cut -d: -f2- | sed 's/^ *//'; }

STAMPED="$(field commit)"
SOURCES="$(field sources)"
IMAGE="$(field image)"
PURPOSE="$(field purpose)"

# --- find the board ---------------------------------------------------------
#
# **By its stable path, never by ttyACM<n>.** The numbering shuffles between
# plug-ins and has pointed at a different board before. This one is
# B0:A6:04:06:E6:D4; the by-id path carries that and does not move.
if [[ -z "$PORT" ]]; then
    found="$(ls /dev/serial/by-id/ 2>/dev/null | grep -m1 'B0:A6:04:06:E6:D4' || true)"
    if [[ -n "$found" ]]; then
        PORT="/dev/serial/by-id/$found"
    else
        cat >&2 <<'EOF'
No port given and the board is not on /dev/serial/by-id.

If it is plugged in and still missing, it is probably asleep: light sleep powers
down the USB Serial/JTAG PHY, so the port comes and goes. Either catch it in a
waking window, or flash the no-light-sleep variant once and it will stay.
EOF
        exit 1
    fi
fi

echo "port    : $PORT"
echo "variant : $TARGET"
echo "purpose : $PURPOSE"

# --- staleness guard --------------------------------------------------------
#
# **Compare firmware inputs, not commits.** "Stamp == HEAD" is the obvious check
# and it is wrong: committing the release/ tree itself moves HEAD, so every image
# would be stale the moment it was checked in. What matters is whether anything
# the binary is BUILT FROM has changed since.
if ! git -C "$ROOT" rev-parse --git-dir >/dev/null 2>&1; then
    echo "note: no git here, so staleness cannot be checked. Image says $STAMPED."
else
    HEAD_SHA="$(git -C "$ROOT" rev-parse --short=12 HEAD)"
    # shellcheck disable=SC2086
    if ! git -C "$ROOT" diff --quiet "$STAMPED" HEAD -- $SOURCES 2>/dev/null; then
        # shellcheck disable=SC2086
        CHANGED="$(git -C "$ROOT" diff --name-only "$STAMPED" HEAD -- $SOURCES | head -8)"
        cat >&2 <<EOF
⚠ STALE IMAGE -- refusing to flash.

    image was built from : $STAMPED
    this checkout is at  : $HEAD_SHA

Firmware sources changed since that image was built:
$(sed 's/^/    /' <<<"$CHANGED")

Rebuild:
    ./tools/release.sh $TARGET

Or, if flashing this exact older image is what you mean -- bisecting a
regression, or reproducing what a board was running during a storm -- say so:
    ./release/flash.sh $TARGET $PORT --force
EOF
        [[ $FORCE -eq 1 ]] || exit 1
        echo "--force given; flashing the stale image anyway." >&2
    fi
fi

for f in bootloader.bin partition-table.bin "$IMAGE"; do
    [[ -f "$DIR/$f" ]] || { echo "missing artifact: $DIR/$f" >&2; exit 1; }
done

# `--before usb-reset` because this board has no external reset line the flasher
# can pull; the USB peripheral's own reset is what puts it into the downloader.
espflash flash --port "$PORT" --non-interactive --before usb-reset \
    --bootloader "$DIR/bootloader.bin" \
    --partition-table "$DIR/partition-table.bin" \
    "$DIR/$IMAGE"

cat <<EOF

⚠ Set the clock. Flashing resets the chip, which stops the RTC -- \`restore\`
then falls back to whatever NVS last saved, up to fifteen minutes stale. A
strike stamped in between would carry a timestamp minutes early.

    echo "date \$(date +%s)" > $PORT

Watch it:
    espflash monitor --port $PORT
EOF
