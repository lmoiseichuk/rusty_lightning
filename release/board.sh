# lightning project, Copyright (c) 2026 Leonid Moiseichuk under MIT License

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
# * `BOARD_ABSENT_HINT`, what this board's absence usually means, which
#   garden's `board_port` prints after it says the board is not on the bus;
# * which garden to load -- the submodule in lightning's checkout, the copy in
#   a release tree -- and `board_garden_tools`, which garden that was, so that
#   a Python tool asks garden's `board.py` and `portlock.py` from the same one.
#
# **What a script puts together before it opens a port is garden's too**:
# `board_garden_allows`, `board_wanted`, `board_port`, `board_port_stable` and
# `board_is` were written here and moved into garden's `board.sh`, where every
# project's scripts share them, along with `board_lock_path`, the lock every
# tool takes before it opens a board's port, and `board_flash_guard`, what a
# flash script asks immediately before it writes. Their exit statuses are the
# ones this file's scripts have always read: 0 go ahead, the identity or path
# on stdout; 1 absent, or another of lightning's boards; 2 refused.
#
# It refuses to load at all -- the `.` fails, and every caller stops on that --
# when garden's `board.sh` cannot be found, or lacks a function used here.
#
# Sourced by `flash.sh` and `tools/recover.sh`, and run by `tools/send.py`,
# `tools/watch.py` and `tools/portlock.py`; as the copy `tools/release.sh` puts
# in `release/`, by `release/flash.sh`, `release/send.py` and
# `release/portlock.py`.
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

# What this board's absence usually means, printed by garden's `board_port`
# after it says the board is not on the bus. Set before garden's file is
# sourced, which only reads it.
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
        board_port_of board_port_refusal board_list_missing board_garden_allows \
        board_wanted board_port board_port_stable board_is board_lock_path \
        board_on_socket board_flash_guard >/dev/null; then
    echo "garden's tools/board.sh ($LIGHTNING_GARDEN_TOOLS) lacks a function lightning needs -- refusing." >&2
    echo "In a checkout:  git submodule update --init" >&2
    return 1
fi

# The `tools/` directory of the garden loaded above. `tools/portlock.py` loads
# garden's `portlock.py` from there, and that loads the `board.py` beside it,
# so the Python checks and the shell ones read the same list.
board_garden_tools() {
    printf '%s\n' "$LIGHTNING_GARDEN_TOOLS"
}
