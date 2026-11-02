#!/usr/bin/env python3
"""Draw the station proposal's pictures: doc/station/*.svg.

    python3 doc/station/figures.py    # writes the SVGs beside this file

The pictures are generated, not drawn, for the same reason the schematics in the
moisture project are: a drawing is a second copy of facts that live in the text,
and a generated one can be checked. Every label is measured with the real font
(Pillow and DejaVu Sans) and the script refuses to write a picture in which a
label overflows its box, two labels overlap, or a label leaves the canvas. The
receiver coverage curves and their worst-case numbers are computed here from
the rod geometry, and the thunder delays from the speed of sound -- not typed in.

Do not hand-edit the SVGs; the next run overwrites them.
"""

import math
from pathlib import Path

from PIL import ImageFont

HERE = Path(__file__).resolve().parent

FONT_FAMILY = "DejaVu Sans, Verdana, sans-serif"
REGULAR = "DejaVuSans.ttf"
BOLD = "DejaVuSans-Bold.ttf"

INK = "#1f2933"
MUTED = "#52606d"

# One palette per concern, the same in every picture: (fill, stroke).
STATION = ("#e3f0fb", "#2f6fae")
LIGHT = ("#fff6d6", "#b7791f")
SOUND = ("#e6f6ea", "#2f855a")
POWER = ("#fde8e1", "#c05621")
RADIO = ("#efe7fa", "#6b46c1")
TIME = ("#e0f4f4", "#2c7a7b")
HOUSE = ("#f2f4f6", "#7b8794")
PLAIN = ("#ffffff", "#7b8794")

_fonts = {}


def text_width(text, size, bold=False):
    """The rendered width of `text` in pixels, from the font file itself."""
    key = (size, bold)
    if key not in _fonts:
        _fonts[key] = ImageFont.truetype(BOLD if bold else REGULAR, size)
    return _fonts[key].getlength(text)


def escape(text):
    return text.replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;")


