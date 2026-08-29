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
| `events [rows]` | also log disturbers and noise, for that many rows (default 20000); `events off` stops |

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
| `srej [0-15]` | spike rejection. **`0` reports man-made impulses as lightning** — see below |
| `wdth [0-15]` | watchdog threshold, aliased `watchdog`. Raising it discards the slow-rising distant arrivals this device exists to report |
| `ap` | raise the access point and its web UI for 60 s. `ap off` drops it; `ap <ssid> <password>` stores credentials and raises with them |
| `golden [clear]` | the settings that last heard lightning, and how many strikes at them |
| `regs` | the sensor's registers as the chip actually holds them, decoded |
| `defence` | the current tuning point: raw value, percent, and the `NF_LEV` field it holds |
| `defence <raw>` | set the tuning point by hand, 0–7. `0` is fully receptive |
| `calibrate [s] [/min]` | bisect the whole space for the most sensitive point that stays quiet |
| `sensitive on\|off` | the noise floor to 0 — the most receptive the tuner's own space goes — with the auto-tune frozen |
| `freq [auto\|40\|80\|160]` | read the clock, or pin it |
| `sleep on\|off` | light sleep alone, leaving the clock where it is |
| `strike [km] [intensity]` | inject a synthetic strike |

---

## Answering the question this device cannot answer

Every recorded storm shows the same shape: a handful of strikes against a flood
of disturbers. On 2026-08-13 a visible overhead cell produced 103,000 disturbers
and 27 records. The standing explanation is that **the disturbers were the
storm** — that real flashes arrive, fail the chip's waveform validation, and are
reported as interference. Nobody has been able to check it, because the log held
only lightning: there were no disturber timestamps to lay against the flashes.

`events` fixes that. It adds `kind` (`lightning` / `disturber` / `noise`) and a
sub-second `millis` column, so arrivals can be compared against an independent
record of when flashes actually happened — a phone recording with a clock in
shot is enough; a Blitzortung screenshot is better.

```sh
echo "events 20000" > /dev/ttyACM0    # arm it before the storm
# ... let the storm run ...
echo "dump"         > /dev/ttyACM0 > storm.csv
```

**It is a budget, not a switch, and that is deliberate.** This log does not
rotate and has no size bound: it grows until the filesystem is full and then
writes fail. Indoors the device sees about eight events a second, so logging
everything writes ~31,000 rows an hour against roughly an hour of free flash.
The budget stops it before that and says so, which makes it safe to leave armed
through a storm nobody is watching — which is when the measurement is wanted.

Not persisted, like `sensitive on`: a device that came back from a power cut
silently filling its flash would be the same kind of trap.

The `millis` column answers a second question at no extra cost. Of 1040 records
in `storm-2026-08-12.csv`, **not one shares a second with another**, where about
46 same-second pairs were expected — odds against of roughly 1e-20. Nothing in
the code implements that floor. Either the servicing path has a dead time that
loses events exactly when a storm is heaviest, or the epoch column is merely
coarse; sub-second arrivals tell those apart in one storm.


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

**`calibrate` is the one worth understanding.** One register is left in the
search, `NF_LEV`, so the whole tunable state is a **3-bit number, 0–7** — and a
sweep of it is **three probes**, not hundreds.

**Everything else is deliberately outside that number**, each removed because it
could buy quiet at the price of a strike. `MIN_NUM_LIGH` suppresses strikes until
N have arrived, hiding the events the device exists to report; it is pinned at 1.
`WDTH` discards slow-rising distant arrivals, which are exactly the ones worth
reporting. `SREJ` rejects short man-made impulses and is the only knob that does
— while it was in the space the walk refunded it first on every quiet spell, and
at zero the sensor reported an electric hammer next door as lightning, 503 times
in one morning. `WDTH` and `SREJ` are settings now: see `wdth` and `srej`.

What is left is the one knob that cannot cost a strike. Raising `NF_LEV` can only
make the device miss a weak signal; it can never turn a strike into a disturber.
That is what makes an automatic search over it safe to run unattended.

Two arguments, both optional and positional:

* **`s`** — seconds per probe, 5–60, default 60. A whole sweep is three probes,
  so 60 s costs about three minutes, once.
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

