#!/usr/bin/env python3
"""Derive the module map in doc/modules.md from the source.

**Generated, not written**, for the same reason the schematics are: a map of
what depends on what is a second copy of a fact that already lives in the
`use` lines, and a second copy is free to disagree with the first. This reads
`src/` and rewrites only the regions between the `<!-- generated: ... -->`
markers, so the prose around them is preserved.

    tools/modules.py            # rewrite doc/modules.md
    tools/modules.py --check    # fail if it is out of date (for CI)

Counts here are counted. Nothing in the generated regions is written by hand.
"""

import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
SRC = ROOT / "src"
TESTS = ROOT / "tests"
DOC = ROOT / "doc" / "modules.md"
README = ROOT / "README.md"

# The five roles a module plays. Assigned by hand because it is an editorial
# judgement about *purpose*, not a fact derivable from the imports -- and it is
# the one thing here that is written rather than derived. A module missing from
# this table shows up as "unclassified" rather than being silently dropped.
ROLES = {
    "main": "entry",
    "listen": "entry",
    "as3935": "sensor",
    "strike": "sensor",
    "defence": "sensor",
    "tuning": "sensor",
    "session": "sensor",
    "merger": "sensor",
    "verdict": "sensor",
    "history": "record",
    "log": "record",
    "csv": "record",
    "storage": "record",
    "settings": "record",
    "clock": "record",
    "civil": "record",
    "ui": "panel",
    "screen": "panel",
    "display": "panel",
    "console": "operator",
    "commands": "operator",
    "effects": "operator",
    "press": "operator",
    "portal": "operator",
    "webui": "operator",
    "query": "operator",
    "credentials": "operator",
    "boot": "platform",
    "battery": "platform",
    "power": "platform",
    "policy": "platform",
    "system": "platform",
    "uptime": "platform",
    "i2c_scan": "platform",
}

ROLE_ORDER = ["entry", "sensor", "record", "panel", "operator", "platform"]
ROLE_TITLES = {
    "entry": "Entry and the loop",
    "sensor": "The sensor and what it hears",
    "record": "Keeping it",
    "panel": "The glass",
    "operator": "Talking to a person",
    "platform": "The board underneath",
}


def modules():
    """Every `mod x;` declared at the crate root, in declaration order."""
    text = (SRC / "main.rs").read_text()
    return re.findall(r"^\s*(?:pub )?mod (\w+);", text, re.M)


def summary(text):
    """The module's own opening sentence, from its `//!` header.

    **The first *sentence*, not the first line.** Taking one line cut
    `defence` off at "as **one 3-bit", because the sentence wraps -- and a
    summary that stops mid-clause is worse than no summary, since it reads like
    the module does less than it does. Joins continuation lines until the
    sentence ends, and stops at the blank line that separates the summary from
    the body.
    """
    collected = []
    for line in text.splitlines():
        stripped = line.strip()
        if not stripped.startswith("//!"):
            if collected:
                break
            continue
        body = stripped[3:].strip()
        if not body:
            if collected:
                break
            continue
        collected.append(body)
        if body.endswith((".", ".**", ".*", ".`")):
            break
    sentence = " ".join(collected)
    # Markdown emphasis that opened and did not close would leak into the table.
    if sentence.count("**") % 2:
        sentence = sentence.replace("**", "")
    return sentence


def dependencies(text, known):
    """Which other crate modules this one names."""
    found = set()
    # `use crate::{a, b};`
    for group in re.findall(r"use crate::\{([^}]*)\}", text, re.S):
        for part in group.split(","):
            name = part.strip().split("::")[0].strip()
            if name in known:
                found.add(name)
    # `use crate::a::B;`, `use crate::a;` and bare `crate::a::b()`
    for name in re.findall(r"crate::(\w+)", text):
        if name in known:
            found.add(name)
    return found


def code(text):
    """The source with `//` comment lines removed.

    **Scanning raw text got this wrong.** `policy` was classified as needing the
    device because its module comment contains the words `esp_idf_hal` while
    explaining that it deliberately does not import them. A classifier that a
    comment can flip is not measuring what it claims to.
    """
    return "\n".join(
        line for line in text.splitlines() if not line.strip().startswith("//")
    )


