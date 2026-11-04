# garden project, Copyright (c) 2026 Leonid Moiseichuk under MIT License

# Which boards a project has, and which of them a script may touch.
#
# **Each project works with its own boards, and only those.** A project keeps
# its own `devices.list` at its root -- gitignored, the boards that project
# uses -- and every tool here refuses any board that is not on it. The code is
# shared, through the project's `garden` submodule, so it is written once; the
# lists are not, because a project has no business with another's boards.
#
# **The MACs are not in the scripts, and not in any repository.** They are a
# bench's identities; the scripts are the part worth sharing. `devices.list` is
# gitignored, garden's `devices.list.example` shows the format, and a clone is
# somebody else's bench rather than a copy of this one.
#
# Sourced by `tools/power.sh`, and by a project's own scripts through the
# wrapper in its `tools/`. A project carries garden as a submodule,
# `<project>/garden`, so the wrapper reaches this file at
# `../garden/tools/board.sh`, and this file finds the project's list two levels
# up from itself.
#
# Where a MAC comes from, first match wins:
#
#   1. the name passed to `board_mac`
#   2. `$BOARD_MAC` in the environment    -- for a one-off or for CI
#   3. the entry named `$BOARD`, or the list's first
#
# Format: `name  MAC  [hub  port]  [flags]  [serial=<usb serial>]  [# why]`.
#
# **Hub and port are the fallback that makes a missing board reachable.** While
# a board is plugged in, sysfs is the better answer -- it is live, so it follows
# a replug. But a board that has wedged, gone to sleep with its USB PHY down, or
# stopped enumerating for any other reason is *absent from sysfs entirely*, and
# that is exactly when its power needs cutting. Without a stored location there
# is nothing left to cut.

# The project's devices file, or failure if there is none.
#
# **Only ever the project's own**: `devices.list` at the root of the project
# this garden is a submodule of -- `moisture/devices.list` for
# `moisture/garden/tools/board.sh`. There is no way to point it anywhere else.
# An environment variable that could would let a leftover setting send a
# project's flash script to some other list's boards, while `board.py`, which
# passes no environment through, kept to the project's own: one rule giving two
# answers. A test lays out a tree of its own instead -- a directory holding a
# `devices.list` and a `garden/tools/` with a copy of this file.
#
# **The root is found by trimming the path, not by `..`.** `dirname` on the
# path this file was sourced by names the project the way the caller reached
# it; a `..` would be resolved by the kernel through any symbolic link on the
# way, and could land in some other project's root.
#
# **Never one of garden's own.** Garden has no boards; run from garden's own
# checkout, there is no list two levels up, and every board is refused.
devices_list() {
    local here root
    here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
    root="$(dirname "$(dirname "$here")")"
    if [[ -f "$root/devices.list" ]]; then
        printf '%s\n' "$root/devices.list"
        return 0
    fi
    return 1
}

