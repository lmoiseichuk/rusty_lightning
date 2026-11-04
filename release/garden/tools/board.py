# garden project, Copyright (c) 2026 Leonid Moiseichuk under MIT License

"""Ask garden's board.sh whether a project may open a port or touch a board.

The answer comes from the project's own `devices.list`: board.sh, in the
project's `garden` submodule, finds it at the project's root.

**A bridge, not a second copy of the rule.** The rule -- which boards a project
may touch, and what to do with one it cannot identify -- lives in
`tools/board.sh`, beside this file. A Python tool that re-implemented it would
be a second copy, free to drift from the first, and the boards it protects
include a live lightning monitor. So this file runs the shell functions and
reads their answer; it decides nothing itself.

**Fails closed.** A port or board counts as allowed only when board.sh says
so explicitly. A missing board.sh, a shell that cannot start, an unexpected
exit status: each is a refusal, carrying what went wrong, because "the check
did not run" must never read as "the check passed".

From a project's Python tool, with garden as the project's submodule, load
this file by its path -- a plain `import board` could find another module of
that name first (Adafruit's Blinka installs one). **The path as it was
reached, never resolved**: `resolve()` would follow a symbolic link on the way
to garden, and board.sh would then read the link target's project's list (see
`BOARD_SH` below):

    import importlib.util, os, pathlib
    path = pathlib.Path(os.path.abspath(__file__)).parent.parent / "garden" / "tools" / "board.py"
    spec = importlib.util.spec_from_file_location("garden_board", path)
    board = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(board)

    refusal = board.port_refusal("/dev/serial/by-id/...")
    if refusal is not None:
        sys.exit(f"refusing: {refusal}")

From a shell, to see an answer by hand:

    python3 garden/tools/board.py <port>

**Lookups, and the functions a tool's script would call.** Besides the two
refusals, this runs board.sh's lookups -- which board is behind a port, its
name, its lock file -- and the functions a console or flash tool puts together
before it opens a port: `wanted`, `board_port`, `stable_port` and `board_is`.
Each runs board.sh's function of the same name and returns what it printed;
board.sh's messages, on stderr, go to the tool's own stderr unchanged.
"""

import os
import pathlib
import subprocess
import sys

# board.sh, beside this file in whichever checkout this file is in.
#
# **The path as the caller reached it, not resolved.** board.sh finds the
# project's list by trimming the path it was run by, so that a garden reached
# through a symbolic link still reads the project's own list. Resolving the link
# here would hand it the target's path -- and the target project's list.
BOARD_SH = pathlib.Path(os.path.abspath(__file__)).parent / "board.sh"

# What the shell prints, last, when it allows. Matched exactly, together with
# its exit status, so that no other output can be mistaken for permission.
ALLOWED = "ALLOWED"

# The shell's side. `$0` is board.sh, `$1` the function, `$2` its argument.
# The refusal functions print a reason and succeed (status 0) when they refuse,
# and print nothing and fail with status 1 when they allow.
#
# **Only those two statuses mean anything.** A missing function is 127, a
# failed command inside it something else again, and reading every non-zero
# status as "allowed" -- as this did -- takes a broken board.sh for permission.
# So the function must exist, 0 is refused, 1 is allowed, and anything else is a
# failure to check: exit 10, 11 and 4. Exit 3 is a board.sh that would not load.
SNIPPET = """
. "$0" || exit 3
declare -F "$1" >/dev/null || exit 4
"$1" "$2"
status=$?
case "$status" in
    0) exit 10 ;;
    1) echo "ALLOWED"; exit 11 ;;
    *) exit 4 ;;
esac
"""


def _ask(function, argument):
    """Run one of board.sh's refusal functions; the refusal, or None."""
    if not BOARD_SH.is_file():
        return f"garden's board.sh is missing ({BOARD_SH}), so nothing can be checked"
    environment = {
        # A search path for bash and the commands board.sh calls, and nothing
        # else from the caller's environment: the answer depends only on the
        # project's own devices.list, which board.sh finds by where it is.
        "PATH": "/usr/local/bin:/usr/bin:/bin",
    }
    try:
        result = subprocess.run(
            ["bash", "-c", SNIPPET, str(BOARD_SH), function, argument],
            capture_output=True,
            text=True,
            env=environment,
            timeout=30,
        )
    except (OSError, subprocess.TimeoutExpired) as error:
        return f"the check could not run: {error}"

    output = result.stdout.strip()
    # **An allow with anything on stderr is not an allow.** A helper that fails
    # inside the refusal function -- "command not found" from a broken board.sh
    # -- prints there, and can leave the function returning 1 regardless.
    if result.returncode == 11 and output.splitlines()[-1:] == [ALLOWED] and not result.stderr.strip():
        return None
    if result.returncode == 10:
        return output or "refused, with no reason given"
    return f"the check failed (status {result.returncode}): {result.stderr.strip() or output or 'no output'}"


