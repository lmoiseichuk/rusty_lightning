#!/usr/bin/env bash

# lightning project, Copyright (c) 2026 Leonid Moiseichuk under MIT License

# Flash a built image from release/, without building anything.
#
#     ./release/flash.sh default
#     ./release/flash.sh no-light-sleep <port>    # checked by MAC
#     ./release/flash.sh default --force          # flash a stale image on purpose
#
# ## Why this is not ../flash.sh
#
# `../flash.sh` builds and writes whatever is in `target/` — the right thing
# while developing. This one writes an image that `../tools/release.sh` built
# and stamped, and it refuses to write one whose sources have changed since.
#
# ⚠ **All three artifacts, every time.** `espflash flash <app>` writes only the
# application and leaves whatever bootloader and partition table are already on
# the board. On first bring-up here that was a vendor image with no `storage`
# partition, and the app loaded and immediately reset while printing nothing --
# which reads exactly like a firmware bug. See ../flash.sh for the full story.
#
# **The images are local.** Each variant's folder here is gitignored, so a fresh
# clone has none until `../tools/release.sh` builds them on that machine.
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
[[ -d "$DIR" ]] || { echo "no such variant: $TARGET -- the images are built locally: ./tools/release.sh $TARGET" >&2; exit 1; }
[[ -f "$DIR/VERSION" ]] || { echo "$TARGET has no VERSION stamp -- rebuild it" >&2; exit 1; }

field() { grep -m1 "^$1 " "$DIR/VERSION" | cut -d: -f2- | sed 's/^ *//'; }

STAMPED="$(field commit)"
SOURCES="$(field sources)"
IMAGE="$(field image)"
PURPOSE="$(field purpose)"

# --- find the board ---------------------------------------------------------
#
# **By its stable path, never by ttyACM<n>.** The numbering shuffles between
# plug-ins and has pointed at a different board before. Which board this is
# lives in lightning's own `devices.list`, not in this script: the script is the
# reusable part. A board not on the list, or flagged `forbidden` there, is
# refused by MAC.
#
# `board.sh` -- lightning's wrapper -- is copied in beside this file by
# `tools/release.sh`, and so are garden's `tools/board.sh`, `tools/board.py`,
# `tools/portlock.py` and `devices.list.example`, laid out as the submodule is. In lightning's
# checkout -- git says the directory above is a work tree whose root tracks
# `release/board.sh` -- the wrapper uses the submodule and the checkout's
# `devices.list`; anywhere else, such as a release tree copied away from it,
# the copy and a `devices.list` at the copied tree's root. Either way the tree
# carries the check with it.
#
# `|| exit 1`, though `set -e` above would stop here too: board.sh refuses to
# load when it has no garden, or one lacking a function it calls, precisely so
# that no check below runs undefined.
# shellcheck source=../tools/board.sh
. "$HERE/board.sh" || exit 1

WHAT="flashing the wrong board replaces its firmware"

# **The board these images are for, by name: `lightning`, the terminal.**
# lightning's list holds more than the terminal -- the station's core, an
# ESP32-P4, is on it -- and any board on the list may be opened, so "which one
# is this image for" has to be said, not left to whichever entry comes first.
# garden's `board_flash_guard` compares the board behind the port with it
# immediately before the write. `$BOARD`, or `$BOARD_MAC` for a one-off, names
# another terminal on the list instead; either is still checked against it.
MEANT="${BOARD_MAC:-${BOARD:-lightning}}"

# **The chip, said once** and given to both the guard and espflash, so the two
# cannot disagree. espflash told `--chip` refuses any other chip after it
# connects and before it writes anything.
CHIP="esp32c3"

if [[ -z "$PORT" ]]; then
    # 1 is an absent board, 2 a refusal; `board_port` has said which on
    # stderr. The advice below is for an absent board only -- after a refusal
    # it would point the way round it.
    status=0
    PORT="$(board_port "$MEANT")" || status=$?
    if (( status == 1 )); then
        cat >&2 <<'EOF'

If it is plugged in and still missing, it is probably asleep: light sleep powers
down the USB Serial/JTAG PHY, so the port comes and goes. Either catch it in a
waking window, or flash the no-light-sleep variant once and it will stay. On
the ordinary build, `awake on` once it is caught keeps it up across restarts.
EOF
    fi
    (( status == 0 )) || exit 1
