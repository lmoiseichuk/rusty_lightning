# Notice for these images

The images in this folder, `default/` and `no-light-sleep/`, are lightning's firmware: MIT,
Copyright (c) 2026 Leonid Moiseichuk, as [LICENSE](LICENSE) says. They also carry other people's
work, and **this folder holds every licence text that work asks to go with a copy**, so a copy of
`release/` needs nothing else from the repository:

| Where | What it holds |
|---|---|
| [LICENSE](LICENSE) | lightning's own licence |
| [default/licences/licences.md](default/licences/licences.md), [no-light-sleep/licences/licences.md](no-light-sleep/licences/licences.md) | each image's notice, generated from that image by garden's `tools/licences.py`: every Rust crate it links, the Rust standard library, ESP-IDF with the third-party components the image links, each with its licence, and the notices in the headers of the ESP-IDF and component sources it links |
| the rest of each `licences/` folder | the texts its notice names: copied from the crates (`crates/`), the toolchain (`rust/`), the ESP-IDF tree and the components the build fetched (`esp-idf/`), and the per-source notices (`esp-idf/sources/`); and a few the tool fills in where the tree ships no text, each marked as such in `licences.md` -- FreeBSD's net80211 header, and SPDX's BSD-3-Clause text with the header's holders for TLSF, the lwIP port's `sys_arch.c`, esp_netif's `wlanif.c`, and littlefs's `lfs_config.c` |
| this file | the texts that tool cannot collect, LittleFS's as well, and which logo each image draws |
| [garden/license](garden/license) | garden's licence, for garden's files under `garden/` |

**They go wherever the images go.** Whoever passes on an image, or a board that carries one,
passes on this file, `LICENSE` and that image's `licences/` with it. `tools/release.sh` copies
`LICENSE` and `garden/license` in, and makes an image's `licences/` each time it builds that image.
This file is written by hand.

## How each image's notice was made

Each `licences.md` names the commit and the `Cargo.lock` it was made from; the image's `VERSION`
names the same commit. From the next release on, `tools/release.sh` makes it straight after
building the image, with the same flags. The two here were made afterwards:

| Image | Version | Commit | Flags |
|---|---|---|---|
| `default/` | 0.22.4 | `53381997ffc6` | none |
| `no-light-sleep/` | 0.22.3 | `9f0c1676f4fc` | `--features no-light-sleep` |

On 2026-11-04 each commit was built again in a clean clone, with its image's flags, and
`licences.py` was run on the released image itself (`--image`), against that rebuild's ESP-IDF
build. Each rebuild matched its released image in every loaded section but one, ESP-IDF's
application descriptor, which holds the build time and a version string; every defined symbol was
at the same address. That is why each `licences.md` says the image's features are "not confirmed":
the tool cannot see how a released file was built, and the rebuild is what confirms it. The tool
did check each image's rustc and ESP-IDF version against the rebuild's, and each `licences.md`
says they matched. Each also names the garden commit the tool ran from. On that day the two
notices listed the same crates, components and texts.

## What that tool cannot collect

