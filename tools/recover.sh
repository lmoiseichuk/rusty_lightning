#!/usr/bin/env bash
#
# Recover a board that will not flash.
#
#     ./tools/recover.sh [port] [max-attempts]
#
# Stops on the first success, and stops with a diagnosis rather than looping
# forever if it cannot get in. Every failure prints what espflash actually said,
# because an endless run of identical progress lines tells you nothing about
# whether it is working.
#
# ## Why recovery is awkward on this board
#
# The panel has its own cell **and** a battery ON/OFF switch, so unplugging USB
# does not power-cycle anything, and BOOT/RESET are awkward to reach under the
# shield — so the usual "hold BOOT and replug" is not reliably available.
#
# A build with light sleep enabled makes it worse: light sleep powers down the
# USB PHY, so the port comes and goes (measured: ~12 s present, ~5 s absent) and
# any single attempt is a coin toss. Hence retrying — but a bounded number of
# times, so a real problem surfaces instead of scrolling past.
#
# ## Two traps this script exists to avoid
#
# * `espflash` defaults to `--before default-reset`, which asserts DTR/RTS — and
#   on the C3 those drive the reset and boot straps. A retry loop on the default
#   can knock the board out of a download mode it had already entered, then fail
#   next time for a reason it caused itself. So `no-reset` is tried first.
#
# * `espflash erase-flash` defaults to `--after hard-reset`, which reboots the
#   chip out of the bootloader the moment the erase finishes — so the write that
#   follows talks to a device that has already left. A successful erase followed
#   by a guaranteed failed write, every time. Hence `--after no-reset`.
set -uo pipefail

PORT="${1:-/dev/ttyACM0}"
MAX_ATTEMPTS="${2:-8}"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DIR="$HERE/target/riscv32imc-esp-espidf/release"

for f in bootloader.bin partition-table.bin lightning; do
    if [[ ! -f "$DIR/$f" ]]; then
        echo "missing: $DIR/$f" >&2
        echo "build first, ideally the recovery image:" >&2
        echo "    cargo build --release --features no-light-sleep" >&2
        exit 1
    fi
done

trap 'echo; echo "interrupted. If this stopped during ERASE or WRITE the flash is"; echo "incomplete -- run this again before power-cycling."; exit 130' INT

stamp() { date +%H:%M:%S; }

cat <<EOF
Recovering via $PORT, up to $MAX_ATTEMPTS attempts.

  Battery switch OFF -> unplug USB -> wait 15 s -> plug USB back in.

Lines marked SAFE mean you can power-cycle. Anything else means a flash
operation is in flight -- leave the board alone until it says otherwise.

EOF

last_error=""
attempt=0
port_seen=0

while (( attempt < MAX_ATTEMPTS )); do
    if [[ ! -e "$PORT" ]]; then
        printf "\r[%s] waiting for %s -- SAFE to power-cycle    " "$(stamp)" "$PORT"
        sleep 2
        continue
    fi
    port_seen=1
    attempt=$((attempt + 1))
    echo

    for before in no-reset default-reset; do
        echo "[$(stamp)] attempt $attempt/$MAX_ATTEMPTS ($before): ERASING -- do not power-cycle"
        erase_out=$(timeout 20 espflash erase-flash --port "$PORT" --chip esp32c3 \
                        --before "$before" --after no-reset 2>&1)
        if [[ $? -ne 0 ]]; then
            last_error=$(grep -iE 'error|╰─▶|exception' <<<"$erase_out" | head -2 | tr '\n' ' ')
            [[ -z "$last_error" ]] && last_error="erase timed out with no output (port vanished mid-attempt)"
            echo "           erase failed: $last_error"
            continue
        fi

        echo "[$(stamp)] erased OK. WRITING -- do not power-cycle"
        write_out=$(timeout 60 espflash flash --port "$PORT" --non-interactive \
                        --chip esp32c3 --before no-reset \
                        --bootloader "$DIR/bootloader.bin" \
                        --partition-table "$DIR/partition-table.bin" \
                        "$DIR/lightning" 2>&1)
        if grep -q 'Flashing has completed' <<<"$write_out"; then
            echo
            echo "=============================================="
            echo " RECOVERED on attempt $attempt (--before $before)"
            echo " Flash erased and rewritten. Safe to power-cycle."
            echo "=============================================="
            exit 0
        fi

        last_error=$(grep -iE 'error|╰─▶|exception' <<<"$write_out" | head -2 | tr '\n' ' ')
        [[ -z "$last_error" ]] && last_error="write produced no completion message"
        echo "           write failed: $last_error"
    done
done

echo
echo "=============================================="
echo " FAILED after $MAX_ATTEMPTS attempts"
echo "=============================================="
echo "last error: ${last_error:-none captured}"
echo
if (( port_seen == 0 )); then
    cat <<'EOF'
The port never appeared at all. That is a cable, a power, or a
battery-switch problem rather than a firmware one -- check the board is
actually powered and enumerating (`dmesg | tail`).
EOF
else
    cat <<'EOF'
The port appeared but the chip never answered the flasher. That means it
is running firmware rather than sitting in the ROM downloader -- a power
cycle alone does not enter download mode, the BOOT strap has to be low at
the moment power comes up.

Things to try, in order:

  1. Leave it unpowered LONGER. Battery switch OFF, USB out, wait 30 s.
     The rails hold enough charge to keep the chip alive through a short
     gap, so it never actually resets and never samples the strap.

  2. Hold BOOT while applying power, if it can be reached at all.

  3. If the board is stuck in a reboot loop, its USB windows are short
     and irregular. Run this again -- catching one is partly luck.
EOF
fi
exit 1