fi

# The board's by-id path from here on, whatever was given: it is named after
# the board's USB serial, so it cannot come to mean another board, as a ttyACM
# number can between this check and the write.
PORT="$(board_is "$PORT" "$WHAT" "$MEANT")" || exit 1

echo "port    : $PORT"
echo "variant : $TARGET"
echo "purpose : $PURPOSE"

# --- staleness guard --------------------------------------------------------
#
# **Compare firmware inputs, not commits.** "Stamp == HEAD" is the obvious check
# and it is wrong: every commit moves HEAD, the copies `release.sh` puts beside
# this script included, so every image would go stale at the next commit of
# anything at all. What matters is whether anything the binary is BUILT FROM
# has changed since.
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

# --- the write ----------------------------------------------------------------
#
# garden's order, in which **nothing opens the port before it is checked and
# locked**: the port, asked of sysfs and compared with the board meant (above);
# the board's lock; for a board known only by its USB socket, the chip's own
# MAC; and then `board_flash_guard`, immediately before espflash.
#
# **The board's lock** -- the one every tool here takes before it opens the
# port (`tools/portlock.py` for the Python tools) -- so a flash is refused
# while `send.py` or `watch.py` holds the board, and they are refused while
# this writes. Two tools on one port fight over the bootloader handshake and
# fail in ways that look like a bad board. Descriptor 9 stays open until this
# script ends, which keeps the lock; espflash inherits it, which holds the same
# lock, not a second one.
#
# **Every line's status is checked**, the lock path's too: `board_lock_path`
# prints nothing and fails for a board that has gone, bash does not stop on the
# `exec 9>>""` that would follow, and a descriptor 9 inherited from a parent
# would then be locked in place of the board's.
SOCKET="$PORT"
LOCK="$(board_lock_path "$(board_of_port "$SOCKET")")" || exit 1
exec 9>>"$LOCK" || exit 1
flock -n 9 || { echo "$SOCKET is held by another tool -- a send.py, a watch.py, a flash. Refusing." >&2; exit 1; }

# **A board known only by its socket** -- a `path=` entry, a bridge with no USB
# serial -- can be told only by the chip: `espflash board-info` reads its MAC
# and writes nothing, and the guard confirms it against the entry. It opens the
# port, which resets the chip, so it runs only now, checked and locked. A board
# with a USB serial of its own -- the terminal's USB Serial/JTAG has one -- is
# named by that serial, and needs no board-info.
INFO=""
if board_on_socket "$SOCKET"; then
    INFO="$(espflash board-info --chip "$CHIP" --port "$SOCKET" 2>&1)" || { echo "espflash board-info could not read $SOCKET -- refusing." >&2; exit 1; }
fi

# **Asked once more immediately before writing** (garden's
# `board_flash_guard`), of sysfs, never of the port: whether lightning may
# still open it, whether it is still the board these images are for, the chip
# named, and the MAC read above when there was one. It prints the board's
# stable path, which espflash is given from here on, and says which board it
# is on stderr, so a person watching sees it before the write starts.
PORT="$(board_flash_guard "$SOCKET" "$CHIP" "$INFO" "$MEANT")" || exit 1

# `--before usb-reset` because this board has no external reset line the flasher
# can pull; the USB peripheral's own reset is what puts it into the downloader.
#
# **`--chip`, so espflash itself refuses any other chip** -- after it connects
# and before it writes anything. lightning's list holds more than the terminal:
# the station's core is an ESP32-P4, and these images are for the C3.
espflash flash --chip "$CHIP" --port "$PORT" --non-interactive --before usb-reset \
    --bootloader "$DIR/bootloader.bin" \
    --partition-table "$DIR/partition-table.bin" \
    "$DIR/$IMAGE"

cat <<EOF

⚠ Set the clock. Flashing resets the chip, which stops the RTC -- \`restore\`
then falls back to whatever NVS last saved, up to fifteen minutes stale. A
strike stamped in between would carry a timestamp minutes early.

    $HERE/send.py --port $PORT --clock

Not \`echo "date ..." > port\`: opening the port restarts the board, and a line
written at the open reaches it while it boots, before anything reads the
console. send.py holds the port, waits for \`con:  listening\`, and reads the
epoch as it sends it.

Watch it -- and every open restarts it, so the banner you see is that restart's:
    espflash monitor --port $PORT
EOF
