# lightning's boards, and the rules for touching one -- garden's `tools/board.sh`,
# from this repository's `garden` submodule, reading lightning's own
# `devices.list` at the repository's root.
#
# **Each project works with its own boards.** A board that is not on lightning's
# list is refused, and so is one the list flags `forbidden`, and so is a port
# whose board cannot be told. Those rules, the lookups, and which board is
# behind a port -- read from sysfs, never by opening the port -- are garden's:
# see `board_refusal` and `board_port_refusal` there.
#
# What is lightning's own, and so stays here:
#
# * `BOARD_ABSENT_HINT`, what this board's absence usually means;
# * `board_garden_allows`, which reads a garden refusal function's answer so
#   that only an explicit "allowed" is allowed -- never a check that failed;
# * four short functions that put garden's together the way lightning's
#   scripts use them, and say what happened in lightning's words:
#   `board_wanted`, `board_port`, `board_port_stable` and `board_is`;
# * `board_garden_tools`, which garden this file loaded, so that a Python tool
#   asks garden's `board.py` from the same one.
#
# It refuses to load at all -- the `.` fails, and every caller stops on that --
# when garden's `board.sh` cannot be found, or lacks a function used here.
#
# Sourced by `flash.sh` and `tools/recover.sh`, and run by `tools/send.py` and
# `tools/watch.py`; as the copy `tools/release.sh` puts in `release/`, by
# `release/flash.sh` and `release/send.py`.
#
# Which board is meant, first match wins (garden's `board_mac`):
#
#   1. `$BOARD_MAC` in the environment    -- for a one-off; still has to be on
#                                            the list
#   2. the entry named `$BOARD`
#   3. the list's first entry
#
# **Naming the boards rather than storing one MAC** is what makes "it is *that*
# board" a sentence the scripts can print.

# What this board's absence usually means, printed by `board_port` after it says
# the board is not on the bus.
BOARD_ABSENT_HINT="If it is plugged in and missing, check the power switch: in battery
mode with no cell fitted, USB does not reach the rail and the board
never enumerates. Light sleep also takes the port away in windows."

# --- garden's board.sh ------------------------------------------------------
#
# **In lightning's own checkout, its submodule; anywhere else, the copy beside
# this file.** `tools/` and `release/` both sit beside the `garden` submodule,
# so in a checkout `../garden/tools/board.sh` reaches it from either, and that
# is the one used: it is the commit the checkout pins, so the guard moves when
# the pin does.
#
# A release tree also carries a copy, `garden/tools/` beside this file, which
# `tools/release.sh` puts there so that a tree copied away from the checkout
# still refuses the wrong board. It is laid out as the submodule is, so garden's
# `board.sh` finds the list the same way -- two levels above itself, which for
# the copy is the release tree's own root.
#
# **"In lightning's checkout" is asked of git, not guessed from what is
# nearby.** The parent directory counts as the checkout only when it is the top
# of a git work tree that tracks this very file -- `tools/board.sh` or
# `release/board.sh` -- so a release tree copied into some other project's root
# never takes that project's `garden/` and that project's `devices.list` for
# its own, as it would if a `../garden/tools/board.sh` were enough. `-ef`
# compares the two directories themselves, so a path reached through a
# symbolic link still matches. A git that cannot answer -- not installed, or
# refusing a repository owned by another user -- reads as "not a checkout",
# which fails closed in a checkout too: `tools/` has no copy, so this refuses
# to load, and `release/`'s copy looks for a list in `release/`, where a
# checkout has none, so every board is refused.
#
# **A checkout without its submodule is refused, not served the copy**: the
# copy can be behind the pin, and the checkout's list is not beside it.
#
# **A missing garden fails the source**, rather than leaving a script with no
# guard: `return` in a sourced file ends the `.` command with that status, and
# every caller stops on it.
lightning_board_here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
lightning_board_root="$(dirname "$lightning_board_here")"
lightning_board_self="$(basename "$lightning_board_here")/board.sh"
# `|| ...=""`: outside any repository git fails, and a caller under `set -e`
# that sources this bare -- not as `. board.sh || exit 1`, which suspends
# `set -e` for the whole file -- would end here on the failed assignment,
# silently, with status 128.
lightning_board_top="$(git -C "$lightning_board_root" rev-parse --show-toplevel 2>/dev/null)" \
    || lightning_board_top=""
