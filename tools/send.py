#!/usr/bin/env python3
"""Type console commands into the board once it is listening, and show the replies.

    ./tools/send.py --clock                    # set the clock to the host's, now
    ./tools/send.py status regs                # each command, and its answer
    ./tools/send.py --for 30 "mask off"        # then keep reading for 30 s
    ./tools/send.py --until "# end" dump       # read until the dump says it is done
    ./tools/send.py --port /dev/serial/by-id/... --clock status

## Why `echo "date $(date +%s)" > /dev/ttyACM0` is not enough

**Opening the port restarts this board.** Measured on 2026-11-01: `rst:0x15`
(the USB Serial/JTAG peripheral resetting the chip) on every open, from
pyserial with DTR and RTS cleared before the open and from `picocom
--noreset`. A bare `echo` was not tested. Nothing suggests it differs -- the
kernel raises both lines on the first open of the port, whoever asks -- and a
recipe that is only right if it does is not worth keeping.

And a restarted board does not read its console for a while: the banner waits
two seconds, the sensor is configured and its antenna measured, the panel
shows the logo for five, and only then does the loop start reading. A line
written at the open is written to nobody. `echo` exits 0 either way, so the
clock simply stays wherever NVS last left it -- up to fifteen minutes stale
after a flash -- and every strike until the next correction carries that error.

## What this does instead

It opens the port **once** and holds it, so there is one restart at most:

* if the boot banner appears within a few seconds, the open restarted the board,
  and it waits for `con:  listening` -- the line the firmware prints when the
  loop starts reading -- before typing anything;
* if nothing appears, the board was already running and may be light-sleeping,
  which loses the first characters while the USB PHY wakes; so it sends a bare
  newline and waits a second first, as doc/console.md says to by hand.

`--clock` sends `date <epoch>` with the epoch read **at the moment it is sent**,
not when the shell expanded `$(date +%s)` -- behind a fifteen-second boot that
would set the clock fifteen seconds slow, on the one command whose whole job is
to be right.

Firmware before 0.21.1 does not print `con:  listening`; against it this waits
the full `--boot` time and then sends anyway, saying so.

## Which board

Like every script here that writes to a device, it finds the board by MAC
through `board.sh` -- lightning's own `devices.list`, read with garden's rules
-- and refuses a port that is not it: these are commands, and `clear` erases
the strike log. It refuses a board that is not on lightning's list, one flagged
`forbidden` there, and a port whose board cannot be told, since opening the port
restarts whatever is on it.

`SEND_SKIP_IDENTITY_CHECK=1` skips the identity check -- "is it the configured
board" -- deliberately, to talk to another of lightning's boards without naming
it in `$BOARD`. It does not skip those refusals.

**Asked again immediately before the open**, of garden's `board.py`, which
reads sysfs and never the port, and fails closed: a check that could not run
is a refusal.
"""
import argparse
import importlib.util
import os
import subprocess
import sys
import time

import serial

HERE = os.path.dirname(os.path.abspath(__file__))

# What a restart looks like from the host: the ROM's reset line, then ours.
RESTART_MARKERS = ("rst:0x", "=== lightning terminal ===")
# Printed by `listen::run` immediately before the loop starts reading.
READY_MARKER = "con:  listening"


def board_helper(function, *arguments):
    """Run a function from board.sh, which sits beside this file in tools/ and
    in a release tree alike. Returns (exit code, stdout).

    `|| exit 2`: a board.sh that cannot load garden's would otherwise go on to
    run a function that does not exist. That fails too, but only by luck of
    polarity -- every check here answers 0 for "go ahead", so a missing one
    reads as "refused" -- and a failed load is worth stopping at by name."""
    helper = os.path.join(HERE, "board.sh")
    script = '. "$1" || exit 2; shift; "$@"'
    done = subprocess.run(
        ["bash", "-c", script, "_", helper, function, *arguments],
        stdout=subprocess.PIPE,
        text=True,
    )
    return done.returncode, done.stdout.strip()


def garden_board():
    """Garden's `board.py`, imported from the garden lightning's `board.sh`
    loaded -- the submodule in a checkout, the copy in a release tree -- so the
    check before the open reads the same list as the checks before it.

    Exits when it cannot be had: a check that cannot run is a refusal, never a
    pass."""
    code, tools = board_helper("board_garden_tools")
    if code != 0 or not tools:
        sys.exit("refusing: garden's tools could not be found, so no port can be checked")
    path = os.path.join(tools, "board.py")
    if not os.path.isfile(path):
        sys.exit(f"refusing: there is no board.py in {tools}, so no port can be checked")
    # **Loaded by its path, never by `import board`.** A plain import searches
    # the module path, and another module of that name can be found first --
    # Adafruit's Blinka installs one -- or already be loaded, in which case
    # the import hands that one back without looking. Garden's own instructions
    # for a Python tool are these three lines: a module object made from this
    # exact file, under a name nothing else uses.
    specification = importlib.util.spec_from_file_location("garden_board", path)
    if specification is None or specification.loader is None:
        sys.exit(f"refusing: {path} could not be loaded, so no port can be checked")
    board = importlib.util.module_from_spec(specification)
    specification.loader.exec_module(board)
    return board


