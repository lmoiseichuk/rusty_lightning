# The serial console

The device carries a small command language over its USB-C port. It is the only
way in until §9's web interface exists, and it is how every bug in this project
so far has actually been found.

---

## Connecting

The XIAO ESP32-C3 uses **native USB Serial/JTAG**, not a USB-UART bridge. There
is no baud rate in any real sense — the port is USB all the way down, and the
number is ignored. Set one anyway, because the tools insist.

The device appears as `/dev/ttyACM<n>` (Linux) or `/dev/cu.usbmodem*` (macOS).
On Linux, address it by its stable path,
`/dev/serial/by-id/usb-Espressif_USB_JTAG_serial_debug_unit_<MAC>-if00` —
written `<by-id path>` below — never by the number: that is whichever device
enumerated first, and on a bench shared with other projects it is often one of
theirs. **Opening the port restarts the board**, so ask first whether it is
lightning's to open (see [Which board a script talks to](#which-board-a-script-talks-to)):

```sh
port="<by-id path>"
python3 garden/tools/board.py "$port"   # "allowed", or why not; asked of sysfs, opens nothing
```

**Every recipe below asks that first, and goes no further at a refusal** —
`&&` in front of a single command, `|| exit` at the top of a block of them.
`board.py` exits 0 only for "allowed", and a check that could not run is a
refusal too. `|| exit` is what stops a script; typed into a terminal it closes
that terminal, which is also nothing opened.

### The quick one — `screen`

```sh
python3 garden/tools/board.py "$port" && screen "$port" 115200
```

Leave with `Ctrl-A` then `k`, then `y`. If you forget, the port stays claimed
and the next tool reports it as busy; `pkill screen` clears it.

### Nicer — `picocom`

```sh
python3 garden/tools/board.py "$port" && picocom -b 115200 --imap lfcrlf "$port"
```

`--imap lfcrlf` matters: the firmware ends lines with `\n` alone, and without
this every line after the first starts at the column the last one ended on.
Leave with `Ctrl-A` `Ctrl-X`.

### Scripted — `tools/send.py`

**The one to use for anything a script sends**, because it is the only form
here that survives the restart an open causes (below):

```sh
./tools/send.py --clock                          # set the clock to the host's
./tools/send.py status regs                      # each command, and its answer
./tools/send.py --until "# end" dump \
    | sed -n '/^# lightning strike log/,/^# end/p' > storm.csv
```

It holds the port open for the whole conversation, so there is one restart at
most; if the banner scrolls past it waits for `con:  listening` — the line the
loop prints as it starts reading — before typing; if the board was already
running it sends a bare newline first, to wake a light-sleeping USB PHY. It
finds the board by MAC from `devices.list` and refuses any other, because
`clear` is a command too. `--clock` reads the epoch as it sends it, not when the
shell expanded `$(date +%s)` fifteen seconds of boot earlier.

### By hand — `stty` plus plain redirection

Useful when you want the output in a file rather than on a screen:

```sh
port="<by-id path>"
python3 garden/tools/board.py "$port" || exit   # at a refusal, nothing below runs
stty -F "$port" 115200 raw -echo -echoe -echok
cat "$port" &                 # reader in the background -- this open restarts it
# ... wait for `con:  listening` in the reader's output ...
echo status > "$port"         # writer: the reader holds the port, so no restart
```

`raw` is the important flag. Without it the line discipline buffers, translates
newlines and interprets `Ctrl-` characters, so commands arrive mangled or not at
all. `-echo` stops your own typing coming back at you.

**Start the reader first and leave it running.** It is the reader's open that
restarts the board; an `echo` while it holds the port is a second open and
changes nothing. An `echo` with no reader is the first open: it restarts the
board and its line arrives during the boot, read by nobody — and `echo` still
exits 0.

Stop the reader with `kill %1` when done.

### Python, for your own scripts

`pyserial` handles the port lifecycle properly, and it reads and writes at once
without fighting the shell. `tools/send.py` is the worked example; the shape is:

```python
import importlib.util
import sys
import serial

# garden's board.py, from the submodule, loaded by its path: a plain
# `import board` can find another module of that name first (Blinka's).
spec = importlib.util.spec_from_file_location("garden_board", "garden/tools/board.py")
board = importlib.util.module_from_spec(spec)
spec.loader.exec_module(board)

path = "<by-id path>"
refusal = board.port_refusal(path)      # asked of sysfs, immediately before the open
if refusal is not None:
    sys.exit(f"refusing: {refusal}")
port = serial.Serial(path, 115200, timeout=1)             # restarts the board
for line in iter(port.readline, None):                    # wait for the loop
    if b'con:  listening' in line:
        break
port.write(b'status\n')
```

