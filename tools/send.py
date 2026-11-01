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
through `board.sh` (`devices.list`) and refuses a port that is not it: these
are commands, and `clear` erases the strike log. `SEND_SKIP_IDENTITY_CHECK=1`
skips that, deliberately -- for a port udev has not named.
"""
import argparse
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
    in a release tree alike. Returns (exit code, stdout)."""
    helper = os.path.join(HERE, "board.sh")
    script = '. "$1"; shift; "$@"'
    done = subprocess.run(
        ["bash", "-c", script, "_", helper, function, *arguments],
        stdout=subprocess.PIPE,
        text=True,
    )
    return done.returncode, done.stdout.strip()


def find_port(asked):
    """The port to use, checked against devices.list unless told not to."""
    if os.environ.get("SEND_SKIP_IDENTITY_CHECK") == "1":
        if not asked:
            sys.exit("SEND_SKIP_IDENTITY_CHECK=1 needs --port: there is no board to look up")
        print(f"⚠ SEND_SKIP_IDENTITY_CHECK=1 -- talking to {asked} without checking which board it is",
              file=sys.stderr)
        return asked
    if not asked:
        code, found = board_helper("board_port")
        if code != 0:
            sys.exit(1)  # board.sh has already said why, on stderr
        return found
    code, _ = board_helper("board_is", asked, "commands sent to the wrong board change its settings")
    if code != 0:
        sys.exit(1)
    return asked


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

    where = find_port(args.port)

    port = serial.Serial()
    port.port = where
    port.baudrate = 115200   # ignored by USB Serial/JTAG, insisted on by pyserial
    port.timeout = 0.2
    # Cleared before the open, as `watch.py` does. It does not stop the restart
    # (see the module comment) but it is the least this tool can do to the lines.
    port.dtr = False
    port.rts = False
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