**`srej` is the difference between a lightning detector and an impulse
counter.** It rejects short man-made transients on waveform shape, and nothing
else in the chip does. It used to be part of the tuning point, where §4.2's
relaxation — which refunds the least valuable register first — walked it to zero
on every quiet spell. On 2026-08-14 that produced **503 "strikes" between 07:00
and 10:36 from roofers next door with electric hammers**, with no lightning
within range, statistically indistinguishable from a real storm.

It is a setting now, defaulting to **1** — between the reference driver's 0 and
the datasheet's 2 — stored in NVS and written at boot. Raise it if the log fills during obvious local work — construction, a
failing appliance, anything impulsive — and remember that every notch also costs
real detections, so raise it only as far as it needs to go.

```
srej          report the current level
srej 1        the default: least rejection that is not none
srej 2        the datasheet's value
srej 8        measured here as enough to silence an electric hammer entirely
```

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
starts mid-range on the one knob it walks (`NF_LEV`), with spike rejection
at its most sensitive — spike rejection is not a volume control, so it is not
pre-set to a guess.

**`sensitive on` opens the noise floor and freezes the auto-tune**, because otherwise
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


## The access point and the web UI

**Hold BOOT for five seconds.** Two to five seconds flips indoor/outdoor gain;
five or more raises a WPA2 access point and a web server, and the panel switches
to a join screen carrying two QR codes — one for the network, one for the page.
Another long press closes it, and so does sixty seconds with nobody connected.

The page carries everything: the strike counts, the receiver's state, the board's
own vitals, the recent-strike table, every setting that has a console command,
and a CSV download of the whole log.

**Every button on that page composes a console command line.** There is one
command table, one parser and one set of range checks; the web UI is a second
way of typing into them rather than a second implementation of them. That is why
nothing on the page can do anything `help` does not list, and why a setting's
valid range is the same in both places without either knowing about the other.

A few consequences worth knowing:

* **The password is generated once and not saved.** A device that has never had
  one invents eight characters from an alphabet with no `0`/`O` or `1`/`l`/`I`,
  shows them on the panel, and stores nothing. It will be different next time.
  That is deliberate — a password nobody has read should not quietly become
  permanent. `ap <ssid> <password>` stores a chosen pair.
* **The network name is derived and stable**, `lightning-XXXXXX` from the last
  three bytes of the MAC, so a QR code printed once keeps working.
* **Light sleep is suspended while the portal is up.** It powers down the modem
  between beacons, which drops an associated station mid-request. The window is
  at most a minute, so this costs a minute of radio rather than a policy
  exception.
* **Handlers never touch the sensor.** They read a snapshot the main loop
  publishes and queue a command line it runs — the console's state is written
  for a single caller, and the I2C bus has no lock.


## The golden combination

**The device writes down what it was set to when it heard lightning.**

This exists because of an asymmetry that makes the tuner hard. Everything the
device measures is *quiet*, and quiet is two different things wearing one face:
a working receiver under a still sky, and a deaf one under a storm. The auto-tune
cannot tell them apart — which is why "minimise events" has **deafness as its
global optimum**. A receiver that hears nothing has scored perfectly.

A detected strike is the only signal that breaks the tie. It is proof, not
inference, that this exact combination of `NF_LEV`, `WDTH`, `SREJ` and the AFE
gain was able to hear lightning *from this room*. So it is recorded, and it is
used three ways:

* **At boot**, it outranks the stored point. The stored point is wherever the
  walk happened to be when power went; if the device had been driven deaf, that
  is the deafness it resumes, silently. The record is a measurement.
* **As a rescue.** If the tuner sits deafer than the record and hears nothing
  for 20 minutes, it returns to the record and says so. Silence at a deafer
  setting than one this room has produced strikes at is evidence about the
  receiver, not about the sky.
* **As a report.** `golden` prints it.

Two strikes are needed before the record is trusted. **One is not evidence that
a setting can hear lightning** — at `srej 0` a single detection may be an
electric hammer, and this device has logged 503 of those in a morning. Two is
cheap in any real storm and discards the isolated false positive.

The record can only pull the point *toward* something already proved, never past
it. A tuner that has found something more open keeps it. And a record made at
one AFE gain is never applied at the other: a noise floor learned indoors
describes a front end seeing roughly four times what the outdoor one sees, and
carrying it across is exactly how a good point becomes a deaf one.

`golden clear` forgets it; the next strike starts a new record.
