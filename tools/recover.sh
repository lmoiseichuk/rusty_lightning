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
# ⚠ TRY `./flash.sh` FIRST. THIS SCRIPT ERASES YOUR DATA.
#
# `esptool erase-flash` erases the **whole chip**, which includes the `storage`
# partition holding the strike log and the `nvs` holding the clock epoch, the
# defence point, the quiet threshold and the mode. Thousands of records go with
# it. `espflash flash` writes only the bootloader, the partition table and the
# app, and leaves both of those alone.
#
# ## The diagnosis this script was built on was wrong
#
# It said espflash 4.5.0 "could not connect to this board at all — dozens of
# attempts across every combination of `--before`", and concluded that esptool
# was the only tool that could get in.
#
# The reset sequence was the right thing to suspect and the wrong conclusion to
# draw. espflash has a `--before usb-reset`, which is the sequence for the
# USB-JTAG-Serial peripheral this chip actually talks; the default,
# `default-reset`, drives DTR/RTS, which is for a USB-to-UART bridge. With
# `--before usb-reset` espflash connects first time and flashes in four seconds
# — confirmed 2026-08-23, on the same board, with the strike log intact
# afterwards. `flash.sh` now passes it.
#
# So this script is for a board that is genuinely unreachable — a half-written
# flash, or a light-sleep build with no console — and not for "espflash said it
# could not connect". Check `--before` before erasing anything.
#
# When it *is* needed: an erased flash has no app, so the ROM falls back to the
# downloader and stays there, with no button and no timing. espflash then
# writes, because it is what turns our ELF into a flashable image without a
# separate conversion step.
set -uo pipefail

# **Find the board by its MAC, never by ttyACM<n>.**
#
# This defaulted to a bare `/dev/ttyACM0`, and this is the one script in the repo
# that erases a whole chip. The numbering shuffles between plug-ins -- it has
# already pointed at a different board -- so the default was one port shuffle
# away from erasing somebody else's log. `release/flash.sh` looks the board up by
# MAC and this had no equivalent.
readonly BOARD_MAC="B0:A6:04:06:E6:D4"

if [[ -n "${1:-}" ]]; then
    PORT="$1"
else
    found="$(ls /dev/serial/by-id/ 2>/dev/null | grep -m1 "$BOARD_MAC" || true)"
    if [[ -z "$found" ]]; then
        echo "The lightning board ($BOARD_MAC) is not on /dev/serial/by-id." >&2
        echo "Pass a port explicitly if you are sure, or check the power switch:" >&2
        echo "in battery mode with no cell fitted the board never enumerates." >&2
        exit 1
    fi
    PORT="/dev/serial/by-id/$found"
fi