if [[ -n "$lightning_board_top" && "$lightning_board_top" -ef "$lightning_board_root" ]] \
        && git -C "$lightning_board_root" ls-files --error-unmatch -- "$lightning_board_self" >/dev/null 2>&1; then
    if [[ -f "$lightning_board_root/garden/tools/board.sh" ]]; then
        LIGHTNING_GARDEN_TOOLS="$lightning_board_root/garden/tools"
    else
        echo "garden's submodule is not checked out, so no board can be checked." >&2
        echo "Fetch it:  git submodule update --init" >&2
        unset lightning_board_here lightning_board_root lightning_board_self lightning_board_top
        return 1
    fi
elif [[ -f "$lightning_board_here/garden/tools/board.sh" ]]; then
    LIGHTNING_GARDEN_TOOLS="$lightning_board_here/garden/tools"
else
    echo "garden's tools/board.sh is missing, so no board can be checked." >&2
    echo "In a checkout, fetch the submodule:  git submodule update --init" >&2
    unset lightning_board_here lightning_board_root lightning_board_self lightning_board_top
    return 1
fi
unset lightning_board_here lightning_board_root lightning_board_self lightning_board_top
# shellcheck source=../garden/tools/board.sh
. "$LIGHTNING_GARDEN_TOOLS/board.sh" || return 1

# **A guard that is missing must not read as a guard that passed.** Garden's
# refusal functions succeed when they refuse, so a caller's `if board_refusal
# ...` would take "command not found" -- a garden too old or too new for this
# file -- as permission. Every garden function lightning's scripts call has to
# exist, or nothing is sourced at all. `declare -F` with several names fails if
# any one of them is not a defined function.
if ! declare -F devices_list board_mac board_name_of board_refusal board_of_port \
        board_port_of board_port_refusal board_list_missing >/dev/null; then
    echo "garden's tools/board.sh ($LIGHTNING_GARDEN_TOOLS) lacks a function lightning needs -- refusing." >&2
    echo "In a checkout:  git submodule update --init" >&2
    return 1
fi

# The `tools/` directory of the garden loaded above. `tools/send.py` and
# `tools/watch.py` import garden's `board.py` from there, so the Python checks
# and the shell ones read the same list.
board_garden_tools() {
    printf '%s\n' "$LIGHTNING_GARDEN_TOOLS"
}

# --- lightning's use of it --------------------------------------------------
#
# **Exit status, the same in every function below:**
#
#   0   go ahead -- the identity or the path is printed on stdout
#   1   the board is one lightning may touch, but it is not on the bus now (for
#       `board_port`), or the port is another of lightning's boards than the
#       one meant (for `board_is`)
#   2   refused, or it cannot be told which board this is -- why is on stderr
#
# Callers print their own advice about an absent board for 1 only, so a refusal
# is never followed by "it is probably asleep" -- advice that, after a refusal,
# points the way round it.

# Whether garden's refusal function `$1` explicitly allows `$2`: success when
# it does, and nothing printed. Otherwise why not, printed, and failure.
#
# **Only an explicit "allowed" is allowed.** Garden's refusal functions --
# `board_refusal`, `board_port_refusal` -- print a reason and return 0 when
# they refuse, and print nothing and return 1 when they allow. Read as "0 means
# refused, anything else means allowed", a check that could not run would pass:
# 127 for a function that is not there, 1 for a shell that stopped on an unset
# variable. So the answer is read by its status, as garden's `board.py` reads
# its own: 1 with nothing printed, on either stream, is an allow; 0 is a
# refusal, with its reason; and any other status, or a 1 that printed
# something, is a check that failed, and refused as one.
#
# The polarity is the other way round from garden's on purpose: success here
# is the allow, so a caller's `if ! ...` refuses whenever this cannot run at
# all, too.
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

