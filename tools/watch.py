#!/usr/bin/env python3
"""Read the console WITHOUT touching DTR/RTS.

The ESP32-C3's USB-Serial/JTAG maps the host's CDC control lines onto the reset
and boot straps, so a monitor that asserts them reboots the board it is trying
to observe -- and then faithfully reports the reset it caused. pyserial asserts
DTR on open by default, which is enough to do it.

Clearing both lines *before* open() is what makes this a passive observer, and
that is the whole point of the file: it is the only way to see what a plug-in
event actually does without being the cause of it.

    ./tools/watch.py [port] [seconds]
"""
import sys
import time

import serial

port = sys.argv[1] if len(sys.argv) > 1 else "/dev/ttyACM0"

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

s = serial.Serial()
s.port = port
s.baudrate = 115200
s.timeout = 1
s.dtr = False          # GPIO9 strap released -- set BEFORE open()
s.rts = False          # EN released

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
