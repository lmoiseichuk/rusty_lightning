# Notice

lightning's own code is MIT-licensed ([LICENSE](LICENSE)), and it is mine: every commit in this
repository is authored by me (checked 2026-11-04), and most also carry a `Co-Authored-By` line for
Claude, the AI assistant they were written with. Some things in it, and more in the firmware
images built from it, are other people's work. This file names each one: where it came from, its
licence, and the attribution it asks for.

**The firmware images carry most of them, and `release/` carries the texts they owe.** Beside each
image, its `licences/` folder holds a notice generated from the image by garden's
`tools/licences.py`, with the licence texts of every crate, the Rust standard library and the
ESP-IDF components it links. [release/notice.md](release/notice.md) holds, once, the texts that
cannot be generated -- DFRobot's, the fonts', qrcodegen's and void's -- and LittleFS's, which each
`licences/` folder now holds as well. `tools/release.sh`
copies [LICENSE](LICENSE) in beside them. So a copy of `release/` carries every notice its images
need without this file, which names the same things with their history, and what is in no image.

| What | Where here | Source | Licence | In a firmware image |
|---|---|---|---|---|
| The AS3935 register driver, ported to Rust | `src/as3935.rs` | DFRobot's `DFRobot_AS3935`: the MicroPython library and its example | MIT, DFRobot 2020; the example's own header says LGPL | yes |
| Modified copies of that library and example | `doc/experiments/DFRobot_AS3935_Lib.py`, `doc/experiments/deep_demo.py` | the same two DFRobot files | their headers say LGPL; **open**, see below | no |
| Helvetica 14, regular and bold (`helvR14`, `helvB14`) | `src/ui.rs`, through the `u8g2-fonts` crate | the X11 distribution's Adobe fonts, as U8g2 converts them | the X11 Adobe/DEC permission notice | yes |
| FreeUniversal Bold 14 (`fub14`) | `src/ui.rs`, through `u8g2-fonts` | Stephen Wilson, after SIL International's Sophia, as U8g2 converts it | SIL Open Font License 1.1 | yes |
| Five fixed fonts: 6x10, 8x13, 9x15, 9x15 bold, 10x20 | `src/ui.rs`, through `embedded-graphics` | the X11 "misc-fixed" fonts, in Markus Kuhn's ucs-fonts | public domain | yes |
| The civil-date algorithm | `src/civil.rs` | Howard Hinnant, `civil_from_days` | public domain, by his own dedication | yes |
| The QR code generator | the `qrcodegen` crate, called from `src/ui.rs` | Project Nayuki | MIT | yes |
| `WIFI_INIT_CONFIG_DEFAULT()`, expanded by hand | `src/portal.rs`, `default_wifi_init_config` | ESP-IDF v5.3.4, `components/esp_wifi/include/esp_wifi.h` | Apache-2.0 | yes |
| ESP-IDF, with its Wi-Fi and PHY libraries | not in the repository: built in from v5.3.4 | Espressif | Apache-2.0 | yes: see [ESP-IDF](#esp-idf) |
| LittleFS | not in the repository: the `joltwallet/littlefs` component, fetched at build | Brian Pugh's `esp_littlefs`, around the littlefs project's `littlefs` | MIT, and BSD-3-Clause | yes |
| The C6 reflash program | `station/c6_reflash/main.c` | Espressif's esp-serial-flasher, `examples/esp32_sdio_example` | Unlicense OR CC0-1.0 | no: a one-off bench program |
| The Rust template | `.gitignore`, lines 1-21 | GitHub's gitignore templates, `Rust.gitignore` | CC0 1.0 | no |
| Copies of five of garden's files | `release/garden/` | garden, my own project | MIT | no: host tools and its licence |
| The boot logo, since `d3e2f16b67b3` | `assets/logo.pbm` | drawn for these projects | MIT, as the project | yes: the `default` image, 0.22.4 |
| The earlier boot logo | `assets/logo.pbm` until `d3e2f16b67b3`, and in the history | **not recorded** | **not recorded** | yes: the `no-light-sleep` image, 0.22.3; see [Origin to be confirmed](#origin-to-be-confirmed) |

## DFRobot's AS3935 driver

- **Source:** DFRobot, <https://github.com/DFRobot/DFRobot_AS3935>:
  `MicroPython/DFRobot_AS3935_Lib.py` and `MicroPython/example/DFRobot_AS3935_detailed.py`. Both
  were added in commit `70efa9f99d96` (2019-08-18) and neither has changed since (checked at
  `1d974051b06a`, the repository's head on 2026-11-04).
- **What is here:** `src/as3935.rs` is a port to Rust of the library's register map, its start-up
  sequence and every operation it has, and of the example's interrupt handling. Its comments quote
  a few lines of both: the library's wait in `getInterruptSrc`, and the example's interrupt
  handler.
- **Licence:** the repository's `LICENSE` is MIT, "Copyright (c) 2020 DFRobot", since V1.0.2
  (commit `ee688f17ce03`, 2021-12-28). Before that, from 2018-11-13, it was the GNU LGPL 2.1, and
  it still was when the MicroPython files were published. The library file has never carried a
  header of its own, so the repository's licence is its licence. The example still carries its
  2018 header, with no LGPL version given:

  ```text
  # Copyright    [DFRobot](http://www.dfrobot.com), 2018
  # Copyright    GNU Lesser General Public License
  ```
- **Attribution:** DFRobot's copyright notice and permission notice in every copy or substantial
  portion. The text is in [release/notice.md](release/notice.md#dfrobots-as3935-driver), which goes
  with the images. `src/as3935.rs` names its source in its header.

### The two DFRobot files under doc/experiments/

- **What they are:** modified copies of the same two files. `DFRobot_AS3935_Lib.py` is the
  library; `deep_demo.py` is DFRobot's `DFRobot_AS3935_detailed.py`, with other pins and imports and
  a noise-level tuner added. They were the behavioural specification before `src/as3935.rs` existed.
  Added under `doc/` in `876a6cf3a7a3` (2026-08-02), they were deleted in `a4e0b42bd67c` the same
  day, which ported what was left in them. `2b3e24bfdcac` (2026-08-06) added them again, unchanged,
  at the repository's root, and `e48bcc84f9c9` (2026-08-28) moved them, unchanged, into
  `doc/experiments/` as early starting code. `doc/specs.md` section 8 and `src/as3935.rs` still say
  they were deleted.
- **What their headers say:**
  - `DFRobot_AS3935_Lib.py`, lines 6-7: the two lines above, then version V1.0, date 2018-11-28,
    and at line 12 "Modified by Leonid Moiseichuk to minimize print() calls and float operations,
    add checks". DFRobot's own library file has never carried this header. It is the example's
    header with the file name changed, the example's six lines describing the sensor (its range,
    its indoor and outdoor modes, its three I2C addresses) left out, and the "Modified by" line
    added.
  - `deep_demo.py`, lines 12-13: the same two lines, exactly as DFRobot's example has them today.
    It does not mark its changes.
- **Where they stand:** neither is in a firmware image: they are MicroPython, and nothing here
  builds them. This repository's `LICENSE` says MIT, these two say LGPL, and no LGPL text is
  carried. **What to do with them is my decision, and it is still open.** The library copy could
  take DFRobot's present MIT notice, since DFRobot's library file has no header and its repository
  is MIT. `deep_demo.py` could be deleted, or kept with the LGPL 2.1 text beside it. Either way,
  neither file is to move into garden.

## Fonts

The glass is drawn in three U8g2 fonts, through the `u8g2-fonts` crate, and five of
`embedded-graphics`' fixed fonts. `u8g2-fonts` is "MIT OR Apache-2.0" for its own code, but its
`LICENSE` says "The fonts, however, are licensed under different conditions" and sends the reader
to U8g2's `LICENSE`, which sends them on to its wiki for each font's terms. Those terms are named
below, font by font; their texts are in [release/notice.md](release/notice.md).

### Helvetica: helvR14 and helvB14

- **Source:** the X11 distribution's Adobe Helvetica bitmap fonts, `HELVR14.BDF` and `HELVB14.BDF`,
  as U8g2 converts them (<https://github.com/olikraus/u8g2/wiki/fntgrpadobex11>). The font files'
  own copyright string, as U8g2's wiki gives it: "Copyright (c) 1984, 1987 Adobe Systems
  Incorporated. All Rights Reserved. Copyright (c) 1988, 1991 Digital Equipment Corporation. All
  Rights Reserved."
- **Licence:** the X11 notice, as U8g2's `LICENSE` gives it for these files (U8g2 at
  `d6c8499c5f27`,
  <https://github.com/olikraus/u8g2/blob/d6c8499c5f2707cac8eccd09fd8f677d12b17977/LICENSE>; the
  file is byte-identical at `46b7f48bcb7b`, the U8g2 commit `u8g2-fonts` 0.8.0 converts its fonts
  from).
- **Attribution:** the copyright notices in all copies, and the copyright notices with the
  permission notice in the supporting documentation, which
  [release/notice.md](release/notice.md#helvetica-helvr14-and-helvb14) is: the notice is there, and
  goes with the images. Adobe's and Digital Equipment's names are not used to advertise anything
  here.

### FreeUniversal: fub14

- **Source:** FreeUniversal Bold, version 1.001, by Stephen Wilson (2009), after SIL International's
  Sophia. U8g2 converts it from `tools/font/ttf/FreeUniversal-Bold.ttf`
  (<https://github.com/olikraus/u8g2/wiki/fntgrpfreeuniversal>), and names the Open Font Library
  as where it was downloaded from.
- **Licence:** the SIL Open Font License, Version 1.1, as the font file's own licence field states.
- **Attribution:** the OFL asks that every copy bundled with software contain the font's copyright
  notice and the licence, "as stand-alone text files, human-readable headers or in the appropriate
  machine-readable metadata fields". Both are in
  [release/notice.md](release/notice.md#freeuniversal-fub14), copied from the font file's own
  copyright and licence fields, and go with the images. The reserved font name is "SILSophia",
  which the converted font does not use: U8g2 calls it `fub14`. The OFL keeps the font itself under
  the OFL; that does not extend to the firmware it is bundled with, nor to anything drawn in it.
- **What was done to it:** U8g2 rendered it to a 14-pixel bitmap and kept the printable ASCII
  characters (the `_tr` in its name). That is a Modified Version in the OFL's sense. This firmware
  draws it as `u8g2-fonts` carries it.

### embedded-graphics' fixed fonts

- **Source:** `FONT_6X10`, `FONT_8X13`, `FONT_9X15`, `FONT_9X15_BOLD` and `FONT_10X20` from
  `embedded-graphics` 0.8.2. They are the X11 "misc-fixed" fonts, from Markus Kuhn's ucs-fonts;
  the crate ships their BDF sources.
- **Licence:** public domain. Each BDF file's `COPYRIGHT` field says "Public domain font. Share and
  enjoy." (6x10: "Public domain terminal emulator font. Share and enjoy.").
- **Attribution:** none required. Given anyway: *fixed fonts from the X11 misc-fixed set, by way of
  Markus Kuhn's ucs-fonts.*

## Howard Hinnant's date algorithms

- **Source:** Howard Hinnant, "chrono-Compatible Low-Level Date Algorithms",
  <https://howardhinnant.github.io/date_algorithms.html>: `civil_from_days`.
- **What is here:** `src/civil.rs`, reworked from it in Rust: a Unix timestamp to a calendar
  date, which dates every row of the strike log. The file's header names `days_from_civil` too;
  nothing here implements that one.
- **Licence:** public domain, by his own words on that page: "Consider these donated to the public
  domain."
- **Attribution:** none required. Given anyway, in the header of `src/civil.rs`.

## The QR code generator

- **Source:** the `qrcodegen` crate, 1.8.0, by Project Nayuki
  (<https://www.nayuki.io/page/qr-code-generator-library>,
  <https://github.com/nayuki/QR-Code-generator>). It makes the two QR codes the panel shows while
  the access point is up.
- **Licence:** MIT, as the crate's manifest and its source header state.
- **Attribution:** the notice from the crate's `src/lib.rs`, in all copies or substantial portions,
  which a firmware image linking it is. The crate ships no licence file, so garden's `licences.py`
  flags it; the notice is in [release/notice.md](release/notice.md#qrcodegen-180), and goes with
  the images.

## ESP-IDF

- **What is in the images:** ESP-IDF v5.3.4 (`ESP_IDF_VERSION` in `.cargo/config.toml`; both
  images carry the version string), with its Wi-Fi and PHY binary libraries. It is not in this
  repository: the build uses an installed copy, or fetches one.
- **What is in the repository:** one expansion of the `WIFI_INIT_CONFIG_DEFAULT()` macro from
  `components/esp_wifi/include/esp_wifi.h`, filled in by hand in `src/portal.rs`, because a C macro
  has no Rust binding. It is a list of a structure's field values.
- **Licence:** the Apache License 2.0. The binary libraries "are provided under the same license as
  the parent esp-idf project - the Apache License 2.0", their `README.rst` says. ESP-IDF links
  third-party libraries under their own licences as well.
- **Attribution:** section 4(a) of the licence asks for a copy of it with every copy of the work,
  which the images are. Each image's `licences/esp-idf/` holds ESP-IDF's `LICENSE` and the licence
  texts of the third-party components that image links, which garden's `licences.py` finds by the
  image's own symbols, and under `sources/` the notices in the headers of the sources it links;
  that image's `licences/licences.md` lists them
  ([default](release/default/licences/licences.md),
  [no-light-sleep](release/no-light-sleep/licences/licences.md)).

## LittleFS

- **Source:** the ESP-IDF component `joltwallet/littlefs` 1.22.3 (`components_esp32c3.lock`),
  fetched at build: Brian Pugh's `esp_littlefs` (<https://github.com/joltwallet/esp_littlefs>),
  around the littlefs project's `littlefs` 2.11 (<https://github.com/littlefs-project/littlefs>).
  It holds the strike log.
- **Licence:** MIT for `esp_littlefs`; BSD-3-Clause for `littlefs`.
- **Attribution:** the MIT notice in all copies, and the BSD notice "in the documentation and/or
  other materials provided with the distribution" of a binary. Both are in
  [release/notice.md](release/notice.md#littlefs), from the component as the build fetched it;
  `esp_littlefs`'s matches its `v1.22.3` tag upstream. They were carried there by hand because
  garden's `licences.py` collected no text for a component fetched at build; it now does, so each
  image's `licences/esp-idf/` holds the same two texts as well, with the notice in the header of
  littlefs's `lfs_config.c`, and its `licences.md` names the version and commit fetched.

## esp-serial-flasher's SDIO example

- **Source:** Espressif's esp-serial-flasher, `examples/esp32_sdio_example/main/main.c` at commit
  `ddf3cdd8fa87` (2026-10-22), <https://github.com/espressif/esp-serial-flasher>.
- **What is here:** `station/c6_reflash/main.c`, reworked from it, which rewrote the FireBeetle's
  ESP32-C6 from its P4 once. Its README's steps build it with the flasher library and esp-hosted's
  slave firmware; neither of those is in this repository.
- **Licence:** "SPDX-FileCopyrightText: 2026 Espressif Systems (Shanghai) CO LTD",
  "SPDX-License-Identifier: Unlicense OR CC0-1.0": "This example code is in the Public Domain (or
  CC0 licensed, at your option.)". The flasher library itself is Apache-2.0.
- **Attribution:** none required. Given anyway, in the file's header.

## The station

`station/` has no released image yet. It builds against ESP-IDF v5.5.5 and four Espressif
components fetched at build, `esp_wifi_remote` 0.15.2, `esp_hosted` 2.7.0, `eppp_link` 1.1.6 and
`esp_serial_slave_link` 1.1.2, each Apache-2.0 by its own `LICENSE`, and crates from crates.io.
When it has an image, that image gets its own `licences/` beside it, generated by garden's
`licences.py` as the terminal's images' are.

## The .gitignore template

- **Source:** lines 1-21 of `.gitignore` are an earlier version of GitHub's `Rust.gitignore`,
  <https://github.com/github/gitignore/blob/main/Rust.gitignore>.
- **Licence:** CC0 1.0, the templates repository's licence. No attribution required.

## garden

`garden/` is a submodule, and `release/garden/` holds copies of five of its files
(`tools/board.sh`, `tools/board.py`, `tools/portlock.py`, `devices.list.example`
and its `license`) that `tools/release.sh` puts there.
garden is my own project, MIT, with its own notice:
<https://github.com/lmoiseichuk/garden/blob/main/notice.md>.

## Looked for and not here

- **Waveshare's driver code or tables:** none. The panel is driven by the `epd-waveshare` crate
  (ISC, Christoph Groß), whose `Epd7in5_V2` follows Waveshare's own drivers; this repository adds no
  Waveshare code and no look-up table. The micropython-waveshare driver linked from `doc/specs.md`
  is cited as a reference only.
- **Colour data:** none.
- **Files other people contributed:** none. Every commit here is authored by me.
- **coludo:** nothing copied. `station/Cargo.toml`, `station/.cargo/config.toml` and
  `doc/station/readme.md` mention it.
- **Facts and short quotations** from the AS3935 datasheet, Espressif's documentation and DFRobot's
  wiki are cited where they are used.
- **The photographs** (`doc/*.jpg`) are mine, and the station's drawings (`doc/station/*.svg`) are
  drawn by `doc/station/figures.py`.

## Origin to be confirmed

### The earlier boot logo

- **What it is:** a cartoon dog in a hat, holding a cane, 480 by 480 pixels: the picture
  `src/ui.rs`'s `logo` drew at a cold boot from `b2918cdbbf75` (2026-08-02) until `d3e2f16b67b3`
  (2026-11-04) replaced `assets/logo.pbm` with a drawing that commit records as original, made for
  these projects, under the project's MIT licence. `d3e2f16b67b3` describes the earlier picture as
  "taken from a picture on the internet, with no recorded author or terms".
- **Where it is still:** in the `no-light-sleep` image, 0.22.3, built before the change, and in the
  repository's history. The `default` image, 0.22.4, draws the new drawing.
- **Where it came from:** `b2918cdbbf75` says the assets were carried over from the moisture
  project. moisture's first logo, from 2026-07-29, was a different file, 200 by 200; its
  `src/mcp/assets/logo.pbm` was this one, byte for byte, from `42442479d622` (2026-08-15) until
  moisture replaced it too, in `c7f7aeaa1416`.
- **What is not recorded:** who drew it, and on what terms. Nothing in either repository says.
  While an image or the history carries it, this entry stays open.
