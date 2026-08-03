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
seconds = int(sys.argv[2]) if len(sys.argv) > 2 else 60

s = serial.Serial()
s.port = port
s.baudrate = 115200
s.timeout = 1
s.dtr = False          # GPIO9 strap released -- set BEFORE open()
s.rts = False          # EN released
s.open()

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