def find_port(asked):
    """The port to use -- the board's by-id path -- checked against
    devices.list. Exits, with board.sh's reason on stderr, when it is refused.

    The by-id path, whatever was asked: it is named after the board's USB
    serial, so it cannot come to mean another board, as a ttyACM number can."""
    what = "commands sent to the wrong board change its settings"
    if os.environ.get("SEND_SKIP_IDENTITY_CHECK") == "1":
        if not asked:
            sys.exit("SEND_SKIP_IDENTITY_CHECK=1 needs --port: there is no board to look up")
        # The skip skips "is it the configured board" and nothing else:
        # `board_port_stable` still refuses a board that is not on lightning's
        # list, one flagged forbidden, and a port whose board cannot be told.
        code, stable = board_helper("board_port_stable", asked, what)
        if code != 0:
            sys.exit(1)  # board.sh has already said why, on stderr
        print(f"⚠ SEND_SKIP_IDENTITY_CHECK=1 -- talking to {stable} without checking it is the"
              " configured board", file=sys.stderr)
        return stable
    if not asked:
        # `board_port` refuses a board lightning may not touch before it looks
        # for one; 1 is an absent board and 2 a refusal, and it has said which.
        code, found = board_helper("board_port")
        if code != 0:
            sys.exit(1)  # board.sh has already said why, on stderr
        return found
    code, stable = board_helper("board_is", asked, what)
    if code != 0:
        sys.exit(1)  # board.sh has already said why, on stderr
    return stable


class Console:
    """The open port, echoing everything the board says, a line at a time."""

    def __init__(self, port):
        self.port = port
        self.partial = b""

    def read_for(self, seconds, until=None):
        """Print what arrives for `seconds`, or until a line containing one of
        `until` does. Returns the marker seen, or None."""
        end = time.time() + seconds
        while time.time() < end:
            data = self.port.read(256)
            if not data:
                continue
            sys.stdout.write(data.decode(errors="replace"))
            sys.stdout.flush()
            if until is None:
                continue
            self.partial += data
            lines = self.partial.split(b"\n")
            self.partial = lines.pop()
            for line in lines:
                text = line.decode(errors="replace")
                for marker in until:
                    if marker in text:
                        return marker
        return None

    def send(self, line):
        print(f">>> {line}", flush=True)
        self.port.write(line.encode() + b"\n")
        self.port.flush()


def main():
    parser = argparse.ArgumentParser(
        description="Type console commands once the board is listening.",
    )
    parser.add_argument("commands", nargs="*", help="console lines, one argument each")
    parser.add_argument("--port", help="the board's port; found by MAC when omitted")
    parser.add_argument("--clock", action="store_true",
                        help="first send `date <now>`, the epoch read as it is sent")
    parser.add_argument("--reply", type=float, default=2.0, metavar="SECONDS",
                        help="how long to show each command's answer (default 2)")
    parser.add_argument("--for", dest="linger", type=float, default=None, metavar="SECONDS",
                        help="keep reading this long after the last command"
                             " (with --until: the most to wait for it, default 300)")
    parser.add_argument("--until", metavar="TEXT",
                        help="after the last command, read until a line containing TEXT")
    parser.add_argument("--boot", type=float, default=60.0, metavar="SECONDS",
                        help="longest to wait for the console after a restart (default 60)")
    args = parser.parse_args()

    if not args.commands and not args.clock:
        parser.error("nothing to send: give commands, or --clock")

    board = garden_board()
    where = find_port(args.port)

    port = serial.Serial()
    port.port = where
    port.baudrate = 115200   # ignored by USB Serial/JTAG, insisted on by pyserial
    port.timeout = 0.2
    # Cleared before the open, as `watch.py` does. It does not stop the restart
    # (see the module comment) but it is the least this tool can do to the lines.
    port.dtr = False
    port.rts = False

    # **Asked immediately before the open, every time** -- garden's rule, since
    # opening is what restarts the board. `None` is an explicit allow; anything
    # else is the reason it is refused, including a check that failed to run.
    refusal = board.port_refusal(where)
    if refusal is not None:
        sys.exit(f"refusing to open {where}: {refusal}")
    try:
        port.open()
    except serial.SerialException as e:
        print(f"could not open {where}: {e}", file=sys.stderr)
        print("if the board is asleep the port comes and goes; try again, or", file=sys.stderr)
        print("check nothing else is holding it.", file=sys.stderr)
        sys.exit(1)

    console = Console(port)
    try:
        # Six seconds: the reset, the ROM, and the banner's own two-second wait.
        restarted = console.read_for(6.0, until=RESTART_MARKERS)
        if restarted:
            print("\n(the open restarted the board -- waiting for its console)", file=sys.stderr)
            ready = console.read_for(args.boot, until=(READY_MARKER,))
            if ready is None:
                print(f"\n(no `{READY_MARKER}` in {args.boot:.0f} s -- firmware before 0.21.1 does not"
                      " print it; sending anyway)", file=sys.stderr)
            # Let the line that announced it finish arriving before typing.
            console.read_for(0.5)
        else:
            # Already running, perhaps asleep: the newline is what wakes the PHY,
            # and it is a blank line to the parser, which ignores it.
            port.write(b"\n")
            console.read_for(1.0)

        if args.clock:
            console.send(f"date {int(time.time())}")
            console.read_for(args.reply)
        for number, line in enumerate(args.commands, start=1):
            console.send(line)
            # A reply can be finished inside its own window, so with `--until`
            # the last command's reply is read by the wait below, from the
            # moment it goes -- not after a window that might have eaten it.
            last = number == len(args.commands)
            if last and args.until is not None:
                break
            console.read_for(args.reply)
        if args.until is not None:
            limit = 300.0 if args.linger is None else args.linger
            if console.read_for(limit, until=(args.until,)) is None:
                print(f"\n(no line containing {args.until!r} in {limit:.0f} s)", file=sys.stderr)
                sys.exit(1)
        elif args.linger:
            console.read_for(args.linger)
    except (serial.SerialException, OSError) as e:
        print(f"\n(port dropped: {e})", file=sys.stderr)
        sys.exit(1)
    except KeyboardInterrupt:
        pass
    finally:
        try:
            port.close()
        except Exception:
            pass


if __name__ == "__main__":
    main()