def host_tests():
    """Which module each test file compiles.

    The module under test is the one included by `#[path]`, not the file name:
    `history.rs` also compiles `strike.rs`.

    **No check counts here.** An earlier version counted `check(` calls and
    reported 219 where `tools/check.sh` reported 261 -- the tests also assert
    inside loops and helpers. Two numbers for one fact is the drift this repo
    refuses everywhere else, so the number lives in one place: `check.sh`
    prints it, and this names the files.
    """
    covers = {}
    for path in sorted(TESTS.glob("*.rs")):
        text = path.read_text()
        covers[path.stem] = re.findall(r'#\[path = "\.\./src/(\w+)\.rs"\]', text)
    return covers


def table(rows, headers):
    out = ["| " + " | ".join(headers) + " |",
           "|" + "|".join(["---"] * len(headers)) + "|"]
    for row in rows:
        out.append("| " + " | ".join(row) + " |")
    return out


def build():
    known = set(modules())
    covers = host_tests()

    tested = {}
    for stem, under in covers.items():
        for name in under:
            tested.setdefault(name, []).append(stem)

    info = {}
    for name in sorted(known):
        path = SRC / f"{name}.rs"
        if not path.exists():
            continue
        text = path.read_text()
        body = code(text)
        info[name] = {
            "summary": summary(text),
            "deps": dependencies(body, known - {name}),
            "lines": len(text.splitlines()),
            "imports_idf": "esp_idf_hal" in body,
            "tests": tested.get(name, []),
        }

    # **Purity is transitive, and the first version of this was not.** A module
    # that imports no ESP-IDF itself but names one that does cannot be built by
    # bare `rustc` either -- `ui` was listed as host-testable while depending on
    # `display`, which drives the panel. Settle it by fixpoint: a module is pure
    # only when it imports nothing from ESP-IDF and everything it names is pure.
    for name in info:
        info[name]["pure"] = not info[name]["imports_idf"]
    changed = True
    while changed:
        changed = False
        for name, entry in info.items():
            if not entry["pure"]:
                continue
            for dep in entry["deps"]:
                if dep in info and not info[dep]["pure"]:
                    entry["pure"] = False
                    changed = True
                    break

    # --- the layered map ----------------------------------------------------
    lines = []
    pure = sorted(n for n, i in info.items() if i["pure"])
    hardware = sorted(n for n, i in info.items() if not i["pure"])

    lines.append("**%d modules**, of which **%d are free of ESP-IDF** and so can be"
                 % (len(info), len(pure)))
    lines.append("compiled and tested by bare `rustc` on this machine:")
    lines.append("")
    lines.append("```")
    lines.append("host-testable (no esp_idf_hal)      needs the device")
    lines.append("-" * 34 + "   " + "-" * 20)
    for index in range(max(len(pure), len(hardware))):
        left = pure[index] if index < len(pure) else ""
        right = hardware[index] if index < len(hardware) else ""
        lines.append(f"{left:<34}   {right}")
    lines.append("```")
    lines.append("")

    covered = sorted(tested)
    untested = sorted(set(pure) - set(covered))
    if untested:
        lines.append(
            "**%d of those %d are covered by the %d files in `tests/`.** Not yet covered: %s."
            % (len(covered), len(pure), len(covers),
               ", ".join(f"`{n}`" for n in untested))
        )
    else:
        lines.append(
            "**Every one of them is covered** by the %d files in `tests/` — "
            "a module that can be host-tested is." % len(covers)
        )
    lines.append("")
    lines.append("`tools/check.sh` prints the check total. It is deliberately not repeated")
    lines.append("here: a count kept in two places is a count that disagrees with itself,")
    lines.append("which is the argument this project already makes about netlists.")
    generated_map = lines

    # --- the per-role tables ------------------------------------------------
    lines = []
    seen = set()
    for role in ROLE_ORDER:
        members = sorted(n for n in info if ROLES.get(n) == role)
        if not members:
            continue
        seen.update(members)
        lines.append(f"### {ROLE_TITLES[role]}")
        lines.append("")
        rows = []
        for name in members:
            entry = info[name]
            marks = []
            if entry["pure"]:
                marks.append("pure")
            for stem in entry["tests"]:
                marks.append(f"`tests/{stem}.rs`")
            rows.append((
                f"`{name}`",
                entry["summary"] or "--",
                ", ".join(marks) or "--",
                str(entry["lines"]),
            ))
        lines += table(rows, ["module", "what it owns", "tested", "lines"])
        lines.append("")

    stray = sorted(set(info) - seen)
    if stray:
        lines.append("**Unclassified** (add them to `ROLES` in `tools/modules.py`): "
                     + ", ".join(f"`{n}`" for n in stray))
        lines.append("")
    generated_roles = lines

    # --- who depends on whom ------------------------------------------------
    lines = []
    lines.append("Read down: a module is listed with what it names. `main` and `listen`")
    lines.append("name almost everything and are omitted, since 'the entry point uses the")
    lines.append("program' is not information.")
    lines.append("")
    rows = []
    for name in sorted(info):
        if name in ("main", "listen"):
            continue
        deps = sorted(info[name]["deps"])
        if not deps:
            continue
        rows.append((f"`{name}`", " ".join(f"`{d}`" for d in deps)))
    lines += table(rows, ["module", "names"])
    lines.append("")

    leaves = sorted(n for n, i in info.items() if not i["deps"])
    lines.append("**Depends on nothing in this crate** — the bottom of the stack, and")
    lines.append("where a change is cheapest: " + ", ".join(f"`{n}`" for n in leaves) + ".")
    generated_deps = lines

    # The README states the same two counts. **Generated there too**, because a
    # README that preaches "counts are counted, not asserted" while asserting
    # two of them is the drift it is warning about, one screen earlier.
    covered_all = not (set(pure) - set(covered))
    readme = [
        "**%d of the %d modules are ESP-IDF-free%s.**"
        % (len(pure), len(info),
           ", and every one of them is covered" if covered_all
           else ", of which %d are covered" % len(covered))
    ]

    return {
        "map": generated_map,
        "roles": generated_roles,
        "deps": generated_deps,
        "counts": readme,
    }


