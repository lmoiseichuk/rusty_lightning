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
# ## Use esptool to get in, espflash to write
#
# **This is the whole lesson of a long evening.** `espflash` 4.5.0 could not
# connect to this board at all — dozens of attempts across every combination of
# `--before`, with and without the stub, hanging or failing instantly. A bare
#
#     esptool erase-flash
#
# with no arguments connected first time and erased the chip in 1.8 seconds.
#
# The difference is the reset sequence. This chip talks USB-Serial/JTAG, and
# entering the downloader over it means driving the host's CDC control lines in
# a particular order; esptool implements that correctly and espflash evidently
# does not, on this chip and this version. Every `--before` variation here was
# working around the wrong tool.
#
# So: **esptool erases, and the erase is what makes the board reachable** — an
# erased flash has no app, so the ROM falls back to the downloader and stays
# there, with no button and no timing. espflash then writes, because it is what
# turns our ELF into a flashable image without a separate conversion step.
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

    echo "[$(stamp)] attempt $attempt/$MAX_ATTEMPTS: ERASING with esptool -- do not power-cycle"
    # Deliberately bare. esptool's own defaults handle USB-Serial/JTAG; every
    # override tried here made it worse, and espflash could not connect at all.
    if ! erase_out=$(timeout 40 esptool --port "$PORT" erase-flash 2>&1); then
        last_error=$(grep -iE 'error|exception|failed' <<<"$erase_out" | head -2 | tr '\n' ' ')
        [[ -z "$last_error" ]] && last_error="erase timed out with no output (port vanished mid-attempt)"
        echo "           erase failed: $last_error"
        continue
    fi
    echo "           $(grep -i 'erased successfully' <<<"$erase_out" | head -1)"

    # The chip now has no app, so it sits in the ROM downloader -- which is the
    # easy case, and why the write below needs no coaxing.
    echo "[$(stamp)] WRITING -- do not power-cycle"
    write_out=$(timeout 90 espflash flash --port "$PORT" --non-interactive \
                    --chip esp32c3 \
                    --bootloader "$DIR/bootloader.bin" \
                    --partition-table "$DIR/partition-table.bin" \
                    "$DIR/lightning" 2>&1)
    if grep -q 'Flashing has completed' <<<"$write_out"; then
        echo
        echo "=============================================="
        echo " RECOVERED on attempt $attempt"
        echo " Flash erased and rewritten. Safe to power-cycle."
        echo "=============================================="
        exit 0
    fi

    last_error=$(grep -iE 'error|╰─▶|exception' <<<"$write_out" | head -2 | tr '\n' ' ')
    [[ -z "$last_error" ]] && last_error="write produced no completion message"
    echo "           write failed: $last_error"
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
