# The serial console

The device carries a small command language over its USB-C port. It is the only
way in until §9's web interface exists, and it is how every bug in this project
so far has actually been found.

---

## Connecting

The XIAO ESP32-C3 uses **native USB Serial/JTAG**, not a USB-UART bridge. There
is no baud rate in any real sense — the port is USB all the way down, and the
number is ignored. Set one anyway, because the tools insist.

The device appears as `/dev/ttyACM0` (Linux) or `/dev/cu.usbmodem*` (macOS).

### The quick one — `screen`

```sh
screen /dev/ttyACM0 115200
```

Leave with `Ctrl-A` then `k`, then `y`. If you forget, the port stays claimed
and the next tool reports it as busy; `pkill screen` clears it.

### Nicer — `picocom`

```sh
picocom -b 115200 --imap lfcrlf /dev/ttyACM0
```

`--imap lfcrlf` matters: the firmware ends lines with `\n` alone, and without
this every line after the first starts at the column the last one ended on.
Leave with `Ctrl-A` `Ctrl-X`.

### Scripted — `stty` plus plain redirection

Useful when you want the output in a file rather than on a screen:

```sh
stty -F /dev/ttyACM0 115200 raw -echo -echoe -echok
cat /dev/ttyACM0 &            # reader in the background
echo status > /dev/ttyACM0    # writer
```

`raw` is the important flag. Without it the line discipline buffers, translates
newlines and interprets `Ctrl-` characters, so commands arrive mangled or not at
all. `-echo` stops your own typing coming back at you.

Stop the reader with `kill %1` when done.

### Scripted, reliably — Python

What the development scripts in this repo use. `pyserial` handles the port
lifecycle properly, and it is the only approach that reads and writes at once
without fighting the shell:

```python
import serial, time

port = serial.Serial('/dev/ttyACM0', 115200, timeout=1)
time.sleep(2)                      # let the port settle after opening
port.reset_input_buffer()
port.write(b'status\n')
time.sleep(3)                      # give the device time to answer
print(port.read(port.in_waiting or 1).decode(errors='replace'))
```

---

## Two things that will confuse you first

**The first command after a quiet spell may be swallowed.** When the device is
light-sleeping its USB PHY is powered down, and the leading characters of a
command are lost while it wakes. The symptom is `?  try: help` in response to a
perfectly valid command. Send a bare newline first, wait a second, then send the
real command — or pin the clock (below), which stops it happening at all.

**Opening the port resets the device.** Most terminal programs assert DTR on
open, and on this chip DTR is wired to the boot strap. If you see the banner
scroll past every time you connect, that is why, not a crash. `picocom
--noreset` avoids it where you need the device to keep running.

---

## Keeping the port alive while you work

This is the one piece of setup worth doing before any debugging session.

The device drops to its frugal power policy five minutes after boot — 80 MHz
with light sleep on — and **light sleep powers down the USB PHY**. The port
stops answering, `espflash` cannot connect, and a watcher script dies with
`OSError: [Errno 5] Input/output error`. That is by design: it is the difference
between roughly 2.5 days and 30 on the battery (§7).

Typing anything extends the awake window by ten minutes, so an interactive
session mostly keeps itself alive. For anything longer, pin the clock:

```
freq 160
```

The clock is now fixed, light sleep is off, and the power policy is paused until
you say otherwise. The board stays reachable indefinitely. Undo with:

```
freq auto
```

Neither setting is saved to NVS. A power cycle always returns the device to its
normal policy — a board that came back from a power cut silently refusing to
sleep would quietly cost the battery, and you would not find out for weeks.

---

## Commands

Type `help` on the device for the current list. Everything below also counts as
console activity, which is what holds the awake window open.

### Looking around

| Command | What it does |
|---|---|
| `help` (or `?`) | the command list |
| `health` | vitals — clock, CPU MHz, die temperature, RAM, flash, battery. The screen's top line. |
| `status` | sensor state — mode, defence level, session counts, last hour, antenna. The screen's second line. |
| `date` | the current time, in local time |
| `dump` | the whole strike log as CSV, including records not yet synced to flash |

### Setting things

| Command | What it does |
|---|---|
| `date <unix-epoch>` | set the clock. There is no RTC and no network yet, so this is how the device learns the time. |
| `tz <hours>` | local offset, e.g. `tz -4` for US Eastern. Whole hours here; the firmware stores minutes, so half-hour zones need no format change later. |
| `scope day\|week\|month` | which span the charts cover |
| `mode indoor\|outdoor` | AFE gain — the same switch as holding the BOOT button |
| `clear` | erase the strike log. No confirmation. |

Setting the clock from the host, which is almost always what you want:

```sh
echo "date $(date +%s)" > /dev/ttyACM0
```

### Diagnostics

| Command | What it does |
|---|---|
| `regs` | the sensor's registers as the chip actually holds them, decoded |
| `sensitive on\|off` | every rejection knob below the auto-tune ladder's floor, with the ladder frozen |
| `freq [auto\|40\|80\|160]` | read the clock, or pin it |
| `sleep on\|off` | light sleep alone, leaving the clock where it is |
| `strike [km] [intensity]` | inject a synthetic strike |

---

## Why some of these exist

**`regs` prints the chip, not our model of it.** Every other status line reports
what the firmware *believes* it wrote. This reads the AS3935 back over I2C and
decodes it, including a loud warning when the IRQ pin has been left as a clock
output — a state in which the sensor is completely deaf and nothing else looks
wrong. It was written during a storm that produced no detections and settled in
one command what an hour of reasoning had not.

**`mode outdoor` is the *lower* gain**, which is the counter-intuitive half.
Indoor gain is roughly four times outdoor. A strike close enough to hear can
saturate the front end, fail the chip's waveform validation, and be reported as
a disturber — so when a near storm produces disturbers and no strikes, *less*
gain is the thing to try.

**`sensitive on` goes below what the auto-tune can reach.** The §4.2 ladder
starts at `WDTH 2` because that is the chip's power-on default, which made it
look like a floor. The field is four bits and goes to 0. This sets every knob to
its minimum and freezes the ladder, because otherwise the first disturber climbs
straight back off it. Expect a lot of disturbers — that is the trade.

**`strike` exists because a real strike cannot be provoked.** The AS3935
validates the waveform, so no spark generator, piezo igniter or lighter will
ever produce a `Lightning` classification — a lighter raises the disturber count
and nothing else. The simulator exercises recording, scoring, logging and
charting without one.

**`sleep` is separate from `freq`** because they answer different questions: the
clock is about speed, light sleep is about whether the USB port survives. Deep
sleep is deliberately absent — it loses RAM, so the history rings would have to
be rebuilt from CSV on every wake, and this device has no reason to use it.

---

## A debugging session, end to end

```sh
stty -F /dev/ttyACM0 115200 raw -echo
cat /dev/ttyACM0 &

echo "date $(date +%s)" > /dev/ttyACM0   # the clock is only as fresh as its last save
echo "freq 160"         > /dev/ttyACM0   # stay reachable
echo "regs"             > /dev/ttyACM0   # confirm the sensor is configured as intended
echo "status"           > /dev/ttyACM0   # and that it is hearing something

# ... leave it running; strikes print as they arrive ...

echo "dump"             > /dev/ttyACM0   # the whole log when you are done
echo "freq auto"        > /dev/ttyACM0   # hand the clock back before you walk away
kill %1
```

The device writes every strike to `/lfs/strikes.csv` on its own filesystem
whether or not anything is connected, so nothing is lost by disconnecting. `dump`
after the fact gets the full record, and the charts survive a power cut because
the rings are rebuilt from that file at boot.
