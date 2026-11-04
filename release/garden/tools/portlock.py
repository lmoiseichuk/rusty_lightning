#!/usr/bin/env python3

# garden project, Copyright (c) 2026 Leonid Moiseichuk under MIT License

"""Claim a board's serial port: only a port the project's list allows, and one tool at a time.

    holder = portlock.claim(port)   # keep it for as long as you use the port

Grown in moisture's tools, where the lock was written, and in color_point's,
which added the garden check in front of it; one copy here now, for every
project. A project's own `tools/portlock.py` is a few lines that load this
file from its `garden` submodule (see garden's readme), so its tools keep
writing `portlock.claim(port)`.

Both checks run **before the port is opened**, because on most of these
boards opening the port resets the board: a tool that opened first and
refused afterwards would already have restarted a board it had no business
touching. On 2026-09-22 a refused second stream did exactly that to a panel
mid-watch, when the lock still lived on the serial port itself.

**One: the project must be allowed the port.** A bench carries several
projects' boards, some of them live, and `/dev/ttyACM<n>` numbers shuffle
between plug-ins. Whether a project may open a port is not decided here: it is
`port_refusal` in `board.py`, beside this file, which runs `board.sh`. That
reads the board behind the port from sysfs -- never from the port -- and looks
it up in the project's own `devices.list`. It refuses a port that does not
exist, one whose board cannot be told, a board not on the list, and one flagged
`forbidden` there. This file keeps no copy of that rule.

**Asked last, immediately before the lock and the caller's open**, on every
claim: the answer is only good for the moment it is given, since ttyACM numbers
move underneath a long run. Anything but an explicit "allowed" refuses -- a
missing board.py, a board.sh that would not load, a function it lacks.

**Two: one tool per board.** Two readers on one port do not queue, they split
the bytes between them, and a command's reply lands half in each -- which
reads exactly like a dead board; espflash and a monitor on one port fight over
the bootloader handshake. So a lock is taken on a small file, non-blocking: a
second tool says something is in the way and leaves, and a cron job that cannot
have the port does not pile up behind an interactive session.

* **Named after the board, not the ttyACM.** `board_lock_path` in board.sh
  names it from the board's identity -- its MAC, or the entry its `serial=` or
  `path=` names -- so it follows the board across a re-enumeration, which
  hands its old ttyACM number to whatever comes next, and every project's
  tools meet at the same lock. Shell scripts take the same file with `flock`.
* **On the open file, not the file's existence.** `flock` holds while the
  file object does, and is released when the process ends for any reason --
  killed included -- with nothing to clean up. The file stays behind, empty.
* **A second claim from the same process shares the first one's lock.**
  `flock` counts two opens of one file as two holders even in one process, so a
  tool that claims again on every re-attach -- after a deep sleep took the port
  away -- would refuse itself the second time. Within one process the board is
  already held, by this process, so the second claim is granted on the same
  open file. **Each claim is its own holder, though**, and the file is closed
  -- the lock let go -- only when the last of them is: closing a re-claim
  while the first is still in use leaves the board held.

From a shell script, `python3 garden/tools/portlock.py <port>` checks the port
and prints the board's name and its lock file's path, or says on stderr why
not and exits non-zero:

    check="$(python3 garden/tools/portlock.py "$port")" || exit 1
    read -r board lock <<<"$check"
    exec 9>>"$lock"
    flock -n 9 || { echo "$port is held by another tool -- refusing" >&2; exit 1; }
"""

import fcntl
import importlib.util
import os
import pathlib
import sys

# board.py, beside this file in whichever checkout this file is in.
#
# **The path as the caller reached it, not resolved.** board.sh finds the
# project's list by trimming the path it was run by -- two levels up -- so a
# garden reached through a symbolic link still reads the project's own list.
# `os.path.abspath` makes the path absolute without resolving links;
# `Path.resolve()` would follow them, to the target's path and the target
# project's list.
BOARD_PY = pathlib.Path(os.path.abspath(__file__)).parent / "board.py"

# The locks this process holds, by lock path: see "A second claim" above.
_held = {}

# board.py once loaded, so that it is loaded once per process.
_board = None

# Said with every refusal, because it is the question a person asks next.
UNTOUCHED = "  The port was not opened, so nothing has been reset."


def garden_board():
    """garden's board.py, loaded as a module; exits, refusing, when it is missing.

    **Loaded from its path, not imported by name.** `import board` would take
    the first `board` on `sys.path`, and that name is common -- Adafruit's
    Blinka installs a `board` module of its own -- so it could find a module
    that is not garden's at all. `spec_from_file_location` names the file
    itself; `module_from_spec` makes an empty module for it, and `exec_module`
    runs the file's code to fill it in, which is what an import does after it
    has found the file.

    A board.py that is there but broken raises, and an exception stops the
    tool before it reaches any port: that fails closed too.

    Loaded once and kept: every claim asks it several questions.
    """
    # `global` lets this function assign the module's `_board`; without it,
    # the assignment below would make a new local name of the same spelling
    # and the module's would stay None.
    global _board
    if _board is not None:
        return _board
    if not BOARD_PY.is_file():
        raise SystemExit(
            f"portlock: garden's board.py is not at {BOARD_PY}, so whose a board is\n"
            "  cannot be told -- refusing every port. If the project's garden submodule\n"
            "  is empty, fetch it with: git submodule update --init\n" + UNTOUCHED
        )
    specification = importlib.util.spec_from_file_location("garden_board", BOARD_PY)
    if specification is None or specification.loader is None:
        raise SystemExit(f"portlock: {BOARD_PY} could not be loaded -- refusing every port.\n" + UNTOUCHED)
    module = importlib.util.module_from_spec(specification)
    specification.loader.exec_module(module)
    _board = module
    return module


