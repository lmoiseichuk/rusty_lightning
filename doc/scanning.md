# How this device scans, and why each part of it is the way it is

The AS3935 does not hand you lightning. It hands you three kinds of interrupt —
**noise**, **disturber** and **lightning** — and a set of registers that decide
which of the three an arriving waveform becomes. Scanning is the business of
choosing those registers continuously, in a room whose noise changes, without
ever choosing a value that costs a strike.

This page is the detailed form. `doc/specs.md` §3 and §4.2 hold the register
tables and the behavioural rules; this is the reasoning behind them and the
measurements that produced it.

---

## 1. The three signals, and what each one licenses

| Interrupt | What the chip means | What it licenses you to change |
|---|---|---|
| **Noise** | the input is above the noise floor for long enough that the chip cannot work | `NF_LEV` — this is exactly what that knob gates |
| **Disturber** | a waveform arrived, was **validated**, and was rejected as man-made | **nothing in the tuning space** |
| **Lightning** | a waveform passed validation, with a distance estimate and an energy figure | nothing — this is the output |

**The middle row is the one that matters and it is the one that was got wrong.**
A disturber has already passed the analog front end and the validation stage;
raising the noise floor does not remove it. For most of this project's life the
tuner counted `noise + disturbers` into one rate, called a band with disturbers
in it "noisy", and answered with `NF_LEV`.

Watched live through the storm of 2026-08-26:

```
nf 2:  11 noise, 0 disturbers
nf 3:   0 noise, 5-8 disturbers      <- one notch, and the band opened
nf 4:   still climbing
```

The first step is the system working. The steps after it are work that cannot
succeed: the tuner climbs to 7, stays "noisy", burns the ladder's whole range
and hands over to the stuck detector. Only `noise` reaches the quiet verdict
now — see `src/verdict.rs`, which exists so that claim is host-testable.

**The bottom row has a sub-row, since 0.22.0.** A lightning interrupt whose
distance reads `overhead` licenses one thing only: the hold, which changes
nothing. Not a dip that stays open, not skipping the next dip, not the rescue's
silence, not the golden record. The reading has been man-made — the hammer at
`srej 0`, September's 9,542 round the clock, November's on a USB plug-in — and
it has been lightning, through the storms of 2026-08-12 and 2026-09-10, with
nothing in it to say which. Its high energies are not a second clue: the chip
estimates distance from the stored energies of recent events (datasheet v1-04,
page 33), so a loud strike and the nearest bin are one observation.
`verdict::Batch` splits the two where the interrupt is read, and
`verdict::Window::walk` makes the hold, so both are host-tested.

---

## 2. The tuning space is one register, and that is a safety property

`NF_LEV`, values 0–7, and nothing else.

`WDTH` (watchdog threshold), `SREJ` (spike rejection) and `MIN_NUM_LIGH`
(minimum strikes) were all in the search space once and were all removed,
because each of them demonstrably rejects real strikes. The rule the design is
built on:

> **No value the tuner can reach is allowed to cost a strike.**

That is not enforced by any code path. It is a consequence of the point being
three bits wide and `defence::FIELDS` naming one register — and a table with an
extra row added back would compile, run, and quietly start trading distant
strikes for a quieter room again. `tests/defence.rs` exists to make that
change fail loudly.

**Why the rule has to be structural.** A lost strike leaves no evidence. There
is no ground truth for a flash that was never detected, so a setting that
silently costs 10% of distant strikes looks exactly like a quiet night. Every
other failure this device can have announces itself; this one cannot. So it is
prevented by construction rather than detected by monitoring.

---

## 3. What "quiet" means, and why the verdict is a rate

A window's verdict is a **rate**, not a count, so it means the same thing
whatever the window length. `Window::per_min` multiplies before dividing, so a
window that is not a whole divisor of sixty still scales instead of collapsing.

Thresholds have moved a long way, and every move was a measurement:

