#!/usr/bin/env bash
#
# Recover a board that will not flash.
#
#     ./tools/recover.sh [port]
#
# Leave it running and power-cycle the board whenever you like — it says on
# every line whether it is safe to do so.
#
# ## Why recovery is awkward on this board
#
# The panel has its own cell **and** a battery ON/OFF switch, so unplugging USB
# does not power-cycle anything. Without a true power cycle the BOOT strap is
# never sampled. And BOOT/RESET are physically awkward to reach under the
# shield, so the usual dance is not a reliable option here.
#
# A build with light sleep enabled makes it worse: light sleep powers down the
# USB PHY, so the port comes and goes (measured: ~12 s present, ~5 s absent) and
# any single attempt is a coin toss. Hence the loop.
#
# ## The trap this script avoids
#
# `espflash` defaults to `--before default-reset`, which asserts DTR/RTS — and on
# the ESP32-C3 those lines drive the reset and boot straps. So a retry loop using
# the default can **knock the board out of a download mode it had already
# entered**, then fail on the next attempt for a reason it caused itself. So
# `no-reset` is tried first: it is the mode that works on a board already in the
# ROM downloader, and the one that cannot knock it back out.
#
# ## Erase first
#
# Recovery erases the whole flash before writing. That is deliberate: a board in
# this state may hold a partition table or NVS contents that are part of why it
# will not start, and recovery should not depend on any of it being sane. The
# cost is the stored settings — the indoor/outdoor mode and the learned battery
# range — both of which the firmware re-creates from its defaults.
set -uo pipefail

PORT="${1:-/dev/ttyACM0}"
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

# Ctrl-C should not leave the board half-erased without saying so.
trap 'echo; echo "interrupted -- if this stopped during ERASE or WRITE the flash"; echo "is incomplete, so run this again before power-cycling."; exit 130' INT

say_safe() { echo "[$(date +%H:%M:%S)] idle -- SAFE TO POWER-CYCLE"; }

echo "watching $PORT. Power-cycle whenever you like; this will catch it."
echo "Battery switch OFF, unplug USB, wait ~15 s, then plug USB back in."
echo

attempt=0
while true; do
    attempt=$((attempt + 1))

    if [[ ! -e "$PORT" ]]; then
        say_safe
        sleep 2
        continue
    fi

    for before in no-reset default-reset; do
        echo "[$(date +%H:%M:%S)] attempt $attempt (--before $before): ERASING -- DO NOT POWER-CYCLE"
        if ! timeout 60 espflash erase-flash --port "$PORT" --chip esp32c3 \
                --before "$before" >/dev/null 2>&1; then
            continue
        fi

        echo "[$(date +%H:%M:%S)] erased. WRITING -- DO NOT POWER-CYCLE"
        # After an erase the chip is already in the flasher stub, so no-reset is
        # the right choice regardless of how the erase got in.
        if timeout 90 espflash flash --port "$PORT" --non-interactive \
                --chip esp32c3 --before no-reset \
                --bootloader "$DIR/bootloader.bin" \
                --partition-table "$DIR/partition-table.bin" \
                "$DIR/lightning" 2>&1 | grep -q 'Flashing has completed'; then
            echo
            echo "[$(date +%H:%M:%S)] *** RECOVERED *** (attempt $attempt, --before $before)"
            echo "Flash erased and rewritten. Safe to power-cycle now."
            exit 0
        fi

        echo "[$(date +%H:%M:%S)] write failed after a good erase -- retrying"
    done

    say_safe
    sleep 1
done
