#!/usr/bin/env bash
#
# Flash the lightning terminal.
#
#   ./flash.sh                    # build + flash + monitor
#   ./flash.sh /dev/ttyACM1       # ...on a specific port
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
set -euo pipefail

PORT="${1:-/dev/ttyACM0}"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DIR="$HERE/target/riscv32imc-esp-espidf/release"

cargo build --release

for f in bootloader.bin partition-table.bin lightning; do
    [[ -f "$DIR/$f" ]] || { echo "missing build artifact: $DIR/$f" >&2; exit 1; }
done

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

espflash flash --port "$PORT" --non-interactive \
    --bootloader "$DIR/bootloader.bin" \
    --partition-table "$DIR/partition-table.bin" \
    "$DIR/lightning"

echo
echo "flashed. Monitor with:"
echo "    espflash monitor --port $PORT"