def check_board(port):
    """(name, identity) of the board behind `port` when the project may open it; otherwise exit, port unopened.

    The board is looked up first and the decision asked last, so that nothing
    but the lock stands between the decision and the caller's open.
    """
    board = garden_board()

    # Lookups, for the lock and the messages. Neither decides anything.
    identity = board.identity_of_port(str(port))
    name = None
    if identity is not None:
        name = board.name_of(identity)

    # The decision: None when the project may open the port, a reason
    # otherwise. Only None lets the port through.
    refusal = board.port_refusal(str(port))
    if refusal is not None:
        if name:
            which = f"the board there is {name}, and "
        else:
            which = ""
        raise SystemExit(f"portlock: refusing {port} -- {which}garden says: {refusal}\n" + UNTOUCHED)

    # Allowed means the board is on the list, so it has an identity and a
    # name. Without them the two answers disagree, and a disagreement is not a
    # yes.
    if identity is None or not name:
        raise SystemExit(
            f"portlock: refusing {port} -- garden allows it, but board.sh gives the board no name.\n" + UNTOUCHED
        )
    return name, identity


def _lock_of(port, identity):
    """The lock file for the board with this identity; exits when board.sh gives none."""
    path = garden_board().lock_path(identity)
    if path is None:
        raise SystemExit(f"portlock: refusing {port} -- board.sh gives its board no lock file.\n" + UNTOUCHED)
    return path


def lock_path(port):
    """The lock file of the board behind `port`, once the project is allowed it; otherwise exit.

    Asked again whenever the port is asked about again: the board behind a
    port can change between two questions.
    """
    _name, identity = check_board(port)
    return _lock_of(port, identity)


class _Lock:
    """One board's lock as this process holds it: the open, locked file, and how many claims share it."""

    def __init__(self, path, file):
        self.path = path
        self.file = file
        self.claims = 0
        # The table this lock is kept in, held here as well: a holder dropped
        # while Python shuts down can be closed after the module's own names
        # have been cleared, and this keeps the table reachable from the lock.
        self.table = _held

    def release(self):
        """Close the file, which lets the lock go, and forget it."""
        # `pop` with a default does nothing when the path is not there.
        self.table.pop(self.path, None)
        self.file.close()


class Holder:
    """One claim on a board. Keep it while the port is in use; `close()` it, or let it go, when done.

    The board stays held while any claim on it in this process is open: the
    lock is let go when the last one closes, or when the process ends.

    `with portlock.claim(port):` closes it at the end of the block -- `__enter__`
    and `__exit__` are what a `with` statement calls on the way in and out.
    `__del__` runs when the last reference to a holder goes, so a holder that
    is simply dropped closes too, as a file object dropped would have.
    """

    def __init__(self, lock):
        self._lock = lock
        lock.claims += 1

    @property
    def closed(self):
        """True once this claim has been closed."""
        return self._lock is None

    def close(self):
        """Let this claim go; the board's lock goes with the last claim on it."""
        lock = self._lock
        if lock is None:
            return
        self._lock = None
        lock.claims -= 1
        if lock.claims == 0:
            lock.release()

    def __enter__(self):
        return self

    def __exit__(self, kind, value, traceback):
        self.close()

    def __del__(self):
        self.close()


def claim(port):
    """Check the project is allowed the port and hold its board, or exit saying why not.

    Returns a `Holder`. The board is held as long as the holder is open, so
    keep a reference to it for as long as the port is in use.
    """
    _name, identity = check_board(port)
    path = _lock_of(port, identity)

    # Already held by this process: a new claim on the same lock.
    lock = _held.get(path)
    if lock is not None:
        return Holder(lock)

    try:
        # "a+": create the file if it is not there, and never truncate it --
        # it holds nothing, but another tool may have it open.
        file = open(path, "a+")
    except OSError as error:
        raise SystemExit(f"portlock: refusing {port} -- its lock file cannot be opened: {error}\n" + UNTOUCHED)
    try:
        fcntl.flock(file.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
    except OSError:
        file.close()
        raise SystemExit(
            f"{port} is held by another tool -- refusing to share it.\n"
            "  Two readers on one port split the bytes between them, which "
            "presents as a dead board.\n" + UNTOUCHED + "\n"
            "  Wait for it, or stop the other session."
        )
    lock = _Lock(path, file)
    _held[path] = lock
    return Holder(lock)


def main(arguments):
    """`portlock.py <port>`: the board's name and its lock file, for a shell script."""
    if len(arguments) != 1:
        raise SystemExit("usage: portlock.py <port>")
    port = arguments[0]
    name, identity = check_board(port)
    print(name, _lock_of(port, identity))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
