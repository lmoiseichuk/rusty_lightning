#!/usr/bin/env bash
# Cut and restore USB power to one of lightning's boards, addressed by its MAC
# -- garden's `tools/power.sh`, from this repository's `garden` submodule. A
# board not on lightning's `devices.list`, or flagged `forbidden` there, is
# refused.
#
#   tools/power.sh list                       # lightning's boards, present or not
#   tools/power.sh cycle <name|MAC>           # a cut, checked against the list
#
# **The last lever.** Light sleep powers down the USB Serial/JTAG PHY, so a
# board running the ordinary build appears and disappears in windows -- and
# when it stops appearing at all, `espflash` cannot connect, the console cannot
# attach, and there is no reset line. Cutting VBUS is what is left.
#
# `exec` replaces this shell with garden's script, so its exit status is this
# script's. See garden's `tools/power.sh` for why it addresses boards by MAC and
# never by port, and garden's `tools/board.sh` for what it refuses.
exec "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/../garden/tools/power.sh" "$@"