---

## Two things that will confuse you first

**The first command after a quiet spell may be swallowed.** When the device is
light-sleeping its USB PHY is powered down, and the leading characters of a
command are lost while it wakes. The symptom is `?  try: help` in response to a
perfectly valid command. Send a bare newline first, wait a second, then send the
real command — or pin the clock (below), which stops it happening at all.

**Opening the port resets the device, every time.** This used to say `picocom
--noreset` avoids it; measured on 2026-11-01 it does not — the board restarts
with `rst:0x15` (`USB`) on every open, from `picocom --noreset` and from pyserial
with DTR and RTS both held low. A bare `echo > /dev/ttyACM0` was not tried, and
nothing suggests it differs: the kernel raises both lines on the first open
whoever asks. If you see the banner scroll past every time you connect, that is
why, not a crash.

**So a command sent at the open is lost.** The restarted board does not read
its console until the boot is done — the banner's two-second wait, the sensor
and antenna checks, the five-second logo — and says so with
`con:  listening -- type \`help\``. Type after that line. One-shot
`echo cmd > /dev/ttyACM0` is the pattern this breaks, which is why every recipe
on this page now uses `tools/send.py` or a reader that holds the port.

Two consequences shape the rest of this page. **Nothing that lives only in RAM
survives looking at it** — so `mask`, an armed `events` budget and `awake on`
are kept in NVS and come back on their own, and the banner says so. And **the restart that matters is usually the
one before the banner you are reading**: an unplug or a brownout on the cell
happens with nobody attached, and connecting to ask resets the board again. The
banner therefore prints the earlier reasons too —

```
boot: USB -- the host reset it through the port, not a fault
boot: before that, newest first: BROWNOUT, power-on
```

— and `health` repeats them for as long as the board stays up.

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
you say otherwise. The board stays reachable until it restarts. Undo with:

```
freq auto
```

Neither `freq` nor `sleep` is saved to NVS. A power cycle always returns the
device to its normal policy — a board that came back from a power cut silently
refusing to sleep would quietly cost the battery, and you would not find out for
weeks. **And every open of the port is a restart**, so `freq 160` lasts exactly
until the next tool connects.

### Across restarts — `awake on`

For a board that lives on USB and has to be there when something comes to read
it — the daily cross-check that reads the log every morning, say — there is a
saved form:

```
awake on        hold 160 MHz with light sleep off, from every boot
awake off       let it sleep on its own again
awake           report: the setting, and what the chip is enforcing now
```

`awake on` puts the board in exactly the state `freq 160` does — 160/160 MHz,
light sleep off, nothing in the policy able to change it — and keeps it there
through every restart until `awake off`. It answers at once:

```
awake on
awake: held at 160 MHz with light sleep off, from every boot until `awake off` (saved)
awake:   costs the battery -- ~2.5 days on the 2000 mAh cell, against up to a week asleep
awake:   `awake off` before it goes out on its cell
awake: now 160 MHz -- pm 160/160 MHz, light sleep off, policy awake, held by `awake on`
```

**A save that fails changes nothing.** The board keeps the lock NVS still
holds, and says so — `awake: NOT saved (…) -- left as NVS holds it: sleeps on
its own`, then `freq 160` (or `sleep on`, the other way) as the session-only
way to get what was asked for. That is what keeps every other report honest:
`status`, a bare `awake` and the web page all call the lock *saved*, and they
can only be right if the lock in RAM never holds a value NVS refused.

**`freq` stays as it was, and wins while it is in force.** It is the override
for this session; the lock is part of the policy. So `freq 80` or `sleep on`
after `awake on` takes effect until `freq auto` or a restart, and `freq auto`
hands back to a policy that the lock holds awake. `awake off` re-decides at
once, with the console counted as just used: the port stays for the usual ten
minutes and then goes.

**The power-cut concern above is real for this one, and it is answered by never
being silent rather than by forgetting.** The lock survives a power cut by
design, so a board carried outside with it still on runs awake on its cell.
Every place that reports the device's state says so:

```
pm:   ⚠ held awake by `awake on` (saved) -- 160 MHz, light sleep off, on USB or the cell
pm:     costs the battery: ~2.5 days on the 2000 mAh cell, against up to a week asleep -- `awake off` lets it sleep
```

at every boot; a `status` line (`HELD AWAKE from every boot`), a `health` line,
a bare `awake`, and a warning and a toggle on the web page. With the lock off,
boot says that too — `sleeps on its own from 5 min after boot` — so a log shows
which one a board was running.