TEMPLATE = """# The modules

<!-- The regions marked `generated` below are written by tools/modules.py from
     the source. Do not hand-edit them; the next run overwrites them. Prose
     outside the markers is preserved. -->

`src/` is one binary crate, not a workspace: every file is a module of it,
declared in `main.rs`. There is one device here, so there is no second binary to
share code with — which is why this project has modules where the moisture
project next door has crates.

<!-- generated: map -->
<!-- end generated -->

## The line that matters

**`cargo test` cannot run in this crate.** It only builds for
`riscv32imc-esp-espidf`, and every dependency pulls in ESP-IDF. So the logic
worth testing is kept in modules that import nothing from ESP-IDF, and
`tests/` compiles those modules — the real ones, by `#[path]`, never
copies — with bare `rustc`. `tools/check.sh` runs them and refuses to build if
any fails.

That constraint is what shapes the module list. Whenever a decision turned out
to be worth testing, it was **moved out of the module that owns the hardware
and into one that owns only the arithmetic**, and the hardware module kept the
registers and re-exported the rest:

| the hardware half | the pure half it was split from |
|---|---|
| `power` — `esp_pm_configure`, the frequency ceilings | `policy` — which policy to run |
| `clock` — the RTC and NVS | `civil` — a timestamp to a calendar date |
| `webui` — rendering the page | `query` — reading what came back from it |
| `session` — the bus and the interrupt | `merger`, `verdict` — folding and judging |
| `as3935` — the registers | `strike`, `defence` — the values and the ladder |

Each of those splits was paid for by a bug. `policy` exists because a wrapping
subtraction pinned the device awake for 49.7 days at thirteen times the current;
`civil` because a wrong date is written into every row of the strike log and
nothing downstream can detect it.

## What each module owns

Summaries are the module's own first line — the source is the authority, and
this table is generated from it.

<!-- generated: roles -->
<!-- end generated -->

## The dependency structure

<!-- generated: deps -->
<!-- end generated -->

## How a strike travels

1. The AS3935 pulls GPIO21 high. `listen`'s notification wakes the loop.
2. `session::collect` reads the interrupt register over I2C and classifies it:
   lightning, disturber, or noise. `as3935` owns those registers; `strike` owns
   the values they decode to.
3. A lightning event goes to `merger`, which folds return strokes arriving
   inside the merge window into one flash. **Strokes are not strikes** — the
   tuner counts strokes as evidence, a person reads flashes.
4. The flash is scored by `history::score_milli`, pushed into the three rings
   (`history`) and appended to the CSV (`log`).
5. Noise and disturbers go to `tuning`, which every window decides whether the
   band is quiet enough to relax the noise floor or loud enough to raise it.
   `defence` holds the ladder; `verdict` decides what "quiet" means.
6. `screen` decides whether anything changed enough to be worth 3.8 s of panel
   time, and `ui` draws it.

At boot the same path runs backwards: `log::for_each` reads the CSV through
`csv`, and replays every record into the rings and the recent-strike table, so a
reboot does not empty the charts.

## Two interfaces, one command table

The console and the web UI are the same thing entered two ways. `console`
parses a line into a `Command`; `commands::run` turns it into an `Effects`
describing what should happen; `effects::handle` applies the parts that need
hardware, and hands the rest back to the loop, which owns the things a command
must not reach for itself — the `Portal`, the sensor, the log.

The web UI **composes a console command line** and queues it. It has no command
table of its own, no range checks of its own, and cannot express anything
`help` does not list. That is deliberate: two interfaces with two parsers drift,
and the second one is always the one that gets the range check wrong.

## Where to put a new thing

* **A decision, a rule, a piece of arithmetic** — a new pure module, plus a file
  in `tests/`. If it needs the device to test it, it is in the wrong place.
* **A register, a pin, a peripheral** — the module that already owns that
  hardware. It should expose values, not registers.
* **A console command** — a variant in `console::Command`, an arm in
  `commands::run`, and an arm in `effects::handle` if it needs hardware. Add it
  to `query::command_from_query`'s allow-list to reach it from the web UI, and
  to `doc/console.md`.
* **Anything drawn** — `ui`, and measure it first. The panel is 800x480 and the
  fonts are fixed-width, so a label's real width is arithmetic, not a guess.

## Regenerating this file

    tools/modules.py            # rewrite the generated regions
    tools/modules.py --check    # exit non-zero if it is out of date
"""


def render(path, regions):
    if path.exists():
        text = path.read_text()
    else:
        text = TEMPLATE
    for name, lines in regions.items():
        pattern = re.compile(
            r"(<!-- generated: %s -->\n).*?(<!-- end generated -->)" % name, re.S
        )
        if not pattern.search(text):
            print(f"warning: no `{name}` region in {path}", file=sys.stderr)
            continue
        body = "\n".join(lines)
        text = pattern.sub(lambda m: m.group(1) + body + "\n" + m.group(2), text)
    return text


def main():
    check = "--check" in sys.argv
    regions = build()
    targets = [
        (DOC, {k: v for k, v in regions.items() if k != "counts"}),
        (README, {"counts": regions["counts"]}),
    ]

    stale = 0
    for path, wanted in targets:
        rendered = render(path, wanted)
        if check:
            current = path.read_text() if path.exists() else ""
            if current != rendered:
                print(f"{path.relative_to(ROOT)} is out of date -- run tools/modules.py",
                      file=sys.stderr)
                stale += 1
            continue
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(rendered)
        print(f"wrote {path.relative_to(ROOT)}")

    if check:
        if stale:
            return 1
        print("doc/modules.md and README.md are up to date")
    return 0


if __name__ == "__main__":
    sys.exit(main())