# The MAC for a name, uppercased. With no name, the list's first entry.
#
# `$BOARD_MAC` stands in only when no name is given: a name or MAC typed on the
# command line -- `power.sh cycle <name>` -- is what is meant, and a variable
# left over in the environment must not quietly replace it.
board_mac() {
    if [[ -z "${1:-}" && -n "${BOARD_MAC:-}" ]]; then
        printf '%s\n' "${BOARD_MAC^^}"
        return 0
    fi
    local want="${1:-${BOARD:-}}" list
    list="$(devices_list)" || return 1
    awk -v want="${want,,}" '
        /^[[:space:]]*(#|$)/ { next }
        { name = tolower($1) }
        want != "" && name == want { print toupper($2); found = 1; exit }
        want == "" && !first { first = toupper($2) }
        END { if (!found && want == "" && first) print first }
    ' "$list"
}

# Which name a MAC belongs to, so a message can say more than the hex.
board_name_of() {
    local mac="${1^^}" list
    list="$(devices_list)" || return 1
    awk -v mac="$mac" '
        /^[[:space:]]*(#|$)/ { next }
        toupper($2) == mac { print $1; exit }
    ' "$list"
}

# The stored `hub port` for a name or MAC, if the entry has one.
#
# The fallback for a board sysfs cannot see. Empty when the entry does not say,
# which is the honest answer for a board on a root port or one whose location
# was never recorded.
board_location() {
    local want="$1" list
    list="$(devices_list)" || return 1
    awk -v want="${want,,}" '
        /^[[:space:]]*(#|$)/ { next }
        (tolower($1) == want || toupper($2) == toupper(want)) {
            # hub is like 5-1; port is the bare number after it
            for (i = 3; i < NF; i++) {
                if ($i ~ /^#/) break
                if ($i ~ /^[0-9]+-[0-9.]+$/ && $(i+1) ~ /^[0-9]+$/) {
                    print $i, $(i+1)
                    exit
                }
            }
            exit 1
        }
    ' "$list"
}

# Every flag on the entry owning `$1` (a MAC), space separated.
#
# Flags are any bare words after the MAC that are not a hub/port pair and not
# a `serial=`, so the columns stay optional without needing placeholders.
board_flags() {
    local mac="${1^^}" list
    list="$(devices_list)" || return 1
    awk -v mac="$mac" '
        /^[[:space:]]*(#|$)/ { next }
        toupper($2) != mac { next }
        {
            out = ""
            for (i = 3; i <= NF; i++) {
                if ($i ~ /^#/) break
                # A hub is like 5-1 or 1-1.2; a port is a bare number.
                if ($i ~ /^[0-9]+-[0-9.]+$/ || $i ~ /^[0-9]+$/) continue
                if ($i ~ /^serial=/) continue
                out = out (out == "" ? "" : " ") $i
            }
            print out
            exit
        }
    ' "$list"
}

# Why a script here must not write to or power-cycle this board -- printed, with
# success -- or nothing and failure when it may.
#
# **The one place the decision is made**, so `board_forbidden` and
# `board_forbidden_why` cannot disagree: both are this, one keeping the answer
# and the other the reason. A board is refused, wherever it is plugged in, when:
#
# * **nothing names it** -- an empty argument. A guard handed no board cannot
#   say which board it is guarding, so it says no.
# * **there is no list.** The list is where the project's boards are; without
#   it there is nothing to check against, and "nothing to refuse" is the wrong
#   answer.
# * **the project's list does not have it.** Each project works with its own
#   boards only, and a board it has not listed is somebody else's, or nobody's
#   yet.
# * **it is flagged `forbidden`**: the project lists it so that it is never
#   written to or power-cycled -- a probe that must be replugged rather than
#   cut, a live device whose interruption costs more than reaching it.
#
# Failing closed is the point: the board that must not be touched is exactly
# the one a confused invocation would otherwise reach.
#
# **The argument is the identity column** -- a MAC, or a small sensor's die id
# -- never a name. `board_mac` turns a name into one.
board_refusal() {
    local identity="${1^^}" list note
    if [[ -z "$identity" ]]; then
        echo "no board given -- this cannot tell which board it is guarding"
        return 0
    fi
    if ! list="$(devices_list)"; then
        echo "there is no devices.list for this project, so nothing can be checked"
        return 0
    fi
    if [[ -z "$(board_name_of "$identity")" ]]; then
        echo "$identity is not on this project's devices.list -- not one of its boards"
        return 0
    fi
    if [[ " $(board_flags "$identity") " == *" forbidden "* ]]; then
        # The entry's own note says why, when it has one.
        note="$(awk -v mac="$identity" '
            /^[[:space:]]*(#|$)/ { next }
            toupper($2) == mac && index($0, "#") { print substr($0, index($0, "#") + 1); exit }
        ' "$list" | sed 's/^ *//')"
        echo "${note:-flagged forbidden in devices.list}"
        return 0
    fi
    return 1
}

# True when a script here must not write to or power-cycle this board. See
# `board_refusal`, which decides.
#
# **The flag travels with the board, not with the port.** Port numbers and
# ttyACM<n> both move; the MAC is printed on the chip. A guard keyed to anything
# else is one replug away from protecting the wrong device.
board_forbidden() {
    board_refusal "$1" >/dev/null
}

# Why it is refused, for a message. Prints nothing for a board that is not.
board_forbidden_why() {
    board_refusal "$1" || true
}

# --- ports ------------------------------------------------------------------
#
# **Which board is behind a port, asked without opening it.** Opening the port
# is what restarts most of these boards, so the moment a script opens one is
# already too late to find out it was the wrong one. The kernel says which USB
# device a port belongs to, and that device's serial number says which board:
# all of it is in sysfs, and reading sysfs touches no port.

# The USB serial number of the device behind a port, or nothing.
#
# `/sys/class/tty/<tty>/device` points into the USB interface the port belongs
# to (an ACM port) or at the port of a USB-serial bridge below it (a CH340's
# ttyUSB). The **first** directory above it with an `idVendor` is the USB device
# itself; its `serial`, if it has one, is the answer. The walk stops there and
# never climbs into a hub: hubs have `idVendor` too, and a root hub even has a
# `serial` (its PCI address), which would name the wrong thing entirely.
#
# An Espressif chip's own USB-Serial/JTAG uses the MAC as its serial number; a
# CH343 has a serial of its own; a CH340 has none, and gets nothing here.
board_serial_of_port() {
    local tty device
    tty="$(basename -- "$(readlink -f -- "$1" 2>/dev/null)")"
    [[ -n "$tty" && -e "/sys/class/tty/$tty/device" ]] || return 0
    device="$(readlink -f -- "/sys/class/tty/$tty/device")"
    while [[ "$device" == /sys/devices/* ]]; do
        if [[ -f "$device/idVendor" ]]; then
            if [[ -f "$device/serial" ]]; then
                tr -d '[:space:]' <"$device/serial"
                echo
            fi
            return 0
        fi
        device="$(dirname -- "$device")"
    done
}

# The identity column of the entry whose `serial=` is `$1`, or nothing.
#
# For a board reached through a USB bridge with a serial number of its own -- a
# CH343 console -- whose USB serial is not its MAC.
board_of_serial() {
    local serial="${1^^}" list
    [[ -n "$serial" ]] || return 0
    list="$(devices_list)" || return 0
    awk -v serial="$serial" '
        /^[[:space:]]*(#|$)/ { next }
        {
            for (i = 3; i <= NF; i++) {
                if ($i ~ /^#/) break
                if (toupper($i) == "SERIAL=" serial) { print toupper($2); exit }
            }
        }
    ' "$list"
}

# The identity of the board behind a port -- its MAC, or the identity of the
# entry its bridge's serial names -- or nothing when it cannot be told.
board_of_port() {
    local serial
    serial="$(board_serial_of_port "$1")"
    [[ -n "$serial" ]] || return 0
    if [[ "$serial" =~ ^([0-9A-Fa-f]{2}:){5}[0-9A-Fa-f]{2}$ ]]; then
        printf '%s\n' "${serial^^}"
        return 0
    fi
    board_of_serial "$serial"
}

# The stable path of a board's port, found by its identity, or nothing.
#
# The `/dev/serial/by-id` name, which follows the board across replugs where a
# ttyACM number does not. Found by asking each by-id link which board is behind
# it, so a CH343 console with a `serial=` entry is found as surely as an
# Espressif chip's own port.
board_port_of() {
    local want="${1^^}" link
    [[ -n "$want" ]] || return 0
    for link in /dev/serial/by-id/*; do
        [[ -e "$link" ]] || continue
        if [[ "$(board_of_port "$link")" == "$want" ]]; then
            printf '%s\n' "$link"
            return 0
        fi
    done
}

# Why a script here must not open this port -- printed, with success -- or
# nothing and failure when it may. Asked before the port is opened, every time.
#
# A port whose board cannot be told -- no such port, no USB serial behind it,
# a serial no entry claims -- is refused: an unknown device on a shared bench
# is somebody's, and opening it restarts it.
board_port_refusal() {
    local port="$1" identity serial
    if [[ -z "$port" ]]; then
        echo "no port given"
        return 0
    fi
    if [[ ! -e "$port" ]]; then
        echo "$port does not exist now -- refusing rather than waiting for whatever appears there"
        return 0
    fi
    identity="$(board_of_port "$port")"
    if [[ -z "$identity" ]]; then
        serial="$(board_serial_of_port "$port")"
        if [[ -n "$serial" ]]; then
            echo "$port is a USB device whose serial no entry on this project's devices.list claims -- not one of its boards (if it is, add serial=<its serial> to that board's entry)"
        else
            echo "$port has no USB serial behind it, so this cannot tell which board it is"
        fi
        return 0
    fi
    board_refusal "$identity"
}

# True when a script here must not open this port. See `board_port_refusal`.
board_port_forbidden() {
    board_port_refusal "$1" >/dev/null
}

# Why the port is refused, for a message. Prints nothing for one that is not.
board_port_forbidden_why() {
    board_port_refusal "$1" || true
}

# Explain how to make one, when a script needs the list and the project has none.
board_list_missing() {
    cat >&2 <<'EOF'
No devices.list for this project, so this cannot tell one board from another.

Every script that can write to or power-cycle a device looks the board up there
first, and refuses any board that is not on it: a project works with its own
boards only. Make the project's own, at its root, from garden's template, and
list that project's boards in it:

    cp garden/devices.list.example devices.list
    $EDITOR devices.list

Find the MACs with:

    tools/power.sh list
EOF
}