| What | In which image | Why it is here | Its text |
|---|---|---|---|
| DFRobot's AS3935 driver, ported to Rust in `src/as3935.rs` | both | it is in lightning's own source, so no crate carries it | [DFRobot's MIT licence](#dfrobots-as3935-driver) |
| The fonts `helvR14` and `helvB14` | both | from `u8g2-fonts`, which the tool flags: its licence is given only as a file, and that file leaves the fonts to U8g2's terms | [the X11 notice](#helvetica-helvr14-and-helvb14) |
| The font `fub14` | both | the same | [the font's copyright and the OFL](#freeuniversal-fub14) |
| `qrcodegen` 1.8.0 | both | flagged: it ships no licence file | [the notice in its source](#qrcodegen-180) |
| `void` 1.0.2 | both | flagged: it ships no licence file | [its repository's MIT text](#void-102) |
| The boot logo | each its own | lightning's own drawing in one image, a picture of unrecorded origin in the other | [below](#the-boot-logo) |

**LittleFS is carried here as well**, though the tool collects it now: the `joltwallet/littlefs`
1.22.3 component, in both images, fetched at build from Espressif's component registry. This file
took its texts in by hand when the tool collected nothing for a component fetched at build; each
`licences/esp-idf/` now holds the same two (`components/` and `sources/`). They are
[below](#littlefs): esp_littlefs's MIT and littlefs's BSD-3-Clause.

Nothing is owed for three more things, named anyway. The five fixed fonts from `embedded-graphics`
(6x10, 8x13, 9x15, 9x15 bold and 10x20) are public domain, as their BDF files' `COPYRIGHT` fields
say. Howard Hinnant's `civil_from_days`, reworked in `src/civil.rs`, is public domain by his own
dedication. The expansion of ESP-IDF's `WIFI_INIT_CONFIG_DEFAULT()` in `src/portal.rs` is ESP-IDF's
code, under the Apache-2.0 text in each `licences/esp-idf/LICENSE`.

## The boot logo

- **`default/`, 0.22.4,** draws `assets/logo.pbm` as it is since commit `d3e2f16b67b3`: a ginger cat
  in a felt hat, which that commit records as an original drawing made for these projects, under
  the project's MIT licence.
- **`no-light-sleep/`, 0.22.3,** still draws the earlier picture, a cartoon dog in a hat holding a
  cane. `d3e2f16b67b3` describes it as "taken from a picture on the internet, with no recorded
  author or terms", and replaced it. **Its author and terms are not known.** It stays in this image
  until the image is next built.

## The texts

### DFRobot's AS3935 driver

- **Source:** <https://github.com/DFRobot/DFRobot_AS3935>, the MicroPython library
  `MicroPython/DFRobot_AS3935_Lib.py` and its example
  `MicroPython/example/DFRobot_AS3935_detailed.py`, which `src/as3935.rs` ports to Rust.
- **Licence:** the library file carries no header of its own, so the repository's `LICENSE` is its
  licence: MIT since commit `ee688f17ce03` (2021-12-28), the GNU LGPL 2.1 before. Below as it stands
  at `1d974051b06a`. The example still carries its 2018 header, "Copyright GNU Lesser General Public
  License", with no version given.

```text
MIT License

Copyright (c) 2020 DFRobot

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

### Helvetica: helvR14 and helvB14

- **Source:** the X11 distribution's Adobe Helvetica bitmap fonts `HELVR14.BDF` and `HELVB14.BDF`,
  as U8g2 converts them and the `u8g2-fonts` crate carries them. That crate's own `LICENSE`, in each
  `licences/crates/u8g2-fonts-0.8.0/`, says the crate is "MIT OR Apache-2.0", that "The fonts,
  however, are licensed under different conditions", and sends the reader to U8g2's `LICENSE`.
- **Licence:** the notice U8g2's `LICENSE` gives for fonts derived from these files, below, at U8g2
  commit `d6c8499c5f27`
  (<https://github.com/olikraus/u8g2/blob/d6c8499c5f2707cac8eccd09fd8f677d12b17977/LICENSE>). It
  asks for the copyright notices in all copies, and for them and the permission notice in the
  supporting documentation, which this file is. `LICENSE` is byte-identical at `46b7f48bcb7b`, the
  U8g2 commit `u8g2-fonts` 0.8.0 converts its fonts from. Adobe's and Digital Equipment's names are
  used here for this attribution and nothing else.

```text
Copyright 1984-1989, 1994 Adobe Systems Incorporated.
Copyright 1988, 1994 Digital Equipment Corporation.

Adobe is a trademark of Adobe Systems Incorporated which may be
registered in certain jurisdictions.
Permission to use these trademarks is hereby granted only in
association with the images described in this file.

Permission to use, copy, modify, distribute and sell this software
and its documentation for any purpose and without fee is hereby
granted, provided that the above copyright notices appear in all
copies and that both those copyright notices and this permission
notice appear in supporting documentation, and that the names of
Adobe Systems and Digital Equipment Corporation not be used in
advertising or publicity pertaining to distribution of the software
without specific, written prior permission.  Adobe Systems and
Digital Equipment Corporation make no representations about the
suitability of this software for any purpose.  It is provided "as
is" without express or implied warranty.
```

### FreeUniversal: fub14

- **Source:** FreeUniversal Bold, version 1.001, as U8g2 converts it from
  `tools/font/ttf/FreeUniversal-Bold.ttf` (at `d6c8499c5f27`, and identical at the crate's
  `46b7f48bcb7b`) and `u8g2-fonts` carries it: rendered
  to a 14-pixel bitmap of the printable ASCII characters, which is a Modified Version in the OFL's
  sense. Its reserved font name, "SILSophia", is not used: the font is called `fub14`.
- **Licence:** the SIL Open Font License 1.1. It asks that each copy bundled with software contain
  the font's copyright notice and the licence. Both are below, copied from the font file's own
  copyright field (name ID 0) and licence field (name ID 13). The OFL covers the font; it does not
  extend to the firmware the font is bundled with.

The font's copyright field:

```text
FreeUniveral (c) Stephen Wilson 2009

Original Font Sil-Sophia
Copyright (c) SIL International, 1994-2008.
```

The font's licence field:

```text
Copyright (c) 2009, tempus,,, (<URL|email>),
with Reserved Font Name SILSophia.

This Font Software is licensed under the SIL Open Font License, Version 1.1.
This license is copied below, and is also available with a FAQ at:
http://scripts.sil.org/OFL


-----------------------------------------------------------
SIL OPEN FONT LICENSE Version 1.1 - 26 February 2007
-----------------------------------------------------------

PREAMBLE
The goals of the Open Font License (OFL) are to stimulate worldwide
development of collaborative font projects, to support the font creation
efforts of academic and linguistic communities, and to provide a free and
open framework in which fonts may be shared and improved in partnership
with others.

The OFL allows the licensed fonts to be used, studied, modified and
redistributed freely as long as they are not sold by themselves. The
fonts, including any derivative works, can be bundled, embedded, 
redistributed and/or sold with any software provided that any reserved
names are not used by derivative works. The fonts and derivatives,
however, cannot be released under any other type of license. The
requirement for fonts to remain under this license does not apply
to any document created using the fonts or their derivatives.

DEFINITIONS
"Font Software" refers to the set of files released by the Copyright
Holder(s) under this license and clearly marked as such. This may
include source files, build scripts and documentation.

"Reserved Font Name" refers to any names specified as such after the
copyright statement(s).

"Original Version" refers to the collection of Font Software components as
distributed by the Copyright Holder(s).

"Modified Version" refers to any derivative made by adding to, deleting,
or substituting -- in part or in whole -- any of the components of the
Original Version, by changing formats or by porting the Font Software to a
new environment.

"Author" refers to any designer, engineer, programmer, technical
writer or other person who contributed to the Font Software.

PERMISSION & CONDITIONS
Permission is hereby granted, free of charge, to any person obtaining
a copy of the Font Software, to use, study, copy, merge, embed, modify,
redistribute, and sell modified and unmodified copies of the Font
Software, subject to the following conditions:

1) Neither the Font Software nor any of its individual components,
in Original or Modified Versions, may be sold by itself.

2) Original or Modified Versions of the Font Software may be bundled,
redistributed and/or sold with any software, provided that each copy
contains the above copyright notice and this license. These can be
included either as stand-alone text files, human-readable headers or
in the appropriate machine-readable metadata fields within text or
binary files as long as those fields can be easily viewed by the user.

3) No Modified Version of the Font Software may use the Reserved Font
Name(s) unless explicit written permission is granted by the corresponding
Copyright Holder. This restriction only applies to the primary font name as
presented to the users.

4) The name(s) of the Copyright Holder(s) or the Author(s) of the Font
Software shall not be used to promote, endorse or advertise any
Modified Version, except to acknowledge the contribution(s) of the
Copyright Holder(s) and the Author(s) or with their explicit written
permission.

5) The Font Software, modified or unmodified, in part or in whole,
must be distributed entirely under this license, and must not be
distributed under any other license. The requirement for fonts to
remain under this license does not apply to any document created
using the Font Software.

