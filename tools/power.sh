#!/usr/bin/env bash
#
# Power-cycle a board by name, using its hub and port from `devices.list`.
#
#     tools/power.sh                    # cycle the default board
#     tools/power.sh lightning          # by name
#     tools/power.sh --list             # what is on the switchable hubs
#
# ## Why this exists
#
# Light sleep powers down the USB Serial/JTAG PHY, so a board running the
# ordinary build appears and disappears in windows -- and when it stops
# appearing at all, nothing in software can reach it. `espflash` cannot connect,
# the console cannot attach, and there is no reset line. Cutting VBUS is the
# only remaining lever.
#
# ## What it refuses, and why
#
# **Only boards named in `devices.list`.** A hub port number is not a board: the
# ports on this bench carry devices from more than one project, and a script
# that took a port number would have no way to know it had been given the wrong
# one. Taking a *name* means the refusal can be specific.
#
# **It says who else is on the hub first.** These Genesys hubs report `ganged`,
# which would mean cutting one port cuts them all. Measured on this bench it is
# false -- port 1 was cycled with a live device on port 2 and the neighbour did
# not blink -- but "measured false once" is not "cannot happen", and the
# neighbour may belong to somebody else's running experiment. So it is named
# before the cut rather than discovered after it.
set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=board.sh
. "$HERE/board.sh"

UHUBCTL="${UHUBCTL:-}"
if [[ -z "$UHUBCTL" ]]; then
    for candidate in uhubctl /usr/sbin/uhubctl /usr/local/sbin/uhubctl; do
        if command -v "$candidate" >/dev/null 2>&1 || [[ -x "$candidate" ]]; then
            UHUBCTL="$candidate"
            break
        fi
    done
fi
if [[ -z "$UHUBCTL" ]]; then
    echo "uhubctl not found. It is often installed to /usr/sbin, which is not" >&2
    echo "on a normal PATH -- set UHUBCTL=/path/to/uhubctl if it is elsewhere." >&2
    exit 1
fi

# `-f` is required: without it this hub reports "No compatible devices detected".
if [[ "${1:-}" == "--list" || "${1:-}" == "list" ]]; then
    "$UHUBCTL" -f
    exit $?
fi

BOARD_NAME="${1:-${BOARD:-lightning}}"

read -r HUB PORT < <(board_location "$BOARD_NAME") || {
    echo "No hub and port for '$BOARD_NAME' in devices.list." >&2
    echo >&2
    echo "Add them as the third and fourth columns -- find them with:" >&2
    echo "    $UHUBCTL -f" >&2
    echo >&2
    echo "A board on a root port cannot be power-cycled at all." >&2
    exit 1
}

MAC="$(BOARD="$BOARD_NAME" board_mac)" || { board_mac_missing; exit 1; }

echo "board : $BOARD_NAME ($MAC)"
echo "hub   : $HUB port $PORT"

# Everything else on the same hub, named where this checkout knows the name.
neighbours="$("$UHUBCTL" -f 2>/dev/null | sed -n "/hub $HUB /,/^Current status/p" \
    | grep -E "^  Port [0-9]+:" | grep -v "^  Port $PORT:" | grep "connect \[" || true)"
if [[ -n "$neighbours" ]]; then
    echo "also on this hub -- the hub claims 'ganged', measured false here:"
    while IFS= read -r line; do
        other_mac="$(grep -oiE '([0-9A-F]{2}:){5}[0-9A-F]{2}' <<<"$line" | head -1)"
        other_name=""
        [[ -n "$other_mac" ]] && other_name="$(board_name_of "$other_mac" 2>/dev/null)"
        printf '  %s%s\n' "$(sed 's/^  //' <<<"$line" | cut -c1-72)" \
            "${other_name:+   <- $other_name}"
    done <<<"$neighbours"
fi

before="$(ls /dev/serial/by-id/ 2>/dev/null | wc -l)"
echo "cycling ..."
"$UHUBCTL" -l "$HUB" -p "$PORT" -a cycle -d "${OFF_SECONDS:-3}" -f >/dev/null 2>&1

# Enumeration is not instant, and a board that boots into light sleep may take a
# waking window to appear. Wait for the name rather than a fixed sleep.
target="$MAC"
for _ in $(seq 1 40); do
    if ls /dev/serial/by-id/ 2>/dev/null | grep -q "$target"; then
        echo "back : /dev/serial/by-id/$(ls /dev/serial/by-id/ | grep -m1 "$target")"
        after="$(ls /dev/serial/by-id/ 2>/dev/null | wc -l)"
        if (( after < before )); then
            echo "⚠ $((before - after)) other device(s) did not come back -- check the hub" >&2
            exit 1
        fi
        exit 0
    fi
    sleep 0.5
done

echo "⚠ $BOARD_NAME did not re-enumerate within 20 s." >&2
echo "  If it runs the light-sleep build the port comes and goes; try again," >&2
echo "  or check the board's own power switch." >&2
exit 1