class Figure:
    def __init__(self, name, width, height):
        self.name = name
        self.width = width
        self.height = height
        self.parts = []
        # Every label's bounding box, for the overlap check at save time.
        self.labels = []

    # -- primitives -------------------------------------------------------

    def rect(self, x, y, w, h, palette, radius=8, dashed=False, stroke_width=1.6):
        fill, stroke = palette
        dash = ' stroke-dasharray="6 4"' if dashed else ""
        self.parts.append(
            f'<rect x="{x:.1f}" y="{y:.1f}" width="{w:.1f}" height="{h:.1f}" rx="{radius}" '
            f'fill="{fill}" stroke="{stroke}" stroke-width="{stroke_width}"{dash}/>'
        )

    def text(self, x, y, text, size=12, bold=False, anchor="start", colour=INK, check=True, halo=False):
        width = text_width(text, size, bold)
        if anchor == "start":
            left = x
        elif anchor == "middle":
            left = x - width / 2
        else:
            left = x - width
        weight = ' font-weight="bold"' if bold else ""
        # paint-order draws the white stroke first, so it clears the lines
        # under the label without eating into the glyphs.
        outline = ' stroke="#ffffff" stroke-width="3" paint-order="stroke"' if halo else ""
        self.parts.append(
            f'<text x="{x:.1f}" y="{y:.1f}" font-size="{size}"{weight} text-anchor="{anchor}" '
            f'fill="{colour}"{outline}>{escape(text)}</text>'
        )
        # Baseline at y: the glyphs reach about 0.8 of the size above it and 0.25 below.
        box = (left, y - size * 0.8, left + width, y + size * 0.25, text)
        if check:
            self.labels.append(box)
        return box

    def line(self, points, colour=MUTED, width=1.6, dashed=False, arrow_end=False, arrow_start=False):
        coordinates = " ".join(f"{x:.1f},{y:.1f}" for x, y in points)
        dash = ' stroke-dasharray="6 4"' if dashed else ""
        self.parts.append(
            f'<polyline points="{coordinates}" fill="none" stroke="{colour}" '
            f'stroke-width="{width}" stroke-linejoin="round"{dash}/>'
        )
        if arrow_end:
            self.arrowhead(points[-2], points[-1], colour)
        if arrow_start:
            self.arrowhead(points[1], points[0], colour)

    def arrowhead(self, start, end, colour):
        """A filled triangle at `end`, pointing away from `start`."""
        angle = math.atan2(end[1] - start[1], end[0] - start[0])
        length, half = 10, 4.5
        back = (end[0] - length * math.cos(angle), end[1] - length * math.sin(angle))
        left = (back[0] + half * math.sin(angle), back[1] - half * math.cos(angle))
        right = (back[0] - half * math.sin(angle), back[1] + half * math.cos(angle))
        coordinates = " ".join(f"{x:.1f},{y:.1f}" for x, y in (end, left, right))
        self.parts.append(f'<polygon points="{coordinates}" fill="{colour}"/>')

    def raw(self, svg):
        self.parts.append(svg)

    # -- composites -------------------------------------------------------

    def box(self, x, y, w, title, lines=(), palette=STATION, dashed=False, height=None):
        """A rounded box with a bold title and smaller lines under it.

        The height follows the number of lines unless given. Raises if any line
        is wider than the box, so a long label is caught here and not on the glass.
        """
        title_size, line_size, padding, pitch = 14, 12, 12, 18
        needed = 30 + pitch * len(lines) + 4
        if height is None:
            height = needed
        if height < needed:
            raise ValueError(f"{self.name}: box '{title}' needs {needed} px of height, has {height}")
        for text, size, bold in [(title, title_size, True)] + [(line, line_size, False) for line in lines]:
            room = w - 2 * padding
            if text_width(text, size, bold) > room:
                raise ValueError(
                    f"{self.name}: '{text}' is {text_width(text, size, bold):.0f} px in a {room:.0f} px box"
                )
        self.rect(x, y, w, height, palette, dashed=dashed)
        # Box text is fenced by the width check above, so it is kept out of the
        # overlap check, which is for free-standing labels.
        self.text(x + padding, y + 22, title, title_size, bold=True, colour=palette[1], check=False)
        for index, line in enumerate(lines):
            self.text(x + padding, y + 22 + pitch * (index + 1), line, line_size, check=False)
        return {
            "left": (x, y + height / 2),
            "right": (x + w, y + height / 2),
            "top": (x + w / 2, y),
            "bottom": (x + w / 2, y + height),
            "x": x,
            "y": y,
            "w": w,
            "h": height,
        }

    # -- output -----------------------------------------------------------

    def check(self):
        for left, top, right, bottom, text in self.labels:
            if left < 2 or top < 2 or right > self.width - 2 or bottom > self.height - 2:
                raise ValueError(f"{self.name}: '{text}' leaves the canvas")
        for index, first in enumerate(self.labels):
            for second in self.labels[index + 1:]:
                apart = first[2] <= second[0] or second[2] <= first[0] or first[3] <= second[1] or second[3] <= first[1]
                if not apart:
                    raise ValueError(f"{self.name}: '{first[4]}' overlaps '{second[4]}'")

    def save(self):
        self.check()
        header = (
            f'<svg xmlns="http://www.w3.org/2000/svg" width="{self.width}" height="{self.height}" '
            f'viewBox="0 0 {self.width} {self.height}" font-family="{FONT_FAMILY}">\n'
            f"<!-- Generated by doc/station/figures.py. Do not edit; the next run overwrites it. -->\n"
            f'<rect width="{self.width}" height="{self.height}" fill="#ffffff"/>\n'
        )
        path = HERE / f"{self.name}.svg"
        path.write_text(header + "\n".join(self.parts) + "\n</svg>\n")
        return path


# -- the receivers' geometry -------------------------------------------------


def rod_response(rod_degrees, bearing_degrees):
    """How much of a distant strike's field one ferrite rod reads, 0 to 1.

    A distant return stroke's magnetic field is horizontal and square to the
    bearing; the rod reads only the part along its own axis. A rod pointing
    straight at the strike therefore reads nothing: its null.
    """
    return abs(math.sin(math.radians(bearing_degrees - rod_degrees)))


