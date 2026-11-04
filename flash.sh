#!/usr/bin/env bash
#
# Flash the lightning terminal.
#
#   ./flash.sh                    # build + flash, then print the monitor line
#   ./flash.sh <port>             # ...on a specific port, checked by MAC
#
# ⚠ Why this script exists rather than `cargo run`.
#
# `espflash flash <app>` writes ONLY the application. The bootloader and the
# partition table already on the board are left alone -- and if they came from
# somewhere else, that is what the app runs under. On first bring-up here the
# board still carried a vendor image, so the boot log read:
#
#     I (24) boot: ESP-IDF v5.5.1-838-gd66ebb86d2e 2nd stage bootloader
#     I (67) boot:  2 factory  factory app  00 00 00010000 003f0000
#
# -- a bootloader from a different IDF release, and a partition table with no
# `storage` region, which is where §5's strike CSV lives. The app loaded and
# immediately reset, printing nothing, which reads exactly like a firmware bug.
#
# Passing all three artifacts makes the board match what was built.
# ⚠ RECOVERY ON THIS BOARD IS NOT THE USUAL DANCE.
#
# The standard advice -- "hold BOOT, unplug and replug USB, release BOOT" --
# **does not work here**, and the reason is easy to miss: this board has its own
# 2000 mAh cell, so unplugging USB does not power-cycle anything. The chip keeps
# running, never leaves reset, and therefore never samples the BOOT strap. The
# device stays enumerated and answers nothing, which looks exactly like a
# hardware fault.
#
# With the board powered, the sequence that works is:
#
#     1. press and hold BOOT
#     2. while holding, press and release RESET
#     3. release BOOT
#
# Easier on this build: the shield board carries a **battery ON/OFF switch**.
# Switch the cell OFF, unplug USB, then plug USB back in while holding BOOT --
# with the cell disconnected the chip genuinely loses power and samples the
# strap on the way back up.
#
# You need this whenever a build with light sleep enabled is running: light
# sleep powers down the USB PHY, so neither the console nor espflash can reach
# the board.
set -euo pipefail

# **Cargo hands the runner the built binary; a person hands it a port.**
#
# `.cargo/config.toml` points `runner` here, so `cargo run` invokes this script
# with the path to the ELF. That is not a port, and treating it as one would
# fail in a confusing way -- so an argument that is an existing regular file is
# recognised as cargo's and dropped. What gets flashed is decided below from the
# target directory either way, which is the same thing cargo just built.
if [[ -n "${1:-}" && -f "${1:-}" ]]; then
    shift
fi

# **By MAC, never by ttyACM<n>.** The numbering shuffles between plug-ins and has
# pointed at a different board before.
#
# The board's identity lives in lightning's own `devices.list`, at the root of
# this checkout, not here -- see tools/board.sh. With no `$BOARD` the board is
# the list's first entry. A board that is not on the list, or that the list
# flags `forbidden`, is refused by MAC, whichever port it is on.
#
# `|| exit 1`, though `set -e` above would stop here too: board.sh refuses to
# load -- garden's submodule missing, or lacking a function lightning calls --
# precisely so that no check below runs undefined, and this says so where it
# happens rather than leaving it to a flag at the top of the file.
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=tools/board.sh
. "$HERE/tools/board.sh" || exit 1

WHAT="flashing the wrong board replaces its firmware"
ASKED_PORT="${1:-}"

# **The board, before the build.** Whether lightning may touch the board meant
# needs no port, so it is asked first: a refusal after several minutes of build
# would be a refusal that wasted them.
board_wanted >/dev/null || exit 1

DIR="$HERE/target/riscv32imc-esp-espidf/release"

cargo build --release

for f in bootloader.bin partition-table.bin lightning; do
    [[ -f "$DIR/$f" ]] || { echo "missing build artifact: $DIR/$f" >&2; exit 1; }
done

# **The port, after the build.** Found or checked before it, a ttyACM number
# would have minutes to change hands -- and a light-sleeping board's port comes
# and goes, so one missing before the build may well be back after it. Either
# way the answer is the board's by-id path, used from here on: `board_is` and
# `board_port` print that path, never the number they were given.
if [[ -n "$ASKED_PORT" ]]; then
    PORT="$(board_is "$ASKED_PORT" "$WHAT")" || exit 1
else
    PORT="$(board_port)" || exit 1
fi

echo "port  : $PORT"
echo "table : $(python3 - "$DIR/partition-table.bin" <<'PY'
import sys
d = open(sys.argv[1], 'rb').read()
names = []
for i in range(0, len(d), 32):
    e = d[i:i+32]
    if e[:2] != b'\xaa\x50':
        break
    names.append(e[12:28].rstrip(b'\x00').decode())
print(' '.join(names))
PY
)"
echo

# `--before usb-reset` is not optional on this board, and it is the whole
# reason `recover.sh` exists in the shape it does.
#
# This chip talks USB-Serial/JTAG. espflash's default, `default-reset`, drives
# the DTR and RTS control lines -- the sequence for a board with a USB-to-UART
# bridge, which this is not. It fails with a bare "Failed to connect to the
# device", which reads as "espflash cannot talk to this chip" and sent a whole
# evening into working around the wrong tool. `usb-reset` is the sequence for
# the USB-JTAG-Serial peripheral, and it connects first time.
#
# **`--chip esp32c3`, so espflash itself refuses any other chip** -- after it
# connects and before it writes anything. lightning's list holds more than the
# terminal: the station's core is an ESP32-P4, and without this an image built
# for the C3 terminal would be written to it.
#
# **Asked once more immediately before writing**, of sysfs, never of the port:
# whether lightning may still open it, and whether it is still the board meant.
board_is "$PORT" "$WHAT" >/dev/null || exit 1
espflash flash --chip esp32c3 --port "$PORT" --non-interactive --before usb-reset \
    --bootloader "$DIR/bootloader.bin" \
    --partition-table "$DIR/partition-table.bin" \
    "$DIR/lightning"

echo
echo "flashed. Set the clock -- flashing stops the RTC -- with the tool that waits for"
echo "the console, since opening the port restarts the board:"
echo "    $HERE/tools/send.py --port $PORT --clock"
echo "Monitor with:"
echo "    espflash monitor --port $PORT"