**It does not watch for the cable, because nothing on this board can be trusted
to.** Two candidates were weighed, and each fails in the direction that would
drop the port on USB — the one thing the lock is for:

* **The fuel gauge** reads a full cell on USB and a resting cell on its own
  identically: `CRATE` exactly 0, the voltage flat. Only a measurably *falling*
  cell is a conclusion, which is what the frugal policy acts on — but nothing on
  record says what the gauge reads as a charge ends and the cell settles back,
  and a lock that gave way to a discharge seen there would drop the port while
  plugged in. It would also have to latch: asleep, the board's 2.5–3.0 mA is
  under one `CRATE` step, so the gauge stops reporting the discharge that made
  it sleep, and an unlatched lock would wake it again.
* **`usb_serial_jtag_is_connected()`** was measured claiming a host after the
  cable was pulled, which is why it was removed in August. What it watches is
  the host's start-of-frame packets (IDF 5.3.4's `CONFIG_USJ_NO_AUTO_LS_ON_CONNECTION`
  is the same signal), and those also stop whenever the host suspends the bus —
  asleep overnight, rebooting, or autosuspending an idle port. A lock that let
  go then would be gone by the morning read, and the board would not get it
  back: asleep, the PHY is down, and the port does not come back in the short
  waking windows (§7), so there would be no frames to see the host return by.

So the lock is unconditional, and the cost of that is bounded and stated.
**On a cell outside, with the lock left on:** it runs at the measured 33.5 mA
instead of 2.5–3.0, about **2.5 days of the 2000 mAh cell, against the week a
sleeping board has lasted**. The week is the observed figure and the one every
report quotes; §7's table turns the frugal current into about thirty days, but
that is idle current alone and leaves out the refreshes, the grace periods and
the access point, which the awake figure barely notices and the frugal one
does. The 13× between the two currents is the ratio to reason with; the week
is the runtime. The USB port stays up the whole time, so a cable reaches it — plugging
one in restarts the board, the lock comes back with it, and `awake off` typed
after `con:  listening` lands as usual — and the web page's toggle turns it off
with no cable at all. **`awake off` before it goes outside.** The
no-light-sleep build ignores the setting — it never sleeps either way — keeps
it for the default image, and leaves its toggle off the page.

---

## Commands

Type `help` on the device for the current list. Everything below also counts as
console activity, which is what holds the awake window open.

Several commands answer to a second name, so a half-remembered one usually
still works: `?`→`help`, `time`→`date`, `chart`→`scope`, `point`→`defence`,
`cal`→`calibrate`, `spike`→`srej`, `watchdog`→`wdth`, `gold`→`golden`,
`portal`→`ap`, `cs`→`clearstats`, `sens`→`sensitive`, `batt`→`battery`,
`epd`→`panel`. (This line used to say "six" while the parser knew twelve —
which is why it no longer gives a number.)

### Looking around

| Command | What it does |
|---|---|
| `help` (or `?`) | the command list |
| `health` | vitals — clock, CPU MHz, die temperature, RAM, flash, battery — plus the firmware's version and commit, and the restarts behind this session, newest first. The screen's top line, and then some. |
| `status` | sensor state — mode, defence level, session counts, last hour, antenna. The screen's second line. Also the raw noise count since boot and the last window's rate beside how many of its batches heard any noise, which is what an unattended 0.20 board could count — see doc/scanning.md §5 |
| `date` | the current time, in local time |
| `dump` | the whole strike log as CSV, including records not yet synced to flash. About 264,000 bytes a second at 160 MHz — a 1.4 MB log is 5–6 s — and any length: since 0.21.2 it stops for a tick every half second, where 0.21.1 was reset by the task watchdog once a dump passed ~1.32 MB |
| `events [rows\|off]` | also log disturbers and noise, for at most that many rows — capped to what the log can hold. Saved, so it counts down across restarts. Bare, it reports what is left and what would fit; `events off` stops |

### Setting things

