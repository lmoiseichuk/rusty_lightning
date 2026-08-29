# Which board this checkout flashes, and whether a given port really is it.
#
# **The MAC is not in the scripts.** It is one device's identity, and the
# scripts are the part worth reusing — anyone cloning this repository has a
# different board, and should not have to edit three files to say so.
#
# Sourced by `flash.sh`, `tools/recover.sh` and `release/flash.sh`, which is
# also what stopped the by-id resolver being written three times; it was, this
# morning, and three copies of a safety check is three places for one of them
# to be subtly weaker.
#
# Where the MAC comes from, first match wins:
#
#   1. `$BOARD_MAC` in the environment    -- for a one-off or for CI
#   2. the entry named `$BOARD` in `devices.list`, default `lightning`
#   3. the first entry in `devices.list`
#
# `devices.list` is gitignored and holds one `name  MAC` pair per line.
# `devices.list.example` shows the format. Naming the boards rather than
# storing one MAC is what makes "it resolves to *that* board" a sentence the
# scripts can print.

# The MAC this checkout targets, or empty if none is configured.
board_mac() {
    if [[ -n "${BOARD_MAC:-}" ]]; then
        printf '%s\n' "$BOARD_MAC"
        return 0
    fi
    local list want
    list="$(devices_list)" || return 1
    want="${BOARD:-lightning}"
    # The named entry, then the first -- so a one-board bench needs no $BOARD.
    awk -v want="$want" '
        /^[[:space:]]*(#|$)/ { next }
        $1 == want { print $2; found = 1; exit }
        !first { first = $2 }
        END { if (!found && first) print first }
    ' "$list"
}

# The devices file, or failure if there is none.
devices_list() {
    local here dir
    here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
    for dir in "$here" "$here/.." "$here/../.."; do
        if [[ -f "$dir/devices.list" ]]; then
            printf '%s\n' "$dir/devices.list"
            return 0
        fi
    done
    return 1
}

# Which name a MAC belongs to, for a message that says more than the hex.
board_name_of() {
    local mac="$1" list
    list="$(devices_list)" || return 1
    awk -v mac="$mac" '
        /^[[:space:]]*(#|$)/ { next }
        toupper($2) == toupper(mac) { print $1; exit }
    ' "$list"
}

# Explain how to configure one. Called when a script needs a MAC and has none.
board_mac_missing() {
    cat >&2 <<'EOF'
No board configured, so this cannot check which device it is talking to.

The by-id lookup and the identity guard both need the board's MAC. Create
devices.list in this checkout:

    cp devices.list.example devices.list
    $EDITOR devices.list

or name one for a single command:

    BOARD_MAC=AA:BB:CC:DD:EE:FF ./flash.sh
    BOARD=panel ./flash.sh          # an entry in devices.list

The MAC is on the by-id path of the port the board enumerates as:

    ls /dev/serial/by-id/
EOF
}

# Resolve any port -- /dev/ttyACM0 included -- back to its by-id name.
#
# **Both paths point at the same character device**, so comparing `readlink -f`
# is enough and needs no udev query. This exists because the guard it feeds used
# to check only paths that already began /dev/serial/by-id/, which meant a bare
# ttyACM path skipped it entirely -- and ttyACM numbering shuffles between
# plug-ins.
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

# Refuse unless `$1` is the configured board. `$2` is what refusing protects.
#
# Returns 0 when the port checks out, 1 when it does not or cannot be checked.
board_is() {
    local port="$1" what="${2:-flashing the wrong board replaces its firmware}"
    local mac resolved
    mac="$(board_mac)" || { board_mac_missing; return 1; }

    if resolved="$(resolve_by_id "$port")"; then
        if [[ "$resolved" != *"$mac"* ]]; then
            local other
            other="$(board_name_of "$(printf '%s' "$resolved" | grep -oiE '([0-9A-F]{2}:){5}[0-9A-F]{2}')" 2>/dev/null)"
            echo "⚠ $port is not the configured board." >&2
            echo "  it resolves to: $resolved" >&2
            [[ -n "$other" ]] && echo "  which is:       $other" >&2
            echo "  expected MAC:   $mac${BOARD:+ ($BOARD)}" >&2
            echo "Refusing: $what" >&2
            return 1
        fi
        return 0
    fi

    echo "⚠ $port has no /dev/serial/by-id entry, so this cannot check which" >&2
    echo "  board it is. Refusing rather than acting on an unknown device." >&2
    return 1
}

# The board's port, found by MAC. Prints it, or explains and fails.
board_port() {
    local mac found
    mac="$(board_mac)" || { board_mac_missing; return 1; }
    found="$(ls /dev/serial/by-id/ 2>/dev/null | grep -m1 "$mac" || true)"
    if [[ -z "$found" ]]; then
        echo "The board ($mac) is not on /dev/serial/by-id." >&2
        echo "If it is plugged in and missing, check the power switch: in battery" >&2
        echo "mode with no cell fitted, USB does not reach the rail and the board" >&2
        echo "never enumerates. Light sleep also takes the port away in windows." >&2
        return 1
    fi
    printf '/dev/serial/by-id/%s\n' "$found"
}
