#!/usr/bin/env python3

# lightning project, Copyright (c) 2026 Leonid Moiseichuk under MIT License

"""One tool per board: garden's `portlock.py`, loaded from the garden lightning's `board.sh` uses.

    import portlock
    holder = portlock.claim(port)          # before the open; keep it while the port is in use

    python3 tools/portlock.py <port>       # for a shell script: the board's name and its lock file

`claim` asks garden whether lightning may open the port -- read from sysfs,
never from the port -- and then takes the board's lock, before anything opens
the port. A second tool on the same board is refused, so `tools/recover.sh`
cannot erase the chip under a running `tools/watch.py`, and `send.py`'s `dump`
cannot be split with a monitor reading the same bytes. Everything that decides
lives in garden; this file only finds it.

**Which garden is `board.sh`'s answer, not this file's.** In lightning's
checkout it is the `garden` submodule; in a release tree copied away from the
checkout, the copy `tools/release.sh` puts in `release/garden/`. `board.sh`
already makes that choice for the shell checks -- asking git, not guessing from
what is nearby -- so this file asks it (`board_garden_tools`) rather than
making the choice a second time, where it could come out differently and lock
one garden's way while checking another's. That is why this is not garden's
readme's five-line wrapper, whose fixed `../garden` path is right in a checkout
and wrong in a release tree.

**Fails closed.** A `board.sh` that will not load, a garden without
`portlock.py`, or one that cannot be loaded: each exits, with the port not
opened.
"""

import importlib.util
import os
import subprocess
import sys

# `board.sh` beside this file: `tools/board.sh` in a checkout, and the copy in
# `release/` beside `release/portlock.py`.
HERE = os.path.dirname(os.path.abspath(__file__))

# The shell side: load lightning's `board.sh` and print which garden it loaded.
# `|| exit 2` because a `board.sh` that refused to load prints nothing useful,
# and the empty answer must not be taken for a directory.
ASK_GARDEN = '. "$1" || exit 2; board_garden_tools'


def garden_tools():
    """The `tools/` directory of the garden lightning's `board.sh` loads; exits, refusing, when there is none."""
    helper = os.path.join(HERE, "board.sh")
    done = subprocess.run(
        ["bash", "-c", ASK_GARDEN, "_", helper],
        stdout=subprocess.PIPE,
        text=True,
    )
    tools = done.stdout.strip()
    if done.returncode != 0 or not tools:
        # board.sh has said why on stderr, which is not captured.
        raise SystemExit("portlock: lightning's board.sh found no garden -- refusing every port. The port was not opened.")
    return tools


def load_garden_portlock():
    """garden's `portlock.py` as a module, loaded by its path; exits, refusing, when it cannot be.

    **By its path, never `import portlock`**: this file is the `portlock` a
    plain import finds first, beside the tool, so a plain import from here
    would find this file again. `spec_from_file_location` names the file
    itself, `module_from_spec` makes an empty module for it, and
    `exec_module` runs the file's code to fill it in -- what an import does
    once it has found the file -- under a name nothing else uses.
    """
    path = os.path.join(garden_tools(), "portlock.py")
    if not os.path.isfile(path):
        raise SystemExit(f"portlock: {path} is missing -- refusing every port (git submodule update --init). The port was not opened.")
    specification = importlib.util.spec_from_file_location("garden_portlock", path)
    if specification is None or specification.loader is None:
        raise SystemExit(f"portlock: {path} could not be loaded -- refusing every port. The port was not opened.")
    module = importlib.util.module_from_spec(specification)
    specification.loader.exec_module(module)
    return module


_garden = load_garden_portlock()

# The names lightning's tools use, handed on from garden's module: `claim`
# returns the holder that keeps the board's lock, `lock_path` names the lock
# file, and `main` is the command line for a shell script.
claim = _garden.claim
lock_path = _garden.lock_path
main = _garden.main

if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
