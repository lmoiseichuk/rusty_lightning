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
# Format: `name  MAC  [hub  port]  [flags]  [serial=<usb serial>]
# [path=<by-path name>]  [# why]`.
#
# **Hub and port are the fallback that makes a missing board reachable.** While
# a board is plugged in, sysfs is the better answer -- it is live, so it follows
# a replug. But a board that has wedged, gone to sleep with its USB PHY down, or
# stopped enumerating for any other reason is *absent from sysfs entirely*, and
# that is exactly when its power needs cutting. Without a stored location there
# is nothing left to cut.

# The root of the project this garden is a submodule of: the directory two
# levels above this file's own, `moisture` for `moisture/garden/tools/board.sh`.
#
# `${BASH_SOURCE[0]}` inside a function is the file the function was defined
# in -- this one -- whichever script sourced it, so the answer does not depend
# on where the caller runs from. `cd ... && pwd` keeps the path as it was
# reached (a symbolic link on the way is not resolved; see below).
board_project_root() {
    local here
    here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
    dirname "$(dirname "$here")"
}

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
    local root
    root="$(board_project_root)"
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
# Flags are any bare words after the MAC that are not a hub/port pair, a
# `serial=` or a `path=`, so the columns stay optional without needing
# placeholders.
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
                if ($i ~ /^(serial|path)=/) continue
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