def best_response(rods, bearing_degrees):
    return max(rod_response(rod, bearing_degrees) for rod in rods)


def decibels(amplitude):
    if amplitude <= 0:
        return -math.inf
    return 20 * math.log10(amplitude)


def coverage(rods):
    """(worst bearing in dB, share of the horizon 6 dB or more down), over a 0.1 degree sweep."""
    samples = [best_response(rods, step / 10) for step in range(3600)]
    worst = decibels(min(samples))
    six_down = sum(1 for sample in samples if decibels(sample) <= -6) / len(samples)
    return worst, six_down


ROD_SETS = [
    ("1 board", [0], "#9aa5b1", 2.0),
    ("2 boards", [0, 90], "#c05621", 2.0),
    ("4 boards", [0, 45, 90, 135], "#2f6fae", 3.0),
]


# -- the speed of sound --------------------------------------------------------


def speed_of_sound(celsius):
    return 331.3 + 0.606 * celsius


# -- the pictures ----------------------------------------------------------------


def overview():
    figure = Figure("overview", 1040, 570)
    figure.text(20, 30, "The station listens on the roof; the panel shows, logs and relays", 16, bold=True)

    figure.rect(20, 50, 250, 500, HOUSE, radius=12, dashed=True)
    figure.text(36, 76, "House", 14, bold=True, colour=HOUSE[1])
    panel = figure.box(
        40, 245, 210, "Panel",
        ["XIAO ESP32-C3, 7.5\" e-paper", "screen, CSV log, console", "its SEN0290: indoor tests"],
        PLAIN,
    )

    figure.rect(300, 50, 720, 500, HOUSE, radius=12, dashed=True)
    figure.text(316, 76, "Roof: low, away from the ridge, white ASA box", 14, bold=True, colour=HOUSE[1])

    fallback = figure.box(320, 95, 170, "ESP32-C3 Pro", ["fallback radio", "antenna socket"], RADIO, dashed=True)
    receivers = figure.box(
        510, 95, 240, "4 × SEN0290 (AS3935)", ["rods at 0°, 45°, 90°, 135°", "one IRQ line each"], STATION
    )
    core = figure.box(
        510, 245, 240, "FireBeetle 2 ESP32-P4",
        ["P4: receivers, ring, time, log", "on-board C6: the radio", "TF card: the full log"],
        STATION,
    )
    ring = figure.box(
        820, 245, 180, "8 flash sensors",
        ["ESP32-C3 each", "TEMT6000 + MAX4466", "sleep, wake on a flash"],
        LIGHT,
    )
    time = figure.box(
        320, 405, 230, "Time and weather",
        ["ATGM336H-5N-31 with PPS", "DS3231: no satellites", "SHT45: speed of sound"],
        TIME,
    )
    power = figure.box(
        590, 405, 410, "Power",
        [
            "5–6 V solar panel → 45 °C thermostat → CN3065",
            "5000 mAh Li-ion, two NTCs, MAX17043 gauge",
            "3.3 V regulator: receivers, sensors",
            "5 V boost at 1 MHz or more: the FireBeetle",
        ],
        POWER,
    )

    # Receivers to the core.
    figure.line([(630, receivers["y"] + receivers["h"]), (630, core["y"])], STATION[1], arrow_end=True)
    figure.text(640, 205, "I2C A (3 boards), I2C B (1), 4 IRQs", 12, colour=STATION[1])
    # The fallback radio, dashed: only if the on-board C6 falls short.
    figure.line(
        [(405, fallback["y"] + fallback["h"]), (405, 268), (core["x"], 268)], RADIO[1], dashed=True
    )
    figure.text(413, 258, "UART", 12, colour=RADIO[1])
    # The link to the panel.
    link_y = 305
    figure.line([(panel["x"] + panel["w"], link_y), (core["x"], link_y)], RADIO[1], width=2.4,
                arrow_end=True, arrow_start=True)
    figure.text(380, link_y - 9, "Wi-Fi or ESP-NOW", 12, bold=True, anchor="middle", colour=RADIO[1])
    figure.text(380, link_y + 20, "no wire to the house", 12, anchor="middle", colour=RADIO[1])
    # The ring to the core.
    figure.line([(ring["x"], 290), (core["x"] + core["w"], 290)], LIGHT[1], arrow_end=True)
    figure.text(785, 282, "I2C B", 12, anchor="middle", colour=LIGHT[1])
    figure.text(785, 307, "4 wires", 12, anchor="middle", colour=LIGHT[1])
    # Time and power to the core, power to the ring.
    core_bottom = core["y"] + core["h"]
    figure.line([(530, time["y"]), (530, core_bottom)], TIME[1], arrow_end=True)
    figure.text(538, 375, "UART + PPS, I2C A", 12, colour=TIME[1])
    figure.line([(720, power["y"]), (720, core_bottom)], POWER[1], arrow_end=True)
    figure.text(728, 375, "5 V", 12, colour=POWER[1])
    figure.line([(910, power["y"]), (910, ring["y"] + ring["h"])], POWER[1], arrow_end=True)
    figure.text(918, 375, "the cell", 12, colour=POWER[1])
    return figure.save()