# The identity (the MAC) of the board meant, printed when lightning may touch
# it. Otherwise lightning's account of why not, and 2.
#
# **Asked before anything is built or looked for.** It needs no port, so
# `flash.sh` asks it before a build of several minutes rather than after.
board_wanted() {
    local identity refusal name
    if ! devices_list >/dev/null; then
        # Garden's message says how to make one; lightning adds where this copy
        # of its tools looks for it -- the checkout's root, or a release tree's.
        board_list_missing
        echo "For these tools it belongs at $(dirname "$(dirname "$LIGHTNING_GARDEN_TOOLS")")/devices.list." >&2
        return 2
    fi
    identity="$(board_mac)"
    if [[ -z "$identity" ]]; then
        # Garden's `board_mac` answers a name the list does not have with
        # nothing, and success, so the empty answer is the "not found".
        if [[ -n "${BOARD:-}" ]]; then
            echo "No board named '$BOARD' in lightning's devices.list." >&2
        else
            echo "lightning's devices.list has no boards in it." >&2
        fi
        echo "lightning's boards, and where each is:  tools/power.sh list" >&2
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

# The board's port -- its /dev/serial/by-id path -- found by its identity
# (garden's `board_port_of`), and printed. 0; 1 when it is not on the bus now;
# 2 when it is refused.
#
# **The by-id path, never a ttyACM number**: it is named after the board's USB
# serial, so it follows the board across replugs and can never come to mean a
# different one.
board_port() {
    local identity port
    identity="$(board_wanted)" || return 2
    port="$(board_port_of "$identity")"
    if [[ -z "$port" ]]; then
        echo "The board ($(board_name_of "$identity")) is not on /dev/serial/by-id." >&2
        printf '%s\n' "$BOARD_ABSENT_HINT" >&2
        return 1
    fi
    printf '%s\n' "$port"
}

# Whether lightning may open `$1` at all, whichever of its boards it is -- and
# if so, the board's by-id path, printed. 0, or 2. `$2` is what refusing
# protects.
#
# **Asked of sysfs, never of the port** (garden's `board_port_refusal`):
# opening the port is what restarts the board. Refused: a port that does not
# exist now, one with nothing behind it to tell which board it is (a CH340, or
# a bridge whose serial no entry claims), a board not on lightning's list, and
# one flagged `forbidden`.
#
# **The path printed is the one to use from then on.** `/dev/ttyACM<n>` is
# handed to whichever device enumerates next, and a script that checked it and
# then wrote to it minutes later -- after a build, across a replug -- wrote to
# whatever had the number by then. A port with no by-id name yet (udev not
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
        echo "⚠ $port is $(board_name_of "$identity"), but it has no /dev/serial/by-id name yet --" >&2
        echo "  refusing to go on with a path whose number can change hands. Try again in a moment." >&2
        return 2
    fi
    printf '%s\n' "$stable"
}

# Whether `$1` is the board meant, and one lightning may open -- and if so, its
# by-id path, printed. 0; 1 when it is another of lightning's boards; 2 when it
# is refused. `$2` is what refusing protects.
#
# `board_port_stable` and one comparison more: the board behind the port must
# be the one `board_wanted` names. A script's "skip the identity check" skips
# that comparison and nothing else, by calling `board_port_stable` instead.
board_is() {
    local port="$1" what="${2:-flashing the wrong board replaces its firmware}"
    local identity stable found
    identity="$(board_wanted)" || { echo "Refusing: $what" >&2; return 2; }
    stable="$(board_port_stable "$port" "$what")" || return 2
    found="$(board_of_port "$stable")"
    if [[ "$found" != "$identity" ]]; then
        echo "⚠ $port is not the configured board." >&2
        echo "  it is:     $(board_name_of "$found") ($found), $stable" >&2
        echo "  expected:  $(board_name_of "$identity") ($identity)" >&2
        echo "If it is the board you mean, name it:  BOARD=$(board_name_of "$found")" >&2
        echo "Refusing: $what" >&2
        return 1
    fi
    printf '%s\n' "$stable"
}