| Command | What it does |
|---|---|
| `date <unix-epoch>` | set the clock. There is no RTC and no network yet, so this is how the device learns the time. |
| `tz <hours>` | local offset in hours, negative west of UTC — e.g. `tz -4`, `tz 2`. Whole hours here; the firmware stores minutes, so half-hour zones need no format change later. |
| `scope day\|week\|month` | which span the charts cover |
| `mode indoor\|outdoor` | AFE gain — the same switch as holding the BOOT button |
| `clear` | erase the strike log. No confirmation. |
| `merge [ms]` | strike-merge window (§4.3). Bare, it reports. `0` logs every return stroke. |
| `awake [on\|off]` | hold 160 MHz with light sleep off from every boot (`on`), or let the board sleep on its own (`off`, the default). Saved; bare, it reports. Costs the battery while on — ~2.5 days on the cell, against up to a week asleep. See [Across restarts](#across-restarts--awake-on) |

Setting the clock from the host, which is almost always what you want:

```sh
./tools/send.py --clock
```

Not `echo "date $(date +%s)" > /dev/ttyACM0`: that open restarts the board and
the line arrives during the boot, so the clock stays wherever NVS left it — and
`date` behind a fifteen-second boot would be fifteen seconds slow even if it
landed. `send.py` waits for the console and reads the epoch as it sends.

### Diagnostics

| Command | What it does |
|---|---|
| `battery` | force a fresh gauge read — voltage, charge, and the `%/hr` rate |
| `clearstats` | discard the sensor's accumulated distance estimate and rebuild from the next strikes |
| `srej [0-15]` | spike rejection. **`0` reports man-made impulses as lightning** — see below |
| `wdth [0-15]` | watchdog threshold, aliased `watchdog`. Raising it discards the slow-rising distant arrivals this device exists to report |
| `ap` | raise the access point and its web UI for 60 s. `ap off` drops it; `ap <ssid> <password>` stores credentials and raises with them |
| `golden [clear]` | the settings that last heard lightning, and how many strikes at them — strikes with a distance only, since 0.22.0; a record from before is shown as set aside |
| `import <epoch>,<distance>,<energy>,<strokes>` | append one historical record; commas and no spaces, because the console splits on whitespace |
| `regs` | the sensor's registers as the chip actually holds them, decoded |
| `defence` | the current tuning point: raw value, percent, and the `NF_LEV` field it holds |
| `defence <raw>` | set the tuning point by hand, 0–7. `0` is fully receptive |
| `calibrate [s] [/min]` | bisect the whole space for the most sensitive point that stays quiet |
| `sensitive on\|off` | the noise floor to 0 — the most receptive the tuner's own space goes — with the auto-tune frozen |
| `mask [on\|off]` | whether disturbers may raise the IRQ. Masked by default; `mask off` is the experiment that logs them. Saved, so it outlives a restart; bare, it reports |
| `freq [auto\|40\|80\|160]` | read the clock, or pin it — until the next restart; `awake on` is the saved form |
| `sleep on\|off` | light sleep alone, leaving the clock where it is |
| `panel sleep [on\|off]` | power the e-paper down after every refresh (`on`, the default) or leave it powered (`off`, how it ran until 0.21.0). Saved; redraws at once so the new setting is in force immediately. Bare `panel` reports. See below |
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
./tools/send.py "events 20000"         # arm it before the storm -- capped to what fits, and saved
# ... let the storm run ...
./tools/send.py --until "# end" dump \
    | sed -n '/^# lightning strike log/,/^# end/p' > storm.csv
```

The old recipe here, `echo "dump" > /dev/ttyACM0 > storm.csv`, never worked:
the second redirection replaces the first, so `dump` went into the file and
nothing went to the board.

**It is a budget, not a switch, and that is deliberate.** This log does not
rotate and has no size bound: it grows until the filesystem is full and then
writes fail — strikes included. Indoors with disturbers unmasked the device
sees six to ten events a second, so logging everything writes 20,000–36,000
rows an hour. The budget stops it before that and says so, which makes it safe
to leave armed through a storm nobody is watching — which is when the
measurement is wanted.

**Every budget is capped to what fits.** A row costs at most 65 bytes (the
longest the format can write), and 256 KB of free space is never spent on event
rows — about 3,500 strike records, kept so that a full event window cannot stop
the log recording lightning. Ask for more and you get what fits, with the
figures:

```
events 20000
events: asked for 20000, the log has room for 9351
events: room for 9351 row(s) -- 849 KB free, 256 KB kept back so strikes can still be written
events: logging disturbers and noise for the next 9351 row(s) (saved)
```

Bare `events` arms nothing; it reports what is left and what would fit. It
used to arm 20,000 rows, which on a log with 0.87 MB free was the one command
that could fill it.

**Saved, and counting down across restarts.** It was RAM-only, on the argument
that a device back from a power cut silently filling its flash would be a trap.
That changed for two reasons: opening the console port restarts the board, so
an armed window ended the moment anybody connected to look at it; and the trap
was the *filling*, which the cap now rules out, and the *silence*, which the
boot banner now breaks (`log:  events armed -- N row(s) left`). The count is
written with the minute's sync that writes the rows it paid for, so a restart
re-arms exactly the rows that were lost with the unsynced buffer.

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
| contains a strike, `overhead` included | **hold** — never escalate |

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

**An `overhead` strike holds, and since 0.22.0 does nothing else** — it keeps
no dip open, does not stop the next one, does not restart the twenty minutes
the rescue waits out, and does not count toward the golden combination below.
The reading has been man-made — 503 from an electric hammer at `srej 0`, 9,542
round the clock in September, and on 2026-11-01 three that matched electrical
events, a USB plug-in to the second, the end of a short run on the cell and one
mid-run — and it has been lightning: the storm of 2026-08-12 below read the
nearest bin 917 times while thunder was audible every 10–30 s, and the strikes
of 2026-09-10 that proved `wd 0` both read `overhead`. Nothing in the reading
says which, so it may not vouch for a setting. It still holds because a hold
only refuses to step: holding for the hammer costs a window the walk did not
take, and not holding for a storm overhead is the climb this rule exists to
stop. (0.22.0 alone did not hold for it; 0.22.1 does again.) They are still
logged, charted and printed — the console marks each one
`[unconfirmed -- not learned from]`, and a window that had some says
`tune: N unconfirmed strike(s) this window`. A strike whose distance could not
be read counts the same way.

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

It is a setting now, defaulting since 0.22.0 to **2** — the chip's own power-on
value — stored in NVS and written at boot. It was 1, "the least rejection that
is not none", and at 1 the device let through 9,542 `overhead` "strikes" round
the clock from 2026-09-16 to 09-24; at 2, in August, the hammer's 503 fell to
five in eight hours, at the cost of 5–6 real strikes an hour missed at 10+ km.
Which suits this room is still to be settled in the field. Raise it if the log
fills during obvious local work — construction, a failing appliance, anything
impulsive — and remember that every notch also costs real detections, so raise
it only as far as it needs to go.

**A saved value is left alone by the device on its own account.** Only
`srej <n>` writes one, so a board told `srej 1` before 0.22.0 keeps 1, and every
boot says so —
``as: ⚠ spike rejection 1 -- saved by `srej 1`, below the default of 2 since 0.22.0``
— with `srej 2` as the way to the default. The device does not migrate a person's
setting, and least of all one that is about to be compared in the field. The one
thing that overrules a saved value is a measurement, below.

**A trusted golden record can lower it at boot**, as it does the watchdog: boot
takes the more permissive of the saved value and the record's, never below 1.
Boot says when it did (`lowered from 2, lightning with a distance has been heard
at it`), and `srej <n>` says at the time when the next restart would undo it —
which, since every open of the console port is a restart, is the trap for a
comparison of 1 against 2. **So the order matters.** `srej 2` typed before two
strikes with a distance have been heard at 1 stands: the strikes after it are
heard at 2, and the record follows them. Typed after, it lasts until the next
connect, unless `golden clear` comes first — which also forgets the record's
noise floor and its trust, to be earned again at 2. The boot line that offers
`srej 2` says which of the two applies. A record from before 0.22.0 is set aside
(see the golden combination below) and lowers nothing.

**A person may set 0; the device may not choose it.** That is the rule, and it
is the same one that took `SREJ` out of the tuning space: what the machine
cannot write, it cannot spend. `srej 0` from the console is a legitimate
experiment and says so when you use it. A default, a fallback or a reset
arriving at 0 on its own is the device quietly re-entering the configuration
that logged 503 false strikes in three and a half hours — so every automatic
path clamps to 1, a golden record included, and a stored 0 is announced at
every boot rather than forgotten between sessions.

```
srej          report the current level
srej 0        allowed, but only from here -- see below
srej 1        the least rejection that is not none, the lowest the device
              will ever choose for itself, and the default until 0.22.0
srej 2        the default since 0.22.0: the chip's own power-on value
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
what §4.2 sees: the auto-tune still counts every stroke, because the strike-hold
rule wants all of them, and the strokes that carried a distance separately,
because those are what a dip and the rescue read.

**A merged flash teaches the device if any of its strokes would have alone** —
the strokes the tuner already counted as they landed, so the golden record and
the walk agree about every flash, and `merge` changes neither. One that mixed an
`overhead` stroke with a kilometre stroke is recorded at kilometres and counts;
one whose only readings were `overhead` is recorded as `overhead` and does not.
So `overhead` in the log's `distance_km` column marks a row that taught the
device nothing, with one exception it cannot show by itself: a flash of
`overhead` and `far` together is recorded as `overhead`, the nearest reading,
and counts for its `far` stroke. No strike on the board has read `far` yet, and
the console marks every flash that taught nothing
`[unconfirmed -- not learned from]`.

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

**`panel sleep` is an experiment with a default.** Until 0.21.0 the e-paper
controller was never powered down: after its first refresh it left its booster
running, and on this board that booster is an inductor switcher (L1, Q1 on the
driver board) a few centimetres from a receiver tuned to 500 kHz — the first
item on the AS3935 datasheet's list of noise sources to avoid. On 2026-11-01 the
receiver indoors heard six to ten disturbers a second, back to back, through
audible thunder, and in September it logged thousands of false `overhead`
strikes landing 6–9 s after a real one — about when that strike's redraw lands.
Neither is proven to be the panel; both are what it would look like.

So the panel now powers down after each refresh and is woken before the next.
The wake is the same reset-and-init every boot does before the logo, about a
fifth of a second plus the booster's start-up; each redraw that paid it says
`woke in N ms` and `powered down in N ms` on the console. `panel sleep off` puts
the old behaviour back, so the comparison is one command on the same board in
the same weather — and it is saved, so the console connection that sets it does
not undo it, and it is on the web page for a board running on its cell with no
cable to connect.

**It is not free in a storm.** Nothing reads the sensor while the panel is
woken, refreshed or put down, and the chip keeps only the latest event. In quiet
weather that is a few seconds an hour; in a storm a strike earns a redraw five
seconds after the last one, so the quarter second each adds comes to 100 s an
hour or more on top of the refreshes themselves. If the panel turns out not to
be the noise source, `panel sleep off` refunds it. A controller that never
releases BUSY during a wake or a power-down is given up on after two seconds
(`epd:  ⚠ wake: BUSY held past 2000 ms`) rather than holding the loop for good.

**`sleep` is separate from `freq`** because they answer different questions: the
clock is about speed, light sleep is about whether the USB port survives. Deep
sleep is deliberately absent — it loses RAM, so the history rings would have to
be rebuilt from CSV on every wake, and this device has no reason to use it.

---

## A debugging session, end to end

```sh
port="<by-id path>"
python3 garden/tools/board.py "$port" || exit   # at a refusal, nothing below runs
stty -F "$port" 115200 raw -echo
cat "$port" &                              # this open restarts the board
# ... wait for `con:  listening` ...

echo "date $(date +%s)" > "$port"          # the clock is only as fresh as its last save
echo "freq 160"         > "$port"          # stay reachable
echo "regs"             > "$port"          # confirm the sensor is configured as intended
echo "status"           > "$port"          # and that it is hearing something

# ... leave it running; strikes print as they arrive ...

echo "dump"             > "$port"          # the whole log when you are done
echo "freq auto"        > "$port"          # hand the clock back before you walk away
kill %1                                    # the last close; the next open restarts it again
```

With the reader holding the port, each `echo` is a second open and restarts
nothing. Started without it, every one of these lines would restart the board
and be lost in the boot — which is what `tools/send.py` exists to avoid.

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

**And a button that would do nothing does not build.** Until 0.21.0 the buttons
were strings written into the page, checked by nothing: `indoor gain`, `outdoor
gain`, `sync`, `reboot` and a `quiet` field all sent words the allow-list drops,
so each one silently did nothing. The page is now built from the tables in
`src/query.rs` — `ACTIONS`, `FIELDS`, `TOGGLES` — and a constant assertion
refuses to compile any entry the allow-list would not pass. The boot check then
runs every line the page can send through the console's real parser and says
`web:  every command the page can send parses`, or names the ones it cannot.
The gain buttons now send `mode indoor` / `mode outdoor`; `sync`, `reboot` and
`quiet` are gone, because no such commands exist — the quiet threshold is
`calibrate`'s second argument. The time-zone field takes hours, like `tz`.

**The page cannot freeze the receiver wide open.** "Open up and freeze" was
exactly `sensitive on` — the noise floor pinned at 0, which is the swamped end
of the ladder, measured here at 300–480 noise a minute and no strikes indoors.
It is a console command now; the allow-list lets only `sensitive off` through
from the network, and the page shows that button only while it is needed.

**`mask`, `panel sleep` and `awake` are toggles on the page**, and the page shows why the
board last restarted and the restarts before that. That is the point of having
them there: a board running on its cell outdoors has no cable, and connecting
one to change a setting — or to read why it restarted — restarts it again.
`awake` goes both ways, unlike `sensitive`: `on` costs the battery and cannot
deafen the receiver, and both of its buttons say what it costs. While it is on
the page carries a warning at the top, since a board held awake on its cell is
the one setting here that empties it.

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
  receiver, not about the sky. **It moves the noise floor, and compares only
  that**: a watchdog or spike rejection deafer than the record's is a setting
  somebody typed, which boot reconciles with the record and the rescue leaves
  alone. (Until 0.22.1 it compared all three, so after `srej 2` on top of a
  record at `sr 1` it could never finish, fired every minute and froze the walk.)
* **As a report.** `golden` prints it.

Two strikes are needed before the record is trusted. **One is not evidence that
a setting can hear lightning** — at `srej 0` a single detection may be an
electric hammer, and this device has logged 503 of those in a morning. Two is
cheap in any real storm and discards the isolated false positive.

**Only a strike with a distance counts, since 0.22.0.** `overhead` is still
logged and shown, and writes nothing here: in September a stream of them, one
every 7–10 s round the clock, made this record read "840 strikes heard here",
and on 2026-11-01 six more lifted that day's record to fourteen. A record
written before 0.22.0 cannot be split into the strikes that carried a distance
and the ones that did not, so 0.22.0 keeps its own record under new keys and
sets the old one aside: `golden` still prints it, marked *set aside*, nothing
acts on it, and `golden clear` removes both. The first boot of 0.22.0 says so
when the old record is the only one there is.

The record can only pull the point *toward* something already proved, never past
it. A tuner that has found something more open keeps it. And a record made at
one AFE gain is never applied at the other: a noise floor learned indoors
describes a front end seeing roughly four times what the outdoor one sees, and
carrying it across is exactly how a good point becomes a deaf one.

`golden clear` forgets it; the next strike with a distance starts a new record.


## Putting history back

`clear` is irreversible on the device, and `dump` beforehand is what makes it
safe — but a dumped CSV cannot be pasted back, because the console splits on
whitespace and every row carries a space inside its ISO timestamp.

`import` takes the **facts** instead:

```
import 1786651612,5,67997,1        epoch, distance, energy, strokes
import 1786651612,overhead,67997,1 the same three spellings the log uses
```

The device then builds the row through the same `Log::append` a live strike
uses, so an imported record is indistinguishable in form from a recorded one and
cannot be got subtly wrong by transcription. The ISO timestamp is *derived* from
the epoch rather than copied, so the two can never disagree.

**The charts do not update until the next boot.** They are rebuilt from the log
at startup, which is the one path that reads it — importing writes the file, and
the replay picks it up.

**`srej` is written as 1 for an imported row, and that is a judgement rather
than a reading.** The history this exists to restore was recorded entirely at
`srej 0`, where 99.1% of it was `overhead` noise from man-made impulses. What is
worth importing is the remainder that carried a real distance; marking those 1
says "trust this row" in the column that means exactly that, instead of
restating a register value that was identical for every row and distinguished
nothing.


## Which board a script talks to

`devices.list` in the checkout root holds **lightning's own boards**, one per
line, `name  MAC  [hub  port]  [flags]  [serial=<usb serial>]  [# why]`. Make it
from garden's example, which documents every column:

```sh
cp garden/devices.list.example devices.list
```

```
lightning   AA:BB:CC:DD:EE:FF   1-1  1
spare       11:22:33:44:55:66              # a second C3 terminal, for the bench
```

The first entry is the default; `BOARD=spare ./flash.sh` picks another, and
`BOARD_MAC=AA:BB:… ./flash.sh` overrides both for one command — and is still
checked against the list. The file is gitignored, so no device identity is
committed.

**Each project works with its own boards, and only those.** Every script here
refuses, by MAC and wherever the board is plugged in:

* a board that is not on lightning's list — another project's, or nobody's yet;
* one the list flags `forbidden` — a board lightning keeps listed so that it is
  never written to or cut; the note after its `#` is quoted in the refusal;
* a port whose board cannot be told: one that does not exist at the moment it
  is checked, one with no USB serial behind it (a CH340), or one whose serial
  no entry claims.

Each of those fails closed, and so does having no list, naming no board, or a
check that could not run: the board that must not be touched is exactly the one
a confused invocation would otherwise reach. **Only an explicit "allowed" is
allowed**, in the shell as in Python. Garden's shell checks answer "refused" by
succeeding, so "command not found" would read as a pass; `tools/board.sh`
therefore refuses to load at all when garden's lacks a function it calls, and
reads each answer by its exit status, refusing anything but garden's own
"allowed".

**Naming them is not decoration.** When a port is the wrong board the refusal
can say *which* one, and "it is `spare`" is a sentence somebody acts on where a
bare MAC is one they squint at:

```
⚠ /dev/ttyACM0 is not the configured board.
  it is:     spare (11:22:33:44:55:66), /dev/serial/by-id/usb-Espressif_USB_JTAG_serial_debug_unit_11:22:33:44:55:66-if00
  expected:  lightning (AA:BB:CC:DD:EE:FF)
If it is the board you mean, name it:  BOARD=spare
Refusing: flashing the wrong board replaces its firmware
```

**Which board is behind a port is read from sysfs, never by opening it** —
the open is what restarts the board — **and it is asked again immediately
before every write**, not once at the start: `flash.sh` checks after its build
and again just before espflash, `tools/recover.sh` before every erase and every
write in its retry loop, and `tools/send.py` and `tools/watch.py` just before
they open the port. Once a port has checked out, a script uses the board's
`/dev/serial/by-id` path, named after its USB serial, so a ttyACM number cannot
change hands under it.

**Every write passes `--chip esp32c3`**, so espflash and esptool refuse any
other chip after connecting and before writing anything. lightning's list can
hold more than terminals — the station's core is an ESP32-P4 — and an image
built for the C3 must never land on one.

The rules and the lookups are garden's `tools/board.sh`, in the `garden`
submodule, and garden's `tools/board.py` asks them for a Python tool.
`tools/board.sh` here sources garden's and adds lightning's few functions:
`board_port` (the board's by-id path, found by MAC), `board_is` (whether a port
is the board meant) and `board_port_stable` (whether lightning may open a port
at all). It is sourced by `flash.sh` and `tools/recover.sh`, and run by
`tools/send.py` and `tools/watch.py`. `release.sh` copies it and `send.py` in
beside the images, and garden's `board.sh` and `board.py` into
`release/garden/tools/`, with garden's `devices.list.example` beside them, so a
release tree carries the check with it. In lightning's checkout — git says the
directory above `tools/` or `release/` is the top of a work tree that tracks
that `board.sh` — the submodule is used and the copy never is. Anywhere else,
such as a release tree copied away, the copy is used, with a `devices.list` at
the copied tree's own root: never a `garden/` or a list that merely happens to
sit beside it.

To ask by hand whether a port may be opened — before `screen` or `picocom`,
say:

```sh
python3 garden/tools/board.py <by-id path>   # "allowed", or why not; opens nothing
```

**Skipping the identity check skips only that.** `RECOVER_SKIP_IDENTITY_CHECK=1`
and `SEND_SKIP_IDENTITY_CHECK=1` skip "is it the configured board", to reach
another of lightning's boards without naming it in `$BOARD`. Everything in the
list of refusals above is refused all the same.

### Cutting the power

`tools/power.sh` is garden's, run from the submodule against lightning's list:

```sh
tools/power.sh list              # lightning's boards: hub, port, present or not
tools/power.sh cycle lightning   # by name
tools/power.sh cycle AA:BB:…     # or by MAC, still checked
```

**This is the last lever.** Light sleep powers down the USB PHY, so a board
running the ordinary build appears in windows — and when it stops appearing at
all, `espflash` cannot connect, the console cannot attach, and there is no reset
line. Cutting VBUS is what is left.

It takes a **name or a MAC**, never a port number: the ports on this bench
carry devices from more than one project, and a script given a port number has
no way to know it was the wrong one. It refuses a board that is not on
lightning's list or is flagged `forbidden`, by MAC, and **with no name it cuts
nothing** — bare, it lists, and the listing shows every other board whose USB
serial is a MAC as not lightning's. The board's hub and port come from sysfs while it is
plugged in; the optional `hub` and `port` columns in the list are the fallback
for a board that has stopped enumerating, which is exactly when a cut is
wanted.

It also names whatever else is on the hub before cutting: these Genesys hubs
report `ganged`, which would mean one port cannot be switched alone. **Measured
here it is false** — port 1 was cycled with a live device on port 2 and the
neighbour did not blink — but "measured false once" is not "cannot happen", and
the neighbour may be somebody else's running experiment. Only boards it can
identify are named there: one whose USB serial is a MAC, or a bridge whose
serial a `serial=` entry on lightning's list claims. A CH340 has no serial, and
is not seen.

Garden's script runs `sudo -n uhubctl -f`: `-f` because this hub otherwise
reports *"No compatible devices detected"*, and `sudo -n` because the hub needs
root — so `uhubctl` has to be on root's PATH and allowed without a password.