def receivers():
    figure = Figure("receivers", 900, 560)
    figure.text(20, 30, "Four rods at 45° steps leave no blind bearing", 16, bold=True)

    # Left: the plate, top view, north up.
    figure.text(40, 62, "One plastic plate, top view", 13, bold=True, colour=STATION[1])
    figure.rect(40, 75, 360, 360, PLAIN, radius=14)
    centres = [(140, 175, 0), (300, 175, 45), (140, 335, 90), (300, 335, 135)]
    for x, y, rod in centres:
        # The board is drawn with its rod pointing north, then turned to its
        # bearing: SVG's rotate() turns clockwise on a y-down canvas, which is
        # the sense a compass bearing turns.
        figure.raw(
            f'<g transform="translate({x} {y}) rotate({rod})">'
            f'<rect x="-26" y="-34" width="52" height="68" rx="4" fill="{STATION[0]}" '
            f'stroke="{STATION[1]}" stroke-width="1.6"/>'
            f'<rect x="-5" y="-27" width="10" height="54" rx="3" fill="#3e4c59"/>'
            f"</g>"
        )
        figure.text(x, y + 62, f"rod {rod}°", 12, anchor="middle")
    # The spacing between two boards.
    figure.line([(140, 112), (300, 112)], MUTED, width=1.2, arrow_end=True, arrow_start=True)
    figure.text(220, 104, "10 cm or more", 12, anchor="middle", colour=MUTED)
    # North.
    figure.line([(375, 128), (375, 92)], INK, width=1.6, arrow_end=True)
    figure.text(375, 145, "N", 12, bold=True, anchor="middle")
    figure.text(40, 462, "Tune one board at a time: the self-test's 500 kHz", 12, colour=MUTED)
    figure.text(40, 480, "oscillation is heard by its neighbours.", 12, colour=MUTED)

    # Right: the best board's response by bearing, computed.
    centre_x, centre_y, radius, floor = 660, 255, 165, -24.0
    figure.text(470, 62, "Best board's response by bearing", 13, bold=True, colour=STATION[1])

    def to_point(bearing, level_db):
        level = max(level_db, floor)
        r = radius * (1 - level / floor)
        angle = math.radians(bearing)
        return centre_x + r * math.sin(angle), centre_y - r * math.cos(angle)

    for ring_db in (0, -6, -12, -18):
        r = radius * (1 - ring_db / floor)
        figure.raw(
            f'<circle cx="{centre_x}" cy="{centre_y}" r="{r:.1f}" fill="none" stroke="#cbd2d9" stroke-width="1"/>'
        )
        figure.text(centre_x + 4, centre_y - r + 13, f"{ring_db} dB", 10, colour=MUTED, check=False, halo=True)
    for bearing, label in ((0, "N"), (90, "E"), (180, "S"), (270, "W")):
        x, y = to_point(bearing, 0)
        dx, dy = math.sin(math.radians(bearing)) * 14, -math.cos(math.radians(bearing)) * 14
        figure.text(x + dx, y + dy + 4, label, 12, bold=True, anchor="middle")

    legend_y = 478
    for index, (label, rods, colour, width) in enumerate(ROD_SETS):
        points = [to_point(step, decibels(best_response(rods, step))) for step in range(0, 361)]
        coordinates = " ".join(f"{x:.1f},{y:.1f}" for x, y in points)
        figure.raw(
            f'<polyline points="{coordinates}" fill="none" stroke="{colour}" stroke-width="{width}" '
            f'stroke-linejoin="round"/>'
        )
        worst, six_down = coverage(rods)
        if math.isinf(worst):
            summary = f"{label}: a null; {six_down * 100:.0f} % of the horizon 6 dB or more down"
        else:
            summary = f"{label}: worst bearing {worst:.1f} dB; {six_down * 100:.0f} % 6 dB or more down"
        y = legend_y + 22 * index
        figure.line([(470, y - 4), (500, y - 4)], colour, width=width)
        figure.text(508, y, summary, 12)
    return figure.save()


