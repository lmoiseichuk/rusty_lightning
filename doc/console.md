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

Six commands answer to a second name, so a half-remembered one usually still
works: `time`→`date`, `chart`→`scope`, `point`→`defence`, `cal`→`calibrate`,
`sens`→`sensitive`, `batt`→`battery`.

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
| `merge [ms]` | strike-merge window (§4.3). Bare, it reports. `0` logs every return stroke. |

Setting the clock from the host, which is almost always what you want:

```sh
echo "date $(date +%s)" > /dev/ttyACM0
```

### Diagnostics

| Command | What it does |
|---|---|
| `battery` | force a fresh gauge read — voltage, charge, and the `%/hr` rate |
| `clearstats` | discard the sensor's accumulated distance estimate and rebuild from the next strikes |
| `regs` | the sensor's registers as the chip actually holds them, decoded |
| `defence` | the current tuning point: raw value, percent, and the four register fields |
| `defence <raw>` | set the tuning point by hand, 0–2047. `0` is fully receptive |
| `calibrate [s] [/min]` | bisect the whole space for the most sensitive point that stays quiet |
| `sensitive on\|off` | every rejection knob wide open, with the auto-tune frozen |
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

**`calibrate` is the one worth understanding.** The three noise-rejection
registers are bit fields in two bytes, so the whole tunable state is one 11-bit
number, 0–2047 — and the bisection over it is **11 probes**, not hundreds. The
layout puts `NF_LEV` in the top bits and `SREJ` in the bottom four, which
matters because binary search resolves high bits first: the knob that cannot
reject a strike is decided in the first few probes, and the finer ones are
decided last.

`MIN_NUM_LIGH` is **not** in that number. It suppresses strikes outright until N
of them have arrived, so every notch of it hides the events the device exists to
report — it is pinned at 1 and neither the sweep nor the walk may spend it.

Two arguments, both optional and positional:

* **`s`** — seconds per probe, 5–60, default 60. A whole sweep is about
  11 probes, so 60 s costs roughly twelve minutes, once.
* **`/min`** — events per minute at or below which a window counts as **quiet**,
  0–240, default 60. **Stored in NVS**; omit it to keep the current one.

That threshold is not cosmetic. Testing quiet as *zero* makes the answer depend
on how long you listened — a 60 s window has six times a 10 s window's chances
of catching one stray event. Three sweeps of the same room settled at 448, 448
and 478, differing only by probe length; the long one bought spike rejection
*and* min strikes on one to two events a minute, having rejected other points at
a hundred a minute by the identical test. The same threshold governs the ±1 walk
between calibrations, so the two can never disagree about what quiet means.

**Pick it for the room, and pick it high enough.** The default was 12/min until
0.7.3, which is one event every five seconds — no house with a refrigerator in
it manages that. This room measured 90–150/min with the air conditioning on, and
121–132/min through the lulls of a live storm. Against 12 every one of those
windows reads as noisy, so the tuner climbs continuously and spends its whole
budget on the house rather than the sky: observed pinned at `nf 7 wd 15`, 53 %
harm, hearing nothing.

At 120 in the same room a sweep settled at `wd 6` where 12 would have forced
`wd 7` plus spike rejection on top, the walk then found 20–23 % harm, and the
strike-hold rule kept it wide open through a whole storm. If the device is
climbing and never coming back down, the threshold is too low for the room —
that is the symptom, and `calibrate 5 <per-min>` re-sets it in about a minute.

`calibrate` runs as ordinary measurement windows driven by the main loop, one
probe each, so the console keeps answering and the normal screen keeps updating
throughout — the gauge shows the point currently under test.

**Between calibrations the device tracks the room**, one decision a minute, and
the asymmetry is deliberate:

| Window | What happens |
|---|---|
| `<= threshold` | relax **one** notch — refunding `SREJ` first, `NF_LEV` last |
| `> threshold` | tighten `rate / threshold` notches — spending `NF_LEV` first, `SREJ` last |
| contains a strike | **hold** — never escalate |

Quick to defend, slow to relax. The proportional step is what lets it answer a
real step change: a microwave door swing here took the band from 6/min to
94/min, and at one notch a minute the machine spent three minutes fiddling the
bottom bits while the watchdog sat untouched. Dividing by the threshold means
the step reads as "how many times over the line is this", so it rescales with
whatever you set for the room.

The strike rule matters more than it looks. A nearby strike throws harmonics
that arrive as disturbers, so a close storm looks like a noisy band to a counter
that cannot tell them apart — and climbing on that would deafen the device at
the one moment it exists for. You will see `tune: holding at ...` on the console
when this fires.

**It has now been watched doing its job.** Through the storm of 2026-08-12 the
band ran at 208–299/min against a 120/min threshold — enough to climb one to two
notches every minute — and every window containing a strike held instead, keeping
the point at `0/2047 (0%)` for three and a half hours while 909 strikes were
recorded. The risk it does *not* cover is the run-up: a storm throws disturbers
ahead of itself, and a tuner that climbs on those is deaf before the first strike
arrives, so the rule never arms. That is what `sensitive on` is for.

**`clearstats` is the only way to say "re-estimate from here"** without changing
anything else. The AS3935 builds its distance figure from the energies of recent
strikes, so it describes a receiver *and* a storm; every other reset is a side
effect of changing the gain, the point or the sensitivity, each of which alters
what is being measured as well as what is remembered.

Reach for it when the distance stops moving — a run of `nearby < 5 km` while the
thunder is plainly getting further away. The firmware does this itself at boot,
at storm end, on any gain or point change, and after three nearest-bin readings
in a row; this is the manual door for the cases none of those cover.

Note what `closest` actually means before concluding it is stuck: it is the
**minimum over the last hour**, not the current distance. A storm that has moved
off keeps it at `nearby < 5 km` for an hour afterwards, correctly.

**`merge` decides whether the log counts flashes or strokes.** One flash is
normally three to four return strokes down the same channel, tens to hundreds of
milliseconds apart, and the sensor reports each. So the raw count is always
higher than what anyone standing outside would count — on 2026-08-12 the log
read 930 while flashes were audible every 10–30 seconds.

Inside the window, strokes become one record: **energies summed**, distance
averaged over measured kilometres, and a `strokes` column recording how many
went in. Nothing is silently collapsed — a merged row says so.

```
merge            report the current window
merge 1000       the default: one second
merge 0          off, every return stroke gets its own record
```

Stored in NVS, so it survives a power cut. Accepted range is 0–10000 ms; a bad
number is refused rather than defaulted, because this setting changes what the
log *means* and a typo should not change that quietly.

The window runs from the **first** stroke of a flash, not the last. A sliding
window would let a continuous train merge without limit, so a storm overhead
could collapse into a single record hours long.

Two things it deliberately does not do. It does not fold `overhead` into the
distance average — overhead is not a kilometre figure, and averaging it in as 1
is what once made a storm read as permanently overhead. And it does not change
what §4.2 sees: the auto-tune still counts every stroke, because that is its
evidence that lightning is present and the strike-hold rule wants all of it.

**`defence <raw>` skips the sweep** when you already know the answer for a room.
`0` is fully receptive; higher is deafer. A device that has never calibrated
starts mid-range on the two volume knobs (`NF_LEV`, `WDTH`) with spike rejection
at its most sensitive — spike rejection is not a volume control, so it is not
pre-set to a guess.

**`sensitive on` opens every knob and freezes the auto-tune**, because otherwise
the first disturber climbs straight back off it. Expect a lot of disturbers —
that is the trade.

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