# Where those answers are read: sysfs, and the links udev makes to each port
# (`/dev/serial/by-id/` and `/dev/serial/by-path/`).
#
# **Set here, never taken from the environment.** These are plain assignments,
# run every time this file is sourced, so whatever a caller's environment held
# under these names is replaced -- nothing a shell inherits can send a check to
# some other tree. A test that sources this file sets them again afterwards, to
# a directory of its own laid out like sysfs and `/dev/serial`, so that a CH340
# or a CH343 can be imitated on a bench that has neither plugged in. `board.py`
# starts a fresh shell for every question, so it always reads the real ones.
BOARD_SYS=/sys
BOARD_SERIAL_LINKS=/dev/serial

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
    [[ -n "$tty" && -e "$BOARD_SYS/class/tty/$tty/device" ]] || return 0
    device="$(readlink -f -- "$BOARD_SYS/class/tty/$tty/device")"
    while [[ "$device" == "$BOARD_SYS"/devices/* ]]; do
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

# The `path=` of the entry with this identity -- a /dev/serial/by-path name,
# bare -- or nothing when the entry has none.
#
# Written in the list either bare, as `ls /dev/serial/by-path/` prints it, or
# as the whole path; the directory is taken off here, so both mean the same.
board_path_of() {
    local identity="${1^^}" list
    [[ -n "$identity" ]] || return 0
    list="$(devices_list)" || return 0
    awk -v identity="$identity" '
        /^[[:space:]]*(#|$)/ { next }
        toupper($2) != identity { next }
        {
            for (i = 3; i <= NF; i++) {
                if ($i ~ /^#/) break
                if ($i ~ /^path=/) {
                    value = substr($i, length("path=") + 1)
                    sub(/^\/dev\/serial\/by-path\//, "", value)
                    print value
                    exit
                }
            }
            exit
        }
    ' "$list"
}

# The identity column of the entry whose `path=` names a /dev/serial/by-path
# link to this port, or nothing.
#
# For a board behind a bridge with **no USB serial at all** -- a CH340 -- which
# sysfs cannot name. udev's by-path link is named after the USB socket the
# bridge is plugged into, so the entry says "whatever sits in this socket is
# that board".
#
# **A by-path name identifies the socket, not the board.** Another board with
# a serial-less bridge, plugged into the same socket, is taken for this one.
# So this is only ever asked for a device that has no serial: a board with a
# serial of its own -- an Espressif chip, a CH343 -- is named by that serial
# alone, wherever it is plugged in, and is never taken for a `path=` entry's
# board. And a flash script must still confirm the board itself before it
# writes, from the MAC espflash prints on connect: see `board_confirm_mac`.
board_of_path_port() {
    local device link identity
    device="$(readlink -f -- "$1" 2>/dev/null)"
    [[ -n "$device" ]] || return 0
    for link in "$BOARD_SERIAL_LINKS"/by-path/*; do
        [[ -e "$link" ]] || continue
        [[ "$(readlink -f -- "$link")" == "$device" ]] || continue
        identity="$(board_of_path_name "$(basename -- "$link")")"
        if [[ -n "$identity" ]]; then
            printf '%s\n' "$identity"
            return 0
        fi
    done
}

# The identity column of the entry whose `path=` is this by-path name, or
# nothing. Compared exactly: a by-path name is a file name, and its case is
# what udev wrote.
board_of_path_name() {
    local name="$1" list
    [[ -n "$name" ]] || return 0
    list="$(devices_list)" || return 0
    awk -v name="$name" '
        /^[[:space:]]*(#|$)/ { next }
        {
            for (i = 3; i <= NF; i++) {
                if ($i ~ /^#/) break
                if ($i ~ /^path=/) {
                    value = substr($i, length("path=") + 1)
                    sub(/^\/dev\/serial\/by-path\//, "", value)
                    if (value == name) { print toupper($2); exit }
                }
            }
        }
    ' "$list"
}

# The identity of the board behind a port -- its MAC, or the identity of the
# entry its bridge's serial names, or for a bridge with no serial at all the
# entry whose `path=` names its socket -- or nothing when it cannot be told.
#
# **A serial, when there is one, decides alone.** The `path=` lookup is reached
# only for a device with no serial: a board that has one is never taken for a
# `path=` entry's board merely for sitting in its socket.
board_of_port() {
    local serial
    serial="$(board_serial_of_port "$1")"
    if [[ -z "$serial" ]]; then
        board_of_path_port "$1"
        return 0
    fi
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
#
# **For a `path=` entry, its by-path link instead**, and only that: a CH340's
# by-id name carries no serial, so every CH340 on the bus wants the same one
# and udev gives it to whichever enumerated last. The by-path link is checked
# the same way -- the board behind it must be this one -- so a board with a
# serial of its own sitting in that socket is not returned.
board_port_of() {
    local want="${1^^}" link path
    [[ -n "$want" ]] || return 0
    path="$(board_path_of "$want")"
    if [[ -n "$path" ]]; then
        link="$BOARD_SERIAL_LINKS/by-path/$path"
        if [[ -e "$link" && "$(board_of_port "$link")" == "$want" ]]; then
            printf '%s\n' "$link"
        fi
        return 0
    fi
    for link in "$BOARD_SERIAL_LINKS"/by-id/*; do
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
# A port whose board cannot be told -- no such port, no USB serial behind it
# and no `path=` entry naming its socket, a serial no entry claims -- is
# refused: an unknown device on a shared bench is somebody's, and opening it
# restarts it.
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
            echo "$port has no USB serial behind it, so this cannot tell which board it is (a bridge with no serial at all, such as a CH340: give its board's entry path=<its /dev/serial/by-path name>)"
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
    # Where this copy of the tools looks for it, which the text above can only
    # describe: the root two levels above this file.
    echo "For these tools it belongs at $(board_project_root)/devices.list." >&2
}

# --- what a project's scripts call -------------------------------------------
#
# The functions above answer one question each. These put them together the
# way a flash or console script uses them, and say what happened -- grown in
# lightning's tools, where every script needed the same five, and general here
# so that each project's wrapper keeps only its own words.
#
# **Exit status, the same in every function below:**
#
#   0   go ahead -- the identity or the path is printed on stdout
#   1   the board is one the project may touch, but it is not on the bus now
#       (for `board_port`), or the port is another of the project's boards
#       than the one meant (for `board_is`)
#   2   refused, or it cannot be told which board this is -- why is on stderr
#
# Callers print their own advice about an absent board for 1 only, so a
# refusal is never followed by "it is probably asleep" -- advice that, after a
# refusal, points the way round it.
#
# **`BOARD_ABSENT_HINT`**, when a project's wrapper sets it, is printed after
# "not on the bus": what that project's board being missing usually means (a
# power switch, a sleep that takes the port away).

# Whether garden's refusal function `$1` explicitly allows `$2`: success when
# it does, and nothing printed. Otherwise why not, printed, and failure.
#
# **Only an explicit "allowed" is allowed.** The refusal functions --
# `board_refusal`, `board_port_refusal` -- print a reason and return 0 when
# they refuse, and print nothing and return 1 when they allow. Read as "0 means
# refused, anything else means allowed", a check that could not run would pass:
# 127 for a function that is not there, 1 for a shell that stopped on an unset
# variable. So the answer is read by its status, as `board.py` reads its own: 1
# with nothing printed, on either stream, is an allow; 0 is a refusal, with its
# reason; and any other status, or a 1 that printed something, is a check that
# failed, and refused as one.
#
# The polarity is the other way round from the refusal functions' on purpose:
# success here is the allow, so a caller's `if ! ...` refuses whenever this
# cannot run at all, too.
board_garden_allows() {
    local function="$1" argument="$2" said status=0
    # `2>&1` so that anything the check wrote to stderr is part of what it said:
    # an allow says nothing at all.
    said="$("$function" "$argument" 2>&1)" || status=$?
    case "$status" in
        1)
            if [[ -z "$said" ]]; then
                return 0
            fi
            printf '%s\n' "the check failed ($function said something while allowing: $said)"
            ;;
        0)
            printf '%s\n' "${said:-refused, with no reason given}"
            ;;
        *)
            printf '%s\n' "the check failed ($function, status $status)${said:+: $said}"
            ;;
    esac
    return 1
}

# The identity of the board meant, printed when the project may touch it.
# Otherwise why not, and 2.
#
# `$1` names it -- by name or by its identity column. With nothing given, the
# board is `board_mac`'s choice: `$BOARD_MAC`, then the entry named `$BOARD`,
# then the list's first.
#
# **Asked before anything is built or looked for.** It needs no port, so a
# flash script asks it before a build of several minutes rather than after.
board_wanted() {
    local asked="${1:-}" identity refusal name
    if ! devices_list >/dev/null; then
        board_list_missing
        return 2
    fi
    identity="$(board_mac "$asked")"
    # `board_mac` reads the name column only; an identity typed instead -- a
    # MAC copied from `power.sh list` -- is the entry whose identity it is.
    if [[ -z "$identity" && -n "$asked" && -n "$(board_name_of "$asked")" ]]; then
        identity="${asked^^}"
    fi
    if [[ -z "$identity" ]]; then
        # `board_mac` answers a name the list does not have with nothing, and
        # success, so the empty answer is the "not found".
        if [[ -n "$asked" || -n "${BOARD:-}" ]]; then
            echo "No board named '${asked:-$BOARD}' in this project's devices.list." >&2
        else
            echo "This project's devices.list has no boards in it." >&2
        fi
        echo "The project's boards, and where each is:  tools/power.sh list" >&2
        return 2
    fi
    # Refused when it is not on the list (a `$BOARD_MAC` typed by hand can be
    # anything), when it is `forbidden`, and when the check cannot say.
    if ! refusal="$(board_garden_allows board_refusal "$identity")"; then
        name="$(board_name_of "$identity")"
        echo "Refusing ${name:-$identity}: $refusal" >&2
        return 2
    fi
    printf '%s\n' "$identity"
}

# The port of the board meant -- its stable path -- printed. 0; 1 when it is
# not on the bus now; 2 when it is refused. `$1` names the board, as for
# `board_wanted`.
#
# **The stable path, never a ttyACM number**: the by-id name follows the
# board's own USB serial across replugs and can never come to mean a different
# one. For a `path=` entry it is the by-path name, which follows the socket.
board_port() {
    local identity port
    identity="$(board_wanted "${1:-}")" || return 2
    port="$(board_port_of "$identity")"
    if [[ -z "$port" ]]; then
        echo "The board ($(board_name_of "$identity")) is not on the bus now." >&2
        if [[ -n "${BOARD_ABSENT_HINT:-}" ]]; then
            printf '%s\n' "$BOARD_ABSENT_HINT" >&2
        fi
        return 1
    fi
    printf '%s\n' "$port"
}

# Whether the project may open `$1` at all, whichever of its boards it is --
# and if so, the board's stable path, printed. 0, or 2. `$2` is what refusing
# protects, for the message.
#
# **Asked of sysfs, never of the port** (`board_port_refusal`): opening the
# port is what restarts the board. Refused: a port that does not exist now, one
# with nothing behind it to tell which board it is, a board not on the
# project's list, and one flagged `forbidden`.
#
# **The path printed is the one to use from then on.** `/dev/ttyACM<n>` is
# handed to whichever device enumerates next, and a script that checked it and
# then wrote to it minutes later -- after a build, across a replug -- wrote to
# whatever had the number by then. A port with no stable name yet (udev not
# done with it) is refused, for the same reason.
board_port_stable() {
    local port="$1" what="${2:-flashing the wrong board replaces its firmware}"
    local refusal identity stable
    if ! refusal="$(board_garden_allows board_port_refusal "$port")"; then
        echo "⚠ Not opening ${port:-a port}: $refusal" >&2
        echo "Refusing: $what" >&2
        return 2
    fi
    identity="$(board_of_port "$port")"
    stable="$(board_port_of "$identity")"
    if [[ -z "$stable" ]]; then
        echo "⚠ $port is $(board_name_of "$identity"), but it has no stable name under $BOARD_SERIAL_LINKS yet --" >&2
        echo "  refusing to go on with a path whose number can change hands. Try again in a moment." >&2
        return 2
    fi
    printf '%s\n' "$stable"
}

# Whether `$1` is the board meant, and one the project may open -- and if so,
# its stable path, printed. 0; 1 when it is another of the project's boards; 2
# when it is refused. `$2` is what refusing protects; `$3` names the board
# meant, as for `board_wanted`.
#
# `board_port_stable` and one comparison more: the board behind the port must
# be the one `board_wanted` names. A script's "skip the identity check" skips
# that comparison and nothing else, by calling `board_port_stable` instead.
board_is() {
    local port="$1" what="${2:-flashing the wrong board replaces its firmware}" asked="${3:-}"
    local identity stable found
    identity="$(board_wanted "$asked")" || { echo "Refusing: $what" >&2; return 2; }
    stable="$(board_port_stable "$port" "$what")" || return 2
    found="$(board_of_port "$stable")"
    if [[ "$found" != "$identity" ]]; then
        echo "⚠ $port is not the board meant." >&2
        echo "  it is:     $(board_name_of "$found") ($found), $stable" >&2
        echo "  expected:  $(board_name_of "$identity") ($identity)" >&2
        echo "If it is the board you mean, name it:  BOARD=$(board_name_of "$found")" >&2
        echo "Refusing: $what" >&2
        return 1
    fi
    printf '%s\n' "$stable"
}

# The lock file for the board with this identity, printed; nothing and failure
# for no identity.
#
# **One lock per board, for every project's tools.** Two tools on one port do
# not queue, they split the bytes between them, and a flash under a running
# monitor fails in ways that look like a bad board. So every tool takes this
# lock before it opens a port (`tools/portlock.py` for Python, `flock` on this
# path for a shell script), and a second one is refused.
#
# **Keyed by the board, not by the ttyACM name.** A ttyACM number changes hands
# when a board re-enumerates -- after a sleep, a replug, a flash -- so a lock
# named after it can be taken by a second tool on the same board under its new
# number. The identity is the board's own, so the lock follows it, as its by-id
# path does; and two projects that both list a board meet at one lock.
#
# **The identity is hashed into the name, for a name of one fixed form**: a
# MAC's colons and a small sensor's die id both become 16 hex digits, which
# every file system takes, and upper and lower case give the same name.
# `sha256sum` reads the identity on stdin; its first 16 hex digits are plenty
# to tell a bench's boards apart.
#
# **The hash hides nothing.** A MAC has 48 bits and the maker's prefix gives
# away 24 of them, so the rest can be found by trying every value, in well
# under a second. A lock path is a name on this bench, not something to publish.
#
# **A digest that did not come out is a failure**, not a shorter name: an empty
# one would give every board the same lock, `/tmp/garden-board-.lock`.
#
# **Always `/tmp`, never `$TMPDIR`.** A lock works only if every tool finds the
# same file, and `$TMPDIR` differs between a terminal, a cron job and an
# editor's session; a lock in two places is no lock.
board_lock_path() {
    local identity="${1^^}" digest
    [[ -n "$identity" ]] || return 1
    digest="$(printf '%s' "$identity" | sha256sum)" || return 1
    digest="${digest:0:16}"
    # `=~` matches a regular expression; the anchors make it the whole word.
    [[ "$digest" =~ ^[0-9a-f]{16}$ ]] || return 1
    printf '/tmp/garden-board-%s.lock\n' "$digest"
}

# Whether the board behind `$1` is known only by the socket it sits in -- a
# `path=` entry -- so that only the chip itself can say which board it is.
# Success when it is; failure for a board with a serial of its own, and for a
# port whose board cannot be told at all (which every check refuses anyway).
# A question, not one of the go-ahead functions above: 0 or 1, nothing printed.
#
# A flash script asks this, once it holds the board's lock, to know whether to
# read the chip's MAC (`espflash board-info`) before `board_flash_guard`, which
# refuses a socket's board without it.
board_on_socket() {
    local identity
    identity="$(board_of_port "${1:-}")"
    [[ -n "$identity" && -n "$(board_path_of "$identity")" ]]
}

# What a flash script calls immediately before it writes: the stable path of
# the port to flash, printed, with the board it is said on stderr. 0, or 2 and
# why not. `$1` is the port; `$2` is the chip espflash is told to expect; `$3`
# is what `espflash board-info` printed for that port, which a board on a
# `path=` socket must have and any other board may have, or ""; `$4` names
# the board the image is for, by name or identity, and must be given.
#
# **Nothing opens the port before it is checked and locked.** `espflash
# board-info` opens it, and opening it resets the chip: run on a port nobody
# checked, it resets whatever is there -- another project's board, a live
# device -- before any refusal; run before the lock, it resets a board a
# console holds. So the order is: the port, asked of sysfs and compared with
# the board meant (`board_is`); the board's lock; only then `board-info`, for
# a board on a socket; then this, immediately before the write:
#
#     board=display                                    # the board this image is for
#     socket="$(board_is "$PORT" "flashing the wrong board replaces its firmware" "$board")" || exit 1
#     lock="$(board_lock_path "$(board_of_port "$socket")")" || exit 1
#     exec 9>>"$lock" || exit 1
#     flock -n 9 || { echo "$socket is held by another tool -- refusing" >&2; exit 1; }
#     info=""
#     if board_on_socket "$socket"; then
#         info="$(espflash board-info --chip esp32 --port "$socket" 2>&1)" || exit 1
#     fi
#     port="$(board_flash_guard "$socket" esp32 "$info" "$board")" || exit 1
#     espflash flash --chip esp32 --port "$port" ...
#
# **Every status is checked, the lock path's too.** `board_lock_path` fails,
# printing nothing, for a port whose board has gone; bash does not stop on
# the `exec 9>>""` that would follow, and a script that inherited a file
# descriptor 9 from its parent would then take the parent's lock and flash
# with none of the board's. `|| exit 1` on each line is what stops it.
#
# * **The project's refusal, asked of sysfs** (`board_port_stable`): a port
#   that does not exist, one whose board cannot be told, a board not on the
#   project's list, one flagged `forbidden`.
# * **The stable path, which espflash is then given** instead of whatever was
#   typed, so the write goes to the board that was checked even if a ttyACM
#   number changes hands in between.
# * **The chip, which must be named.** espflash with `--chip` refuses, before
#   it writes anything, a chip that is not the one named; without it, it
#   writes whatever it finds. A board's USB descriptor does not say which chip
#   it is -- every Espressif chip's own port enumerates alike -- so this check
#   cannot be made here, only required: the chip has to be an `esp32...` word.
# * **For a board on a `path=` socket, the chip's own MAC**, confirmed against
#   the entry by `board_confirm_mac`. The socket says where a port is plugged
#   in, not what is there: any serial-less board in that socket is taken for
#   the entry's. So without `$3`, or with a `$3` whose MAC is not the entry's,
#   the guard refuses. `espflash board-info` reads the MAC and writes nothing;
#   it opens the port, which the sysfs check has already allowed. Given for any
#   other board, `$3` is confirmed the same way.
# * **The board the image is for**, `$4`: the board behind the port must be
#   that one. The project may open any of its boards, and one of them can sit
#   where another is expected -- a board with a serial of its own in the
#   socket a `path=` entry names is taken for itself, as it should be, and
#   without this the guard would answer "flashing <it>" and hand espflash
#   another board's image. Only `--chip` would then stop the write, and only
#   when the two boards are different chips.
# * **Which board it is**, by name, so a person watching sees it before the
#   write starts.
#
# **The lock is the caller's to take**, on `board_lock_path`'s file, because a
# lock lives on an open file: one taken inside this function would be taken in
# the subshell `$(...)` runs it in, and gone when it returned.
board_flash_guard() {
    local port="$1" chip="${2:-}" info="${3:-}" meant="${4:-}" stable identity name wanted
    if [[ ! "$chip" =~ ^esp32[a-z0-9]*$ ]]; then
        echo "⚠ No chip, or not an Espressif one: '${chip}'. Name it (esp32c3, esp32c6, ...) -- espflash told the chip refuses any other before writing." >&2
        return 2
    fi
    # **The board must be named**, as for `board_confirm_mac`: with no name,
    # `board_wanted` would fall back to `$BOARD_MAC`, `$BOARD` or the list's
    # first entry, and the guard would compare the port with a board nobody
    # meant.
    if [[ -z "$meant" ]]; then
        echo "⚠ board_flash_guard was not told which board the image is for -- name it, the fourth argument, so that another of the project's boards on this port is refused." >&2
        return 2
    fi
    wanted="$(board_wanted "$meant")" || return 2
    stable="$(board_port_stable "$port" "flashing the wrong board replaces its firmware")" || return 2
    identity="$(board_of_port "$stable")"
    name="$(board_name_of "$identity")"
    if [[ -z "$identity" || -z "$name" ]]; then
        echo "⚠ $port was allowed, but its board now has no name on the list -- refusing." >&2
        return 2
    fi
    if [[ "$identity" != "$wanted" ]]; then
        echo "⚠ $stable is $name, not $(board_name_of "$wanted"), the board this image is for." >&2
        echo "Refusing: flashing the wrong board replaces its firmware." >&2
        return 2
    fi
    if [[ -n "$(board_path_of "$identity")" && -z "$info" ]]; then
        echo "⚠ $stable names a socket, not a board: any board without a USB serial of its own" >&2
        echo "  that sits in it is taken for $name. Read the chip's MAC first and pass what espflash printed:" >&2
        echo "      info=\"\$(espflash board-info --chip $chip --port $stable 2>&1)\"    # with the board's lock held" >&2
        echo "      board_flash_guard \"$port\" $chip \"\$info\" $name" >&2
        echo "Refusing: flashing the wrong board replaces its firmware." >&2
        return 2
    fi
    if [[ -n "$info" ]]; then
        # The name, not the identity: board_confirm_mac says which board it
        # expected by name in its refusal.
        board_confirm_mac "$name" "$info" >/dev/null || return 2
    fi
    echo "flashing $name, as an $chip, at $stable" >&2
    printf '%s\n' "$stable"
}

# Whether the MAC in `$2` -- the text espflash printed on connect, all of it or
# the one line -- is the one the entry `$1` names. 0 when it is; 2 and why not.
# `$1` is a name or an identity, as for `board_wanted`, and must be given.
#
# **The positive identification of a board on a `path=` socket.** The by-path
# name says where a port is plugged in, not what is there; the MAC comes from
# the chip itself. `espflash board-info --port <by-path>` prints it and writes
# nothing, so a flash script runs that first and checks it here --
# `board_flash_guard` does, given its output -- and only then writes. But
# board-info opens the port, which resets the chip, so the port is checked and
# the board's lock taken before it runs (see `board_flash_guard`):
#
#     socket="$(board_is "$PORT" "flashing the wrong board replaces its firmware" display)" || exit 1
#     lock="$(board_lock_path "$(board_of_port "$socket")")" || exit 1
#     exec 9>>"$lock" || exit 1
#     flock -n 9 || { echo "$socket is held by another tool -- refusing" >&2; exit 1; }
#     info="$(espflash board-info --chip esp32 --port "$socket" 2>&1)" || exit 1
#     board_confirm_mac display "$info" || exit 1
#
# **The board must be named.** With no name, `board_wanted` would fall back to
# `$BOARD_MAC`, `$BOARD` or the list's first entry, and a script whose name
# variable was empty would confirm the chip against some other board than the
# one it guarded.
#
# Every MAC-shaped string in the text is read; exactly one distinct MAC must be
# there, and it must be the entry's. None, two different ones, or another
# board's is a refusal.
board_confirm_mac() {
    local identity printed count other
    if [[ -z "${1:-}" ]]; then
        echo "⚠ board_confirm_mac was given no board to confirm -- refusing rather than guessing which one." >&2
        return 2
    fi
    identity="$(board_wanted "$1")" || return 2
    # `grep -o` prints each match on a line of its own; `sort -u` keeps one of
    # each. `|| true` because grep fails when nothing matches, and under a
    # caller's `set -e -o pipefail` that failure would end the script here,
    # silently, instead of at the refusal below that says why.
    printed="$(grep -oE '([0-9A-Fa-f]{2}:){5}[0-9A-Fa-f]{2}' <<<"${2:-}" | tr '[:lower:]' '[:upper:]' | sort -u || true)"
    count=0
    if [[ -n "$printed" ]]; then
        count="$(wc -l <<<"$printed")"
    fi
    if [[ "$count" -ne 1 ]]; then
        echo "⚠ Expected one MAC in what espflash printed, found $count -- cannot confirm $(board_name_of "$identity"); refusing." >&2
        return 2
    fi
    if [[ "$printed" != "$identity" ]]; then
        other="$(board_name_of "$printed")"
        if [[ -z "$other" ]]; then
            other="a board not on this project's list"
        fi
        echo "⚠ The chip on this port is not $(board_name_of "$identity"): it reports the MAC of $other." >&2
        return 2
    fi
    printf '%s\n' "$identity"
}