def ring():
    figure = Figure("ring", 780, 760)
    figure.text(20, 30, "Eight flash sensors, one 45° sector each, on one cable", 16, bold=True)
    centre_x, centre_y = 390, 400
    node_radius, wedge_inner, wedge_outer = 215, 262, 318
    names = ["N", "NE", "E", "SE", "S", "SW", "W", "NW"]

    def at(bearing, r):
        angle = math.radians(bearing)
        return centre_x + r * math.sin(angle), centre_y - r * math.cos(angle)

    # The sectors, alternately shaded, each a 45° wedge facing out from its node.
    for index in range(8):
        start, end = index * 45 - 22.5, index * 45 + 22.5
        outer_start, outer_end = at(start, wedge_outer), at(end, wedge_outer)
        inner_start, inner_end = at(start, wedge_inner), at(end, wedge_inner)
        fill = LIGHT[0] if index % 2 == 0 else "#fdfaf0"
        figure.raw(
            f'<path d="M {inner_start[0]:.1f} {inner_start[1]:.1f} L {outer_start[0]:.1f} {outer_start[1]:.1f} '
            f'A {wedge_outer} {wedge_outer} 0 0 1 {outer_end[0]:.1f} {outer_end[1]:.1f} '
            f'L {inner_end[0]:.1f} {inner_end[1]:.1f} '
            f'A {wedge_inner} {wedge_inner} 0 0 0 {inner_start[0]:.1f} {inner_start[1]:.1f} Z" '
            f'fill="{fill}" stroke="#e2c27a" stroke-width="1"/>'
        )
        x, y = at(index * 45, (wedge_inner + wedge_outer) / 2)
        figure.text(x, y + 5, names[index], 13, bold=True, anchor="middle", colour=LIGHT[1])

    # The chain: out of the core to the north node, then round clockwise, open at the end.
    figure.line([(centre_x, centre_y - 70), at(0, node_radius)], INK, width=3)
    arc_start, arc_end = at(0, node_radius), at(315, node_radius)
    figure.raw(
        f'<path d="M {arc_start[0]:.1f} {arc_start[1]:.1f} A {node_radius} {node_radius} 0 1 1 '
        f'{arc_end[0]:.1f} {arc_end[1]:.1f}" fill="none" stroke="{INK}" stroke-width="3"/>'
    )

    # The nodes.
    for index in range(8):
        x, y = at(index * 45, node_radius)
        figure.rect(x - 34, y - 20, 68, 40, LIGHT, radius=6)
        figure.text(x, y + 5, f"0x{0x10 + index:02X}", 13, bold=True, anchor="middle", colour=INK, check=False)

    # The core in the middle.
    figure.rect(centre_x - 95, centre_y - 70, 190, 140, STATION, radius=10)
    figure.text(centre_x, centre_y - 36, "FireBeetle 2", 14, bold=True, anchor="middle", colour=STATION[1])
    figure.text(centre_x, centre_y - 16, "polls each node", 12, anchor="middle")
    figure.text(centre_x, centre_y + 2, "every 100–200 ms", 12, anchor="middle")
    figure.text(centre_x, centre_y + 30, "SEN0290 plate", 13, bold=True, anchor="middle", colour=STATION[1])
    figure.text(centre_x, centre_y + 50, "under the ring", 12, anchor="middle")

    # A flash at 70°: the two brightest neighbours, NE and E, give the bearing.
    flash_bearing = 70
    bolt_x, bolt_y = at(flash_bearing, 352)
    for neighbour in (45, 90):
        figure.line([(bolt_x, bolt_y), at(neighbour, node_radius + 22)], LIGHT[1], width=1.4, dashed=True)
    bolt = [(0, -22), (-9, 2), (-1, 2), (-7, 22), (10, -4), (2, -4), (8, -22)]
    coordinates = " ".join(f"{bolt_x + dx:.1f},{bolt_y + dy:.1f}" for dx, dy in bolt)
    figure.raw(f'<polygon points="{coordinates}" fill="#f6c343" stroke="{LIGHT[1]}" stroke-width="1.2"/>')

    figure.line([(30, 706), (70, 706)], INK, width=3)
    figure.text(80, 711, "the 4-wire chain: cell +, GND, SDA, SCL; about 1–2 m, 2.2 kΩ pull-ups", 12)
    figure.text(30, 736, "A flash at 70°: the two brightest neighbours, NE and E, give the bearing, to about ±20°.", 12)
    return figure.save()


