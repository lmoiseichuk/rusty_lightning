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
strikes for a quieter room again. `tests/host/defence.rs` exists to make that
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
300/min. So "when unsure, be more sensitive" is not a safe default either.

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

---

## 6. The interrupt path, and how an event is lost

The AS3935 asserts INT and **holds it** until register 0x03 is read. The pin is
configured for a rising edge.

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

Arm the log, sweep the rungs by hand, and read the three curves nobody has:

```sh
echo "events 20000" > /dev/ttyACM0     # log disturbers and noise too, bounded
echo "place 0"      > /dev/ttyACM0     # then 1..7, ten minutes each
echo "dump"         > /dev/ttyACM0 > sweep.csv
```

Offline that yields, per rung: the noise rate, the disturber rate, and how often
the rescue poll found what the interrupt missed. Every argument on this page is a
claim about one of those three curves.

If the disturber rate is flat across all eight rungs, the liveness idea carries
no information in this room. If it *falls* with rising `NF_LEV`, then the knob is
desensitising the front end and the founding premise of the tuning space is
wrong. Either answer is worth more than another month of reasoning.