- `== 0` — testing for silence. Too strict: a device in a normal room is never
  silent, so the tuner never rested.
- 12/min, then 60/min, then 300/min, with the ceiling going 240 → 360.

**Testing `== 0` is the mistake worth remembering.** "No noise" is not the same
as "quiet enough to hear a storm", and the first is unreachable indoors.

---

## 4. The failure mode the whole design turns on

> **Deafness reads as quiet.**

A device that hears nothing produces a perfect quiet verdict. So an objective of
the form *"choose the most sensitive setting that reads quiet"* has its **worst
outcome as a global optimum** — a deaf configuration satisfies it perfectly and
for ever.

Two things follow, and they are the heart of how this device should scan.

**The cost of `NF_LEV` is U-shaped, and both ends are deaf.** At the top the
noise floor is above the signal. At the bottom the chip is swamped and the
servicing path saturates: booting fully open was measured at 7–9 noise events
per batch continuously, and the rescue path caps the observable rate near
300/min (an early figure; what bounds the count since the 200 ms rescue is the
chip's own ~90 ms re-raise, §5). So "when unsure, be more sensitive" is not a
safe default either.

**Absence of noise is not proof of life.** A rung must be shown to hear
*something* before the device rests at it. A disturber is proof of life — it is
a waveform the front end received and the validator examined — so a rung
producing a steady trickle of disturbers is demonstrably listening, where a rung
producing nothing at all may simply be deaf. Never rest at a rung that has not
been measured to hear something.

---

## 5. Why a saturated count is not a measurement

When events arrive faster than the servicing path retires them, the counter
stops being a rate and becomes a floor. The IRQ path drops edges (below) and the
rescue poll bounds the observable rate near 300/min, so a count at or above that
means **"at least this many"** and nothing more.

Feeding a saturated count into a rate estimate under-estimates the true rate
precisely when the band is at its worst — the moment the estimate matters most.
Such an observation is *right-censored*, and the honest treatment is to record it
as noisy, exclude it from every rate, and say so.

**Since 0.21.0 the interrupt is a high level (§6), and the worry was that the
count had changed under the thresholds.** An INT that rose while the pin was
disarmed — the rest of a pass, a 3.8 s redraw — is now serviced the moment the
loop re-arms, where the edge left it waiting, uncounted, for the batch-end poll.
The datasheet says `INT_NH` *"persists as long as the noise level is above the
threshold"*, so in a noisy band that is a re-raise the old path lost.

**Worked out in 0.22.0, the edge capped the count only while the chip slept —
and the thresholds were fitted awake.** What bounds the count is how soon the
chip raises INT again after 0x03 is read, and the logs measure it: on 0.21.x,
196 consecutive `NoiseTooHigh` gaps had p10 74 ms, median 90 ms, p90 94 ms, with
a loop that services an event in 8–10 ms — so the chip, not the loop, sets
~90 ms, about eleven a second, ~660/min.

* **Awake, the edge counted at that rate.** After a read in an ordinary pass the
  loop is back at the re-arm within microseconds, long before the next re-raise,
  so its edge is caught. An edge was lost only when the read was followed by
  more than ~90 ms of work before the re-arm — the batch-close pass with a
  redraw, a sweep's 500 ms settle — and then nothing was counted until the next
  batch-end poll, at most one batch (1 s, ~11 events) later. A window without
  strikes redraws at most twice a minute (the 30 s floor on a changed gauge), so
  0.20 awake read at least 58/60 ≈ 97 % of what 0.21 reads: one notch of `climb`
  at most, at the ladder's saturating end. Measured, awake at 160 MHz with
  disturbers unmasked (they share the INT path): 0.20.1's 5,072 gaps had median
  110 ms, p90 190 ms and **three** of 1 s or more; 0.21.x's 5,699 had median
  118 ms, p90 220 ms. The same distribution — the edge was not capping it.
* **Asleep, the edge read about once a batch.** GPIO21 was isolated in light
  sleep and not a wake source (`CONFIG_PM_SLP_DISABLE_GPIO`), and the chip went
  back to sleep within 30 ms of each batch's work (3 idle ticks), so a re-raise
  ~90 ms after a read always arrived asleep and was never seen; each 1 s timer
  wake read 0x03 once, through the ISR or the batch-end poll. That is ≤ 60/min —
  **exactly the quiet threshold, which `quiet` passes (`<=`)**. An unattended
  0.20 board, on the frugal policy, could never call a window noisy, let alone
  drowned (300/min).

The thresholds were all measured at the console, awake: 60/min from rooms
counted at 90–150/min with the air conditioning running and 121–132 in a
storm's lulls, the drowned multiple from `nf 0` at 595/min in a sweep and
300–480/min under `sensitive on`. 595/min is impossible under the asleep cap. So
the level, which wakes the chip on every re-raise, counts the same quantity
awake and asleep — the one the thresholds describe — and **nothing in the tuner
was rescaled**. What changed is the unattended board, and it changed toward the
meaning, not away from it.

For laying a field log against an old one, `status` prints the raw count since
boot and the last window's rate beside the number of batches in that window
that heard any noise — the quantity an unattended 0.20 board could count. The
`noise` rows `events` logs carry their own millisecond arrival times.

---

## 6. The interrupt path, and how an event is lost

The AS3935 asserts INT and **holds it** until register 0x03 is read. Until
0.21.0 the pin was configured for a rising edge, and what follows is how that
lost events; 0.21.0 made it a high level, for the reason at the end.

`esp-idf-hal` disables the pin inside the ISR, and the IDF's disable also clears
that pin's pending status — so an edge arriving while the interrupt is disabled
is **erased, not deferred**. And because INT is level-held, the pin then sits
statically high: no further rising edge can ever occur. One lost edge means no
more edges until something reads 0x03 by another route.

The rescue poll is that other route. It exists because this has been observed:

```
poll: found Disturber with no interrupt -- the edge was missed
```

**The consequence for scanning is subtle and important.** A degrading interrupt
path makes every window read *quiet*, because the events are never counted — and
a quiet-driven controller answers quiet by becoming more sensitive, or by
resting where it is. So a window in which the rescue poll found anything the IRQ
missed is not evidence about the band at all. It has to be withheld from the
decision, not averaged into it.

**Since 0.21.0 the trigger is a high level**, because only a level can wake the
C3 from light sleep and the chip shares one trigger field between the interrupt
and the wake. It also closes the mechanism above: an INT still high when the
loop re-arms fires at once, so no edge can be missed and nothing waits for the
poll. The poll now reads only while INT is low — high, the event is the level
interrupt's to read, after its settle — so a `poll: found … with INT low` line
is an event the IRQ wire never carried, not a missed edge. Two costs come with
the level: every re-raise is now counted (§5), and a read that fails leaves INT
high, so the loop then holds the pin until the batch closes rather than
re-arming into it on every pass.

---

## 7. Why the rung must be recorded with every event

The rung is chosen as a deterministic function of the noise, and the noise
follows the weather. So any observational comparison of "strikes per storm-minute
by rung" measures that confound rather than the knob: every high-rung minute is
also an unusual minute.

Two things follow:

- **Every logged event carries the `NF_LEV` in force** (`nf` column), written
  from `session::apply` — the only path that touches the register, so it cannot
  disagree with the hardware.
- **Randomised probe placement** is what turns the record into an experiment.
  If the probed rung is drawn at random rather than derived from the noise, the
  assignment becomes exchangeable with respect to the weather and the resulting
  yield table is a genuine randomised comparison.

This is the one place randomness earns its keep here. The tuning space has eight
values; randomly sampling eight values is not a better search than scanning
them. What randomisation buys is **falsifiability of the safety rule**, which
nothing this device has ever logged could test.

Randomised dwell length has a separate and smaller justification: a fixed cadence
measured against a periodic load — a compressor, a switching supply, a display
refresh — gives a phase-locked and therefore biased estimate of the time-average
rate, and no amount of averaging fixes it.

---

## 8. What is measured, and what is still argued

**Measured**

- Only `noise` should drive `NF_LEV`; disturbers cannot be answered by it.
  (Live, 2026-08-26.)
- Indoor gain is roughly 4× outdoor and the difference matters through walls: a
  storm seen in outdoor mode indoors produced **zero** strikes.
- I²C at 100 kHz put its fifth harmonic in the 500 kHz passband: 8–10 noise
  events a second, gone at 200 kHz. 400 kHz fails outright.
- Neither the MCU clock (160 vs 80 MHz) nor the supply (battery vs USB) is a
  measurable radiator here. The antenna self-tests at 499–500 kHz.
- The tuner hunts rather than converges: with noise near the threshold it walks
  1 → 2 → 3 → 2 → 1 → 2 once a minute indefinitely. There is no hysteresis.
- Of 1040 logged records in one storm, **not one shares a second** with another,
  where ~46 same-second pairs were expected. Something imposes a one-second
  floor that nothing in the code implements.

**Argued, not measured**

- That `NF_LEV` cannot reject a lightning waveform. This claim appears in six
  places and cites no datasheet page in any of them. It is load-bearing: it is
  the reason a high rung is considered free.
- That the disturbers *are* the storm — that real flashes arrive, fail
  validation and are reported as interference. It is the best explanation for a
  storm producing 103,000 disturbers and 27 records, and it has never been
  checked, because until now the log held no disturber timestamps to lay against
  the flashes. `events` exists to answer it.
- How often a lost edge actually costs a strike. The mechanism is proven; the
  rate is not.

---

## 9. The measurement that settles most of it

Arm the log, sweep the rungs by hand, and read the three curves nobody has.

**One open of the port for the whole sweep.** Opening it restarts the board, and
the two settings a sweep depends on — the frozen tuner and the hand-placed rung —
live in RAM on purpose, so a second open part-way would undo them. A reader that
holds the port makes every `echo` a second open, which restarts nothing:

```sh
port="<by-id path>"                              # the board's /dev/serial/by-id path
python3 garden/tools/board.py "$port" || exit    # at a refusal, nothing below opens it
stty -F "$port" 115200 raw -echo
cat "$port" > sweep.log &                        # this open restarts the board
until grep -q 'con:  listening' sweep.log; do sleep 1; done

echo "freq 160"     > "$port"                    # stay awake: light sleep drops the port
echo "events 20000" > "$port"                    # log disturbers and noise, capped to what fits
echo "mask off"     > "$port"                    # or the disturber curve is empty by construction
echo "sensitive on" > "$port"                    # freeze the tuner, or it walks off each rung
for nf in 0 1 2 3 4 5 6 7; do
    echo "defence $nf" > "$port"; sleep 600      # ten minutes a rung
done
echo "sensitive off" > "$port"
echo "mask on"       > "$port"                   # saved, so put it back
echo "freq auto"     > "$port"
kill %1

./tools/send.py --until "# end" dump | sed -n '/^# lightning strike log/,/^# end/p' > sweep.csv
```

(This recipe used to say `place 0`, which is not a console word — `defence` is —
and sent each line with its own `echo`, so every line restarted the board and
was lost in its boot.)

Offline that yields, per rung: the noise rate, the disturber rate, and how often
the poll found an event with INT low. Every argument on this page is a claim
about one of those three curves — and on 0.21.0 and later the third should be
zero, since the level interrupt leaves the poll nothing to miss; a rung where it
is not is a rung where the IRQ wire is failing.

If the disturber rate is flat across all eight rungs, the liveness idea carries
no information in this room. If it *falls* with rising `NF_LEV`, then the knob is
desensitising the front end and the founding premise of the tuning space is
wrong. Either answer is worth more than another month of reasoning.