def flash_sensor():
    figure = Figure("flash_sensor", 1000, 640)
    figure.text(20, 30, "One flash sensor: light wakes it, sound gives the distance", 16, bold=True)

    temt = figure.box(30, 115, 170, "TEMT6000", ["phototransistor"], LIGHT, height=70)
    front = figure.box(240, 115, 200, "Load + AC coupling", ["load lowered for daylight"], LIGHT, height=70)
    comparator = figure.box(490, 48, 220, "Comparator", ["LMV331 or TLV7011 class"], LIGHT, height=56)
    peak = figure.box(490, 128, 220, "Peak-hold", ["a diode and a capacitor"], LIGHT, height=56)
    microphone = figure.box(30, 258, 250, "MAX4466 + electret", ["gain by trimmer, 25–125×"], SOUND, height=70)
    digital = figure.box(30, 345, 250, "MS3625, optional", ["I2S, for close strikes"], SOUND, dashed=True,
                         height=60)
    figure.rect(760, 45, 220, 380, STATION, radius=10)
    figure.text(775, 410, "ESP32-C3", 16, bold=True, colour=STATION[1], check=False)

    # Ports on the chip's left edge, at the height of the line that reaches each.
    ports = [
        (76, "GPIO: light-sleep wake"),
        (156, "ADC1: first stroke's peak"),
        (221, "ADC1: edges, DMA ~20 kHz"),
        (293, "ADC1: thunder, ~1 kHz"),
        (375, "I2S, if fitted"),
    ]
    for y, label in ports:
        if text_width(label, 12) > 220 - 24:
            raise ValueError(f"port label '{label}' overflows the chip")
        figure.text(772, y + 4, label, 12, check=False)

    figure.line([temt["right"], front["left"]], LIGHT[1], arrow_end=True)
    branch_x = 465
    figure.line([front["right"], (branch_x, 150)], LIGHT[1])
    figure.line([(branch_x, 150), (branch_x, 76), comparator["left"]], LIGHT[1], arrow_end=True)
    figure.line([(branch_x, 150), (branch_x, 156), peak["left"]], LIGHT[1], arrow_end=True)
    figure.line([(branch_x, 150), (branch_x, 221), (760, 221)], LIGHT[1], arrow_end=True)
    figure.line([comparator["right"], (760, 76)], LIGHT[1], arrow_end=True)
    figure.line([peak["right"], (760, 156)], LIGHT[1], arrow_end=True)
    figure.text(600, 207, "fast edges only: the steady daylight is blocked", 12, anchor="middle", colour=LIGHT[1])
    figure.line([microphone["right"], (760, 293)], SOUND[1], arrow_end=True)
    figure.text(520, 286, "band-pass 5–300 Hz in firmware", 12, anchor="middle", colour=SOUND[1])
    figure.line([digital["right"], (760, 375)], SOUND[1], dashed=True, arrow_end=True)
    figure.text(520, 368, "BCLK, WS, SD", 12, anchor="middle", colour=SOUND[1])

    # The cable leaves the bottom of the chip.
    figure.line([(870, 425), (870, 458)], INK, width=3)
    figure.text(860, 476, "4-wire chain: cell +, GND, SDA, SCL", 12, bold=True, anchor="end")

    figure.box(30, 500, 300, "Sleep",
               ["light sleep at about 0.15 mA", "wakes about 1 ms after a flash", "awake 60 s for strokes, thunder"],
               PLAIN)
    figure.box(350, 500, 300, "Reports, polled over I2C",
               ["flash: time, peak, rise, length", "thunder: onset, level, 3 bands", "about 12 and 16 bytes"],
               PLAIN)
    figure.box(670, 500, 310, "Bus and power",
               ["I2C slave, 0x10–0x17 from NVS", "time mark: general call at PPS", "cell → 5 V pin → on-board LDO"],
               PLAIN)
    return figure.save()


