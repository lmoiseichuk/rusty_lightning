#!/usr/bin/env python3
"""Read the console, touching DTR/RTS as little as pyserial allows.

    ./tools/watch.py [port] [seconds]

The ESP32-C3's USB-Serial/JTAG maps the host's CDC control lines onto the reset
and boot straps, so a monitor that asserts them reboots the board it is trying
to observe -- and then faithfully reports the reset it caused. pyserial asserts
DTR on open by default, which is enough to do it.

**Clearing both lines before open() does not make this passive after all.**
Written as the one way to watch a plug-in without causing a reset, and measured
on 2026-11-01 to cause one anyway: `rst:0x15` on every open, with DTR and RTS
both cleared -- and `picocom --noreset` did the same. Which transition does it
is not settled: the kernel raises both lines on the first open of the port
whoever asks, and pyserial then lowers them one at a time. Either way, whatever
this shows begins with a restart this script caused; the restart before that is
the one in the banner's `boot: before that, newest first:` line, which is why
that line exists.

To type commands rather than only watch, use `tools/send.py`, which waits for
the console to be listening first.

## Which board

**Since the open restarts the board, the port is checked before it**, by
garden's `board.py` against lightning's own `devices.list`: a board not on the
list, one flagged `forbidden` there, and a port whose board cannot be told are
refused. With no port given, it is the board `board.sh` finds by MAC -- `$BOARD`,
or the list's first -- at its /dev/serial/by-id path. There is no default
ttyACM number: `/dev/ttyACM0` is whichever device enumerated first, and on a
bench shared with other projects that is often somebody else's.
"""
import sys
import time

import serial

# `send.py`, beside this file, already knows how to run `board.sh` and where
# garden's `board.py` is; borrowed rather than written twice.
import send

# A bare `int()` here raised an unhandled ValueError on a typo, printing a
# traceback where a usage line belongs.
if len(sys.argv) > 2:
    try:
        seconds = int(sys.argv[2])
    except ValueError:
        print(f"not a number of seconds: {sys.argv[2]!r}", file=sys.stderr)
        print(f"usage: {sys.argv[0]} [port] [seconds]", file=sys.stderr)
        sys.exit(2)
else:
    seconds = 60

board = send.garden_board()

if len(sys.argv) > 1:
    port = sys.argv[1]
else:
    # `board_port` prints the by-id path, or says on stderr why there is none:
    # the board absent (1) or refused (2).
    code, port = send.board_helper("board_port")
    if code != 0:
        sys.exit(1)

s = serial.Serial()
s.port = port
s.baudrate = 115200
s.timeout = 1
s.dtr = False          # GPIO9 strap released -- set BEFORE open()
s.rts = False          # EN released

# **Asked immediately before the open, every time.** `None` is an explicit
# allow; anything else is the reason it is refused, including a check that
# failed to run.
refusal = board.port_refusal(port)
if refusal is not None:
    sys.exit(f"refusing to open {port}: {refusal}")

# **`open()` belongs inside the guard.** It sat above the try, so the single
# most common failure -- the port missing, or already held by another console --
# produced a raw traceback instead of the message written for exactly that case.
try:
    s.open()
except serial.SerialException as e:
    print(f"could not open {port}: {e}", file=sys.stderr)
    print("if the board is asleep the port comes and goes; try again, or", file=sys.stderr)
    print("check nothing else is holding it.", file=sys.stderr)
    sys.exit(1)

end = time.time() + seconds
try:
    while time.time() < end:
        data = s.read(512)
        if data:
            sys.stdout.write(data.decode(errors="replace"))
            sys.stdout.flush()
except (serial.SerialException, OSError) as e:
    print(f"\n(port dropped: {e})")
finally:
    try:
        s.close()
    except Exception:
        pass
