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
that name first (Adafruit's Blinka installs one):

    import importlib.util, pathlib
    path = pathlib.Path(__file__).resolve().parent.parent / "garden" / "tools" / "board.py"
    spec = importlib.util.spec_from_file_location("garden_board", path)
    board = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(board)

    refusal = board.port_refusal("/dev/serial/by-id/...")
    if refusal is not None:
        sys.exit(f"refusing: {refusal}")

From a shell, to see an answer by hand:

    python3 garden/tools/board.py <port>
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


if __name__ == "__main__":
    if len(sys.argv) != 2:
        sys.exit("usage: board.py <port>")
    refusal = port_refusal(sys.argv[1])
    if refusal is None:
        print("allowed")
    else:
        print(f"refused: {refusal}")
        sys.exit(1)