def power():
    figure = Figure("power", 1060, 470)
    figure.text(20, 30, "Power: one linear charger, one fast boost, nothing switching near 500 kHz", 16, bold=True)

    solar = figure.box(20, 60, 150, "Solar panel", ["5–6 V, about 5 W"], POWER, height=70)
    thermostat = figure.box(205, 60, 190, "Thermostat", ["KSD9700, 45 °C, NC", "taped to the cell"], POWER, height=70)
    charger = figure.box(430, 60, 170, "CN3065", ["linear charger", "CHRG, DONE → GPIO"], POWER, height=70)
    cell = figure.box(635, 60, 190, "Li-ion, 5000 mAh", ["about 18 Wh", "two NTCs → ADC"], POWER, height=70)
    gauge = figure.box(860, 60, 180, "MAX17043", ["across the cell", "I2C A, 0x36"], POWER, height=70)

    figure.line([solar["right"], thermostat["left"]], POWER[1], arrow_end=True)
    figure.line([thermostat["right"], charger["left"]], POWER[1], arrow_end=True)
    figure.line([charger["right"], cell["left"]], POWER[1], arrow_end=True)
    figure.line([cell["right"], gauge["left"]], POWER[1])
    # The thermostat's thermal contact with the cell, dashed below the row.
    cell_bottom = cell["y"] + cell["h"]
    figure.line([(300, thermostat["y"] + thermostat["h"]), (300, 170), (700, 170), (700, cell_bottom)],
                POWER[1], dashed=True)
    figure.text(500, 188, "opens above 45 °C, closes near 35 °C: works if the firmware hangs", 12,
                anchor="middle", colour=POWER[1])

    # The cell feeds three branches.
    bus_y = 225
    figure.line([(780, cell_bottom), (780, bus_y)], POWER[1], width=2.4)
    figure.line([(420, bus_y), (930, bus_y)], POWER[1], width=2.4)
    regulator = figure.box(340, 250, 210, "3.3 V regulator", ["linear: no switching", "SEN0290s, SHT45, DS3231"],
                           POWER)
    boost = figure.box(580, 250, 220, "5 V boost", ["fixed frequency, 1 MHz +", "MT3608 class, 500 mA"], POWER)
    figure.box(830, 250, 210, "The cell itself", ["to the ring's 5 V pins", "each node's own LDO"], POWER)
    for x in (445, 690, 935):
        figure.line([(x, bus_y), (x, 250)], POWER[1], arrow_end=True)
    loads = figure.box(340, 360, 210, "Receivers, sensors", ["and the GNSS"], STATION)
    beetle = figure.box(580, 360, 220, "FireBeetle 2", ["takes 5 V only"], STATION)
    nodes = figure.box(830, 360, 210, "8 flash sensors", ["ESP32-C3 each"], LIGHT)
    figure.line([(445, 250 + regulator["h"]), (445, loads["y"])], POWER[1], arrow_end=True)
    figure.line([(690, 250 + boost["h"]), (690, beetle["y"])], POWER[1], arrow_end=True)
    figure.line([(935, 250 + boost["h"]), (935, nodes["y"])], POWER[1], arrow_end=True)

    figure.box(20, 250, 290, "Why not a 150 kHz module",
               ["3rd harmonic at 450 kHz", "4th harmonic at 600 kHz", "pulse skipping at light load",
                "drops into the AS3935's band"],
               PLAIN)
    return figure.save()