# An explicit port is still checked, because being explicit is not the same as
# being right.
#
# **This used to check only paths that already began `/dev/serial/by-id/`.**
# The condition was `by-id/* && != *MAC*`, so both halves had to hold to refuse
# -- and a bare `/dev/ttyACM0` failed the first half, skipped the guard
# entirely, and went straight to `esptool erase-flash`. The comment above it
# claimed the opposite. That is the exact shuffle this script's own header
# describes as "one port shuffle away from erasing somebody else's log", left
# open on the path a person types by hand when they are in a hurry.
#
# So resolve whatever was given back to a by-id name and check *that*. Both
# paths point at the same character device, so comparing `readlink -f` is
# enough and needs no udev query.
resolve_by_id() {
    local port="$1" link target
    target="$(readlink -f -- "$port" 2>/dev/null)" || return 1
    [[ -n "$target" ]] || return 1
    for link in /dev/serial/by-id/*; do
        [[ -e "$link" ]] || continue
        if [[ "$(readlink -f -- "$link")" == "$target" ]]; then
            printf '%s\n' "$link"
            return 0
        fi
    done
    return 1
}

if [[ "${RECOVER_SKIP_IDENTITY_CHECK:-}" == "1" ]]; then
    echo "⚠ RECOVER_SKIP_IDENTITY_CHECK=1 -- erasing $PORT without checking which board it is."
elif resolved="$(resolve_by_id "$PORT")"; then
    if [[ "$resolved" != *"$BOARD_MAC"* ]]; then
        echo "⚠ $PORT is not the lightning board." >&2
        echo "  it resolves to: $resolved" >&2
        echo "  expected MAC:   $BOARD_MAC" >&2
        echo "This script ERASES THE WHOLE CHIP. Refusing." >&2
        exit 1
    fi
else
    echo "⚠ $PORT has no /dev/serial/by-id entry, so its identity cannot be" >&2
    echo "  checked -- and this script ERASES THE WHOLE CHIP, including the" >&2
    echo "  strike log and NVS. Refusing." >&2
    echo >&2
    echo "If the board really is this one and udev has not named it, set" >&2
    echo "RECOVER_SKIP_IDENTITY_CHECK=1 deliberately:" >&2
    echo >&2
    echo "    RECOVER_SKIP_IDENTITY_CHECK=1 $0 $PORT" >&2
    exit 1
fi
MAX_ATTEMPTS="${2:-8}"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# **Its own target directory, and the recovery feature is not optional.**
#
# This read `target/riscv32imc-esp-espidf/release` -- the same directory
# `flash.sh` builds the *default* image into -- and only advised building with
# `--features no-light-sleep` in an error message nothing enforced. The natural
# sequence made that fatal: `flash.sh` fails, leaving exactly the light-sleep
# binary that broke the console sitting in `target/`, and recovery writes it
# straight back. The board comes up unreachable again and the run looks like a
# success.
#
# A separate `--target-dir` means the recovery image cannot be a leftover of
# anything else, and building it here means it cannot be forgotten.
RECOVERY_TARGET="$HERE/target/recovery"
DIR="$RECOVERY_TARGET/riscv32imc-esp-espidf/release"

need_build=0
for f in bootloader.bin partition-table.bin lightning; do
    [[ -f "$DIR/$f" ]] || need_build=1
done

if (( need_build )); then
    echo "building the recovery image (light sleep compiled out) ..."
    if ! (cd "$HERE" && cargo build --release --features no-light-sleep \
              --target-dir "$RECOVERY_TARGET"); then
        echo "the recovery build failed -- not erasing anything" >&2
        exit 1
    fi
fi

for f in bootloader.bin partition-table.bin lightning; do
    if [[ ! -f "$DIR/$f" ]]; then
        echo "missing after build: $DIR/$f" >&2
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

# **The wait is bounded too.** This loop used to `continue` without touching
# `attempt`, so a port that never appears spun here for ever -- directly against
# this script's own promise to "stop with a diagnosis rather than looping
# forever", and making its "port never appeared" message unreachable. A separate
# counter, because waiting for a port and failing to talk to one are different
# failures and deserve different diagnoses.
waited=0
readonly MAX_WAIT=150   # 150 x 2 s = five minutes

while (( attempt < MAX_ATTEMPTS )); do
    if [[ ! -e "$PORT" ]]; then
        if (( waited >= MAX_WAIT )); then
            echo
            echo "[$(stamp)] $PORT never appeared after $(( MAX_WAIT * 2 )) s." >&2
            echo "The board is not enumerating. Check the power switch position:" >&2
            echo "in battery mode with no cell fitted, USB does not reach the rail" >&2
            echo "and the board will not boot at all." >&2
            exit 1
        fi
        waited=$((waited + 1))
        printf "\r[%s] waiting for %s -- SAFE to power-cycle    " "$(stamp)" "$PORT"
        sleep 2
        continue
    fi
    waited=0
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
    # **Gated on the exit status, not on the wording.** This tested only
    # `grep -q 'Flashing has completed'` and threw the exit status away, while
    # the erase above was `if !`-gated -- so if espflash ever rephrases that
    # line, a *successful* write is reported failed, the loop comes round and
    # erases the flash it just wrote, and after eight attempts it announces
    # FAILED on a board that was recovered on the first one. The grep stays,
    # as a display detail rather than as the verdict.
    write_status=0
    write_out=$(timeout 90 espflash flash --port "$PORT" --non-interactive \
                    --chip esp32c3 \
                    --bootloader "$DIR/bootloader.bin" \
                    --partition-table "$DIR/partition-table.bin" \
                    "$DIR/lightning" 2>&1) || write_status=$?
    if (( write_status == 0 )); then
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

  4. **Flash something that never sleeps, then come back.** Any known-good
     always-awake firmware will do -- MicroPython for the ESP32-C3 is the
     one that worked here. It holds the USB PHY up permanently, so the
     port goes stable and the timing race disappears entirely. Once it is
     running, `esptool erase-flash` has all the time it needs, and this
     script will then work first try.

     That is the reliable way out of a light-sleep lockout, and it beats
     any amount of retrying.
EOF
fi
exit 1
