
### Clock skew, measured 2026-08-26

| observed | panel said | real | behind by |
|---|---|---|---|
| 2026-08-23 | 13:54:55 | 14:50 | ~55 min |
| 2026-08-26 | 11:03 | 11:54 | ~51 min |
| 2026-08-26 12:28 | 11:31:58 | 12:28:42 | **57 min** |

**Roughly constant, not growing.** That matters: a stored epoch that never gets
re-saved would fall further behind every day, and this does not. So the epoch
*is* being written; what is lost is bounded and recurring, which points at the
restore arithmetic or at a device that reboots often enough to keep paying the
same penalty.

`clock::SAVE_INTERVAL_S` is 15 min, so a single reboot should cost at most 15
min plus the time actually off. Losing ~55 every time is about four of those.

The moisture panel does not do this, and prints `clock: restored from reset,
outage assumed, +2 s` where lightning prints only `time: restored <stamp>` —
diff those two restore paths before theorising further.

**This storm's strike log will be stamped ~57 min early.** The offset is known,
so the records are correctable; not worth a reboot mid-storm to fix.

### The tuner climbs on disturbers, and NF_LEV cannot answer them

Watched live, 2026-08-26, indoor, through a storm:

    nf 2:  11 noise, 0 disturbers
    nf 3:   0 noise, 5-8 disturbers      <- one notch, and the band opened
    nf 4:   still climbing

The first step is the system working: `NF_LEV` is exactly the knob for a chip
drowning in its own noise floor, and `[wd 2 sr 0 fixed]` shows the two
strike-costing knobs held still while it moved. That is 0.11.0 earning its keep.

**The steps after it are pointless.** `Tuning::observe` counts
`noise + disturbers` into `events`, so a storm throwing disturbers reads as a
noisy band — and the tuner answers with `NF_LEV`, which gates the *noise floor*
and does nothing to a validated disturber. It will climb to 7, stay noisy, and
hand over to the stuck detector.

Harmless for detection, because `NF_LEV` cannot reject a lightning waveform
either. But it is work that cannot succeed, and it burns the ladder's whole
range before the stuck detector notices.

`Tuning::hold` already refuses to climb when a window saw strikes, for exactly
this reason — a nearby strike throws harmonics that arrive as disturbers. It
does not fire here because the window saw *no* strikes, which is the case that
matters most.

Worth considering: hold on a high disturber rate as well as on strikes, or count
only `noise` into the quiet verdict since that is the only thing the knob can
change. The second is closer to the truth and smaller.

**Fixed 2026-08-28, not yet flashed.** Took the second option, which the note
already called closer to the truth and smaller: `observe` folds only `noise`
into the verdict. Disturbers are still counted and still reported — they are
simply not evidence about the noise floor.

The decision moved to `src/verdict.rs`, free of ESP-IDF like `defence` and for
the same reason: `tuning` drives an I²C sensor and cannot be built on a
workstation, so the one claim worth checking was untestable. `tests/host/verdict.rs`
now compiles the real module and asserts the negative directly — *a window of
disturbers alone is quiet*. Nothing in the type system says that, and folding the
two together compiles and looks reasonable, which is how it shipped.

**Still not flashed: the storm is running.** The board is detecting and a
reflash costs a reboot and the RTC with it.

### Clock: resolved 2026-08-26, and the cause was us

Set to the host's time: panel 12:54:48 against host 12:54:42, six seconds fast,
down from 56 minutes behind. `time: restored` confirms it persists across a
reboot.

**The drift was not a bug in the running device.** `restore()` prefers a live
clock and only falls back to the stored epoch when there is nothing running --
and the RTC keeps counting across resets, so ordinary reboots cost nothing. The
fallback is reached only on a *true power cut*, and there it costs (up to
`SAVE_INTERVAL_S`, 15 min) plus however long the board was actually off, with no
compensation for either.

That is precisely the "battery OFF, USB out, wait, back" cycle this board needs
to reach its bootloader, and today's flashing did several. The skew grew 51 to
57 minutes across them, which is about one cycle's worth each.

So: in the field, on battery, it should hold. It drifts when somebody power
cycles it, which is a development cost rather than an operational one.

Worth doing if that becomes annoying: drop `SAVE_INTERVAL_S` from 15 min to 5,
which trades ~35k NVS writes a year for ~105k and cuts the worst case per cycle
by two thirds. Not done -- it needs a reflash, which needs a power cycle, which
costs exactly what it saves.
