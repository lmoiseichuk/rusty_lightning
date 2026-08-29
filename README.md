# Rusty lightning sensor

Basically that is practical playground for Rust: idea is to monitor thunderstorms and lights
and show output on wide screen with battery monitoring, now XIAO 7.5" ePaper Panel
as it has esp32c3 inside with an excellent Rust support

The panel looks ![in usual mode as](https://github.com/lmoiseichuk/rusty_lightning/blob/main/doc/panel.jpg)

And panel ![details available](https://github.com/lmoiseichuk/rusty_lightning/blob/main/doc/internals.jpg)

All details collected [in specification](doc/specs.md)

How the device actually scans — what the three interrupt kinds each license, why
the tuning space is one register, and the failure mode the whole design turns on
(*deafness reads as quiet*) — is [in the scanning guide](doc/scanning.md).

The device has a serial console over its USB-C port — how to connect to it, the
command list, and how to keep the port alive while debugging (light sleep takes
it away) are [in the console guide](doc/console.md).

## The console, in brief

The [console guide](doc/console.md) is authoritative and lists everything. This
is only the handful reached for daily, kept short deliberately: a second full
copy of the command list is a second place for it to go stale.

| Command | What it does |
|---|---|
| `health` / `status` | vitals, and sensor state — the screen's two top lines |
| `dump` / `clear` | the strike log as CSV; erase it, no confirmation |
| `freq 160` | pin the clock, so light sleep stops taking the USB port away |
| `merge [ms]` | strike-merge window — how many return strokes count as one flash |
| `calibrate [s] [/min]` | bisect the tuning space for the most sensitive quiet point |
| `regs` | the sensor's registers as the chip actually holds them |
| `strike [km] [int]` | inject a synthetic strike |

> **Set the clock after every flash.** `espflash` resets the chip, which stops
> the RTC — so `restore` falls back to whatever NVS last saved, up to fifteen
> minutes stale. Observed: a board reflashed twice inside four minutes rewound
> its clock each time, and a strike stamped in between would have carried a
> timestamp minutes early. One line fixes it:
>
> ```sh
> echo "date $(date +%s)" > /dev/ttyACM0
> ```

## Status

**The sensor has seen real lightning** — 917 strikes on 2026-08-12, over three
and a half hours, cross-checked against flashes audible outside every 10–30 s.
Pull a storm off the device with `dump`; they are not kept in the repository,
because in Florida there is another one along in a few days.

§9 of the [specification](doc/specs.md) records what it did and did not settle.
The auto-tune's strike-hold rule works: at 208–299 events/min it held the
receiver wide open for the whole storm instead of climbing into deafness. The
chip's distance estimate does not: all 917 read the nearest bin, and the
suspected cause — statistics that were never cleared — is fixed but unproven
until the next storm.