TERMINATION
This license becomes null and void if any of the above conditions are
not met.

DISCLAIMER
THE FONT SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND,
EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO ANY WARRANTIES OF
MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT
OF COPYRIGHT, PATENT, TRADEMARK, OR OTHER RIGHT. IN NO EVENT SHALL THE
COPYRIGHT HOLDER BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY,
INCLUDING ANY GENERAL, SPECIAL, INDIRECT, INCIDENTAL, OR CONSEQUENTIAL
DAMAGES, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING
FROM, OUT OF THE USE OR INABILITY TO USE THE FONT SOFTWARE OR FROM
OTHER DEALINGS IN THE FONT SOFTWARE.
```

### qrcodegen 1.8.0

- **Source:** Project Nayuki's QR Code generator library,
  <https://github.com/nayuki/QR-Code-generator>.
- **Licence:** MIT, as the crate declares. It ships no licence file; its notice is the header of
  its `src/lib.rs`, below with the comment's leading ` * ` taken off.

```text
QR Code generator library (Rust)

Copyright (c) Project Nayuki. (MIT License)
https://www.nayuki.io/page/qr-code-generator-library

Permission is hereby granted, free of charge, to any person obtaining a copy of
this software and associated documentation files (the "Software"), to deal in
the Software without restriction, including without limitation the rights to
use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of
the Software, and to permit persons to whom the Software is furnished to do so,
subject to the following conditions:
- The above copyright notice and this permission notice shall be included in
  all copies or substantial portions of the Software.
