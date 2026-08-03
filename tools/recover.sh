#!/usr/bin/env bash
#
# Recover a board that will not flash, without anyone having to hold a button
# while also watching a terminal for the right moment.
#
# ## Why recovery is awkward on this board
#
# The panel has its own cell **and** a battery ON/OFF switch, so unplugging USB
# does not power-cycle anything. Without a true power cycle the BOOT strap is
# never sampled, and a build with light sleep enabled is reachable only in the
# brief windows when its USB PHY happens to be powered.
#
# ## The trap this script avoids
#
# `espflash` defaults to `--before default-reset`, which asserts DTR/RTS — and on
# the ESP32-C3 those lines drive the reset and boot straps. So a retry loop using
# the default can **knock the board out of a download mode it had already
# entered**, then fail on the next attempt for a reason it caused itself. That is
# the most plausible reading of why 25 rapid retries all failed against a board
# that was, at least some of the time, sitting exactly where we wanted it.
#
# So `no-reset` is tried first: it is the mode that works on a board already in
# the ROM downloader, and the one that cannot knock it back out.
#
# ## Use
#
#     1. battery switch OFF, unplug USB
#     2. start this script
#     3. hold BOOT, plug USB in, release BOOT
#
# It keeps trying for several minutes, so there is nothing to co-ordinate.
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

echo "waiting for $PORT -- hold BOOT and apply power whenever you are ready"
for attempt in $(seq 1 150); do
    if [[ -e "$PORT" ]]; then
        for before in no-reset default-reset; do
            out=$(timeout 25 espflash flash --port "$PORT" --non-interactive \
                      --chip esp32c3 --before "$before" \
                      --bootloader "$DIR/bootloader.bin" \
                      --partition-table "$DIR/partition-table.bin" \
                      "$DIR/lightning" 2>&1)
            if grep -q 'Flashing has completed' <<<"$out"; then
                echo
                echo "flashed on attempt $attempt (--before $before)"
                exit 0
            fi
        done
        printf "."
    fi
    sleep 1
done

echo
echo "no luck after 150 attempts."
echo "Check the battery switch is OFF: with the cell connected the board never"
echo "actually loses power, so the BOOT strap is never sampled and the dance is"
echo "a no-op."
exit 1