def timing(celsius=25):
    figure = Figure("timing", 1000, 380)
    speed = speed_of_sound(celsius)
    seconds_per_km = 1000 / speed
    figure.text(20, 30, f"A flash opens a 60 s window; thunder arrives {seconds_per_km:.2f} s per km later", 16,
                bold=True)
    left, right, axis_y, span = 90, 950, 230, 60.0

    def x_of(seconds):
        return left + (right - left) * seconds / span

    # PPS marks: one a second, each sent down the chain as an I2C general call.
    for second in range(0, 61):
        figure.line([(x_of(second), 92), (x_of(second), 102)], TIME[1], width=1.2)
    figure.text(left, 82, "PPS time marks, one a second, down the I2C chain: every node on station time", 12,
                colour=TIME[1])

    figure.line([(left, axis_y), (right, axis_y)], INK, width=1.6, arrow_end=True)
    for second in range(0, 61, 10):
        figure.line([(x_of(second), axis_y), (x_of(second), axis_y + 6)], INK, width=1.2)
        figure.text(x_of(second), axis_y + 22, f"{second} s", 12, anchor="middle")

    # The flash, at zero.
    figure.line([(x_of(0), 112), (x_of(0), axis_y)], LIGHT[1], width=3)
    figure.text(x_of(0) + 8, 128, "flash: light at the ring in nanoseconds;", 12, colour=LIGHT[1])
    figure.text(x_of(0) + 8, 145, "AS3935 IRQ and the sensors' wake within about 1 ms", 12, colour=LIGHT[1])

    for distance in (1, 5, 10, 16):
        arrival = distance * seconds_per_km
        x = x_of(arrival)
        figure.line([(x, 165), (x, axis_y)], SOUND[1], width=2)
        figure.text(x, 160, f"{distance} km", 12, bold=True, anchor="middle", colour=SOUND[1])
        figure.text(x + 6, 196, f"{arrival:.1f} s", 11, colour=SOUND[1])

    window_y = 275
    figure.rect(x_of(0), window_y, x_of(60) - x_of(0), 18, SOUND, radius=4)
    figure.text(x_of(30), window_y + 13, "listening window: 60 s after each flash", 12, anchor="middle",
                colour=SOUND[1], check=False)
    figure.text(left, 322,
                f"Sound at {celsius} °C: 331.3 + 0.606 × {celsius} = {speed:.1f} m/s; the SHT45 supplies the temperature.",
                12)
    figure.text(left, 342, "Thunder carries about 16 km in quiet air. Two flashes in one window are ambiguous:"
                " pair single-flash windows first.", 12)
    figure.text(left, 362, "A 10 m/s wind along the path still costs about 3 %.", 12)
    return figure.save()


def main():
    paths = [overview(), receivers(), ring(), flash_sensor(), power(), timing()]
    for path in paths:
        print(path.relative_to(HERE.parent.parent))


if __name__ == "__main__":
    main()