- The Software is provided "as is", without warranty of any kind, express or
  implied, including but not limited to the warranties of merchantability,
  fitness for a particular purpose and noninfringement. In no event shall the
  authors or copyright holders be liable for any claim, damages or other
  liability, whether in an action of contract, tort or otherwise, arising from,
  out of or in connection with the Software or the use or other dealings in the
  Software.
```

### void 1.0.2

- **Source:** `void`, <https://github.com/reem/rust-void>, by Jonathan Reem, as its manifest names
  him.
- **Licence:** the crate declares MIT, and carries neither a licence text nor a copyright line; its
  repository's `1.0.2` tag (`ab2f4dbb7c95`) has neither. The repository's `LICENSE-MIT`, below, came
  with a pull request merged after that tag, in `a6e061227f47` (2016-04-18), whose manifest gives
  version 1.0.2 the licence "MIT / Apache-2.0". The crate's one source file, `src/lib.rs`, is the
  same byte for byte at the tag, at that merge and at the repository's head.

```text
Copyright (c) 2015 The rust-void Developers

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

### LittleFS

- **Source:** the ESP-IDF component `joltwallet/littlefs` 1.22.3, as `components_esp32c3.lock` pins
  it, fetched at build: Brian Pugh's `esp_littlefs` (<https://github.com/joltwallet/esp_littlefs>,
  commit `bde79e13be97`), around the littlefs project's `littlefs` 2.11
  (<https://github.com/littlefs-project/littlefs>).
- **Licence:** MIT for `esp_littlefs`, its `LICENSE`; BSD-3-Clause for `littlefs`, its
  `src/littlefs/LICENSE.md`. The BSD licence asks for its notice in the documentation that goes
  with a binary, which this file is. Both below, as the builds of both images fetched them.

```text
Copyright 2020 Brian Pugh

Permission is hereby granted, free of charge, to any person obtaining a copy of this software and associated documentation files (the "Software"), to deal in the Software without restriction, including without limitation the rights to use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of the Software, and to permit persons to whom the Software is furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.
```

```text
Copyright (c) 2022, The littlefs authors.  
Copyright (c) 2017, Arm Limited. All rights reserved.

Redistribution and use in source and binary forms, with or without modification,
are permitted provided that the following conditions are met:

-  Redistributions of source code must retain the above copyright notice, this
   list of conditions and the following disclaimer.
-  Redistributions in binary form must reproduce the above copyright notice, this
   list of conditions and the following disclaimer in the documentation and/or
   other materials provided with the distribution.
-  Neither the name of ARM nor the names of its contributors may be used to
   endorse or promote products derived from this software without specific prior
   written permission.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS" AND
ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE IMPLIED
WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE LIABLE FOR
ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES
(INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES;
LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON
ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT
(INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE OF THIS
SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
```

## The rest of the repository

The repository's own notice, `notice.md` at its root
(<https://github.com/lmoiseichuk/rusty_lightning/blob/main/notice.md>), names everything in the
repository that is other people's work, with its history, including what is in no image.