def port_refusal(port):
    """Why this project must not open `port`, or None when it may.

    Asked of sysfs, never of the port itself: opening it is what restarts most
    of these boards. A port whose board cannot be told is refused.
    """
    return _ask("board_port_refusal", str(port))


def board_refusal(identity):
    """Why this project must not touch the board with this MAC or id, or None."""
    return _ask("board_refusal", str(identity))


# --- lookups, and what a tool calls before it opens a port --------------------

# The shell's side for everything below. `$0` is board.sh, `$1` the function,
# and the rest its arguments; `"$@"` is all of `$1...`, so the function runs
# with its arguments, each one still a single word however many spaces it
# holds. Exit 3 is a board.sh that would not load and 4 a function it lacks:
# neither is a status any of these functions gives, so neither can be read as
# an answer.
LOOKUP = """
. "$0" || exit 3
declare -F "$1" >/dev/null || exit 4
"$@"
"""


def _run(function, *arguments):
    """Run one of board.sh's functions: (its exit status, what it printed on stdout).

    stdout is captured, because it is the answer. stderr is not: it is where
    board.sh says why it refused, and that goes to the tool's own stderr as it
    is, for the person running the tool. A check that could not start reads as
    status 2, refused.
    """
    if not BOARD_SH.is_file():
        print(f"garden's board.sh is missing ({BOARD_SH}), so nothing can be checked", file=sys.stderr)
        return 2, ""
    environment = {"PATH": "/usr/local/bin:/usr/bin:/bin"}
    command = ["bash", "-c", LOOKUP, str(BOARD_SH), function]
    for argument in arguments:
        command.append(str(argument))
    try:
        result = subprocess.run(
            command,
            stdout=subprocess.PIPE,
            text=True,
            env=environment,
            timeout=30,
        )
    except (OSError, subprocess.TimeoutExpired) as error:
        print(f"the check could not run: {error}", file=sys.stderr)
        return 2, ""
    if result.returncode == 3:
        print("garden's board.sh would not load, so nothing can be checked", file=sys.stderr)
        return 2, ""
    if result.returncode == 4:
        print(f"garden's board.sh has no function {function}, so this cannot be checked", file=sys.stderr)
        return 2, ""
    return result.returncode, result.stdout.strip()


def _lookup(function, argument):
    """A lookup's answer -- its first line -- or None when it has none or failed."""
    status, output = _run(function, argument)
    if status != 0 or not output:
        return None
    return output.splitlines()[0]


def identity_of_port(port):
    """The identity (MAC, or the entry's id) of the board behind `port`, or None.

    A lookup, not a permission: a board it names may still be refused. Read
    from sysfs; the port is not opened.
    """
    return _lookup("board_of_port", str(port))


def name_of(identity):
    """The name the project's list gives this identity, or None."""
    return _lookup("board_name_of", str(identity))


def lock_path(identity):
    """The lock file every tool takes for this board, or None for no identity."""
    return _lookup("board_lock_path", str(identity))


def _go_ahead(function, *arguments):
    """(status, text) from one of the functions that say "go ahead".

    Their statuses are 0 go ahead (the text is the answer), 1 absent or another
    of the project's boards, 2 refused. **Anything else is a check that failed**,
    and is returned as 2: a status these functions never give means something
    went wrong in the shell, and that is not permission. A 0 with nothing
    printed is refused for the same reason -- there is no answer to go ahead
    with.
    """
    status, output = _run(function, *arguments)
    if status == 0:
        if output:
            return 0, output.splitlines()[0]
        print(f"the check failed ({function} said go ahead, and printed nothing to go ahead with)", file=sys.stderr)
        return 2, ""
    if status == 1:
        return 1, ""
    if status == 2:
        return 2, ""  # board.sh has said why, on stderr
    print(f"the check failed ({function}, status {status})", file=sys.stderr)
    return 2, ""


def wanted(name=None):
    """(status, identity) of the board meant, when the project may touch it.

    `name` is a name or an identity from the list. With None, the list's first
    entry -- this passes no environment to board.sh, so `$BOARD` and
    `$BOARD_MAC` do not reach it: a tool that honours them reads them itself
    and passes the value here.
    """
    return _go_ahead("board_wanted", name or "")


def board_port(name=None):
    """(status, path): the stable path of the board meant -- 1 when it is not on the bus now."""
    return _go_ahead("board_port", name or "")


def stable_port(port, what=""):
    """(status, path): the stable path of `port`, when the project may open it at all.

    `what` is what refusing protects, for board.sh's message.
    """
    return _go_ahead("board_port_stable", str(port), what)


def board_is(port, what="", name=None):
    """(status, path): the stable path of `port` when it is the board meant -- 1 when it is another of ours."""
    return _go_ahead("board_is", str(port), what, name or "")


if __name__ == "__main__":
    if len(sys.argv) != 2:
        sys.exit("usage: board.py <port>")
    refusal = port_refusal(sys.argv[1])
    if refusal is None:
        print("allowed")
    else:
        print(f"refused: {refusal}")
        sys.exit(1)
