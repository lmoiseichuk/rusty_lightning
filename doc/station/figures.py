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
    figure.text(20, 30, "The station listens on the roof; the panel shows and logs", 16, bold=True)

    figure.rect(20, 50, 250, 500, HOUSE, radius=12, dashed=True)
    figure.text(36, 76, "House", 14, bold=True, colour=HOUSE[1])
    panel = figure.box(
        40, 245, 210, "Panel",
        ["XIAO ESP32-C3, 7.5\" e-paper", "screen, CSV log, console", "its SEN0290: indoor tests"],
        PLAIN,
    )

    figure.rect(300, 50, 720, 500, HOUSE, radius=12, dashed=True)
    figure.text(316, 76, "Roof: low, away from the ridge, white ASA boxes", 14, bold=True, colour=HOUSE[1])

    fallback = figure.box(320, 95, 170, "ESP32-C3 Pro", ["fallback radio", "antenna socket"], RADIO, dashed=True)
    receivers = figure.box(
        510, 90, 240, "4 × SEN0290 (AS3935)",
        ["in the octagon, 50 cm up", "rods at 0°, 45°, 90°, 135°", "one IRQ line each"], STATION
    )
    core = figure.box(
        510, 245, 240, "FireBeetle 2 ESP32-P4",
        ["P4: receivers, ring, time, log", "on-board C6: the radio", "64 GB card: log ring, FAT32"],
        STATION,
    )
    ring = figure.box(
        820, 222, 180, "8 flash sensors",
        ["ESP32-C3 each", "BPW34, TEMT6000", "MAX4466, MS3625", "on for storms"],
        LIGHT,
    )
    time = figure.box(
        320, 405, 230, "Time and weather",
        ["GNSS with PPS: the octagon", "DS3231: the core box", "SHT45: shielded, on the mast"],
        TIME,
    )
    power = figure.box(
        590, 405, 410, "Power",
        [
            "solar panel on the core box → two chargers",
            "cell 1, its own MAX17043: the core",
            "cell 2, its own MAX17043: the ring",
            "3.3 V regulator in the octagon",
            "5 V boost, chosen by its noise: the FireBeetle",
        ],
        POWER,
    )

    # Receivers to the core.
    figure.line([(630, receivers["y"] + receivers["h"]), (630, core["y"])], STATION[1], arrow_end=True)
    figure.text(640, 214, "up the mast: I2C A and B, 4 IRQs", 12, colour=STATION[1])
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
    # The ring to the core: eight lanes through the PCA9548A.
    figure.line([(ring["x"], 290), (core["x"] + core["w"], 290)], LIGHT[1], arrow_end=True)
    figure.text(785, 282, "8 lanes", 12, anchor="middle", colour=LIGHT[1])
    figure.text(785, 307, "PCA9548A", 11, anchor="middle", colour=LIGHT[1])
    # Time and power to the core, power to the ring.
    core_bottom = core["y"] + core["h"]
    figure.line([(530, time["y"]), (530, core_bottom)], TIME[1], arrow_end=True)
    figure.text(538, 375, "UART + PPS, I2C A", 12, colour=TIME[1])
    figure.line([(720, power["y"]), (720, core_bottom)], POWER[1], arrow_end=True)
    figure.text(728, 375, "5 V", 12, colour=POWER[1])
    figure.line([(910, power["y"]), (910, ring["y"] + ring["h"])], POWER[1], arrow_end=True)
    figure.text(918, 385, "switched", 12, colour=POWER[1])
    return figure.save()


def layout():
    figure = Figure("layout", 1000, 720)
    figure.text(20, 30, "The station from the side: the noise below, the ears 50 cm up", 16, bold=True)
    mast_x, mast_top, mast_bottom = 500, 262, 504
    # A magnetic near field falls as the cube of the distance: how much quieter the
    # boost and the FireBeetle are at 50 cm than beside the plate, at 10 cm.
    near, far = 0.10, 0.50
    quieter_db = 20 * math.log10((far / near) ** 3)

    # The roof, and the core box with the solar panel on its lid, shading the cells.
    figure.line([(30, 664), (970, 664)], MUTED, width=2)
    figure.text(40, 690, "the roof: keep the octagon's top below the ridge line", 12, colour=MUTED)
    figure.box(300, 528, 400, "Core box",
               ["FireBeetle 2, DS3231, the 5 V boost", "cell 1 (the core), cell 2 (the rail)",
                "two CN3065s, MAX17043s, NTCs", "switches: octagon, GNSS, rail"], STATION, height=130)
    figure.rect(270, 506, 460, 16, POWER, radius=2)
    figure.text(260, 518, "solar panel", 12, bold=True, anchor="end", colour=POWER[1])

    # The mast, with the cables inside it and the SHT45's shield on it.
    figure.rect(mast_x - 8, mast_top, 16, mast_bottom - mast_top, PLAIN, radius=2)
    figure.line([(530, mast_bottom), (530, mast_top)], MUTED, width=1.2, arrow_end=True, arrow_start=True)
    figure.text(540, 360, "50 cm", 12, bold=True)
    figure.text(540, 440, "a fiberglass or plastic tube;", 12)
    figure.text(540, 457, "four sheathed cables inside", 12)
    for index, y in enumerate((300, 310, 320)):
        figure.line([(mast_x - 66 + index * 4, y), (mast_x - 14 - index * 4, y)], INK, width=2.4)
    figure.text(424, 306, "SHT45 in a", 12, anchor="end")
    figure.text(424, 322, "radiation shield", 12, anchor="end")

    # The octagon, with the GNSS on top and two of its eight arms.
    figure.box(390, 174, 220, "The octagon",
               ["4 × SEN0290 on one plate", "PCA9548A, a PTC per lane", "3.3 V regulator"], STATION,
               height=88)
    figure.rect(470, 150, 60, 24, TIME, radius=3)
    figure.text(500, 167, "GNSS", 12, bold=True, anchor="middle", colour=TIME[1], check=False)
    for side in (-1, 1):
        inner = 390 if side < 0 else 610
        outer = inner + side * 210
        figure.line([(inner, 218), (outer, 218)], INK, width=3)
        box_x = outer - 80 if side < 0 else outer
        figure.rect(box_x, 196, 80, 44, LIGHT, radius=6)
        figure.text(box_x + 40, 223, "sensor", 12, bold=True, anchor="middle", check=False)
        # Each window looks out and about 20° up.
        start_x = box_x if side < 0 else box_x + 80
        rise = 60 * math.tan(math.radians(20))
        figure.line([(start_x, 210), (start_x + side * 60, 210 - rise)], LIGHT[1], width=1.6, arrow_end=True)
    figure.text(185, 266, "8 arms, one per 45° sector (2 shown)", 12, anchor="middle")
    figure.text(815, 266, "a lane cable along each arm", 12, anchor="middle")
    figure.text(40, 168, "window ~20° up", 12, colour=LIGHT[1])

    figure.box(715, 330, 270, "Why this way",
               ["boost, FireBeetle: 50 cm from the plate",
                f"near field ∝ 1/r³: {quieter_db:.0f} dB under 10 cm",
                "the panel shades the cells",
                "no wire leaves the station"],
               PLAIN)
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
    figure = Figure("ring", 780, 810)
    figure.text(20, 30, "Eight flash sensors on the octagon's arms, one 45° sector each", 16, bold=True)
    centre_x, centre_y = 390, 400
    node_radius, wedge_inner, wedge_outer = 215, 262, 318
    names = ["N", "NE", "E", "SE", "S", "SW", "W", "NW"]

    def at(bearing, r):
        angle = math.radians(bearing)
        return centre_x + r * math.sin(angle), centre_y - r * math.cos(angle)

    # The sectors, alternately shaded, each a 45° wedge facing out from its sensor.
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

    # The lanes: a star, one spoke per sensor, so a broken or wet cable costs one
    # sensor and not every sensor after it. Drawn first, so the core covers their
    # inner ends.
    for index in range(8):
        figure.line([(centre_x, centre_y), at(index * 45, node_radius)], INK, width=3)

    for index in range(8):
        x, y = at(index * 45, node_radius)
        figure.rect(x - 34, y - 20, 68, 40, LIGHT, radius=6)
        figure.text(x, y + 5, f"ch {index}", 13, bold=True, anchor="middle", colour=INK, check=False)

    figure.rect(centre_x - 100, centre_y - 75, 200, 150, STATION, radius=10)
    figure.text(centre_x, centre_y - 44, "The octagon", 14, bold=True, anchor="middle", colour=STATION[1])
    figure.text(centre_x, centre_y - 24, "SEN0290 plate, GNSS", 12, anchor="middle")
    figure.text(centre_x, centre_y - 8, "PCA9548A", 12, anchor="middle")
    figure.text(centre_x, centre_y + 8, "a PTC per lane", 12, anchor="middle")
    figure.text(centre_x, centre_y + 38, "on a 50 cm mast", 13, bold=True, anchor="middle", colour=STATION[1])
    figure.text(centre_x, centre_y + 56, "over the core box", 12, anchor="middle")

    # A flash at 70°: the two brightest neighbours, NE and E, give the bearing.
    flash_bearing = 70
    bolt_x, bolt_y = at(flash_bearing, 352)
    for neighbour in (45, 90):
        figure.line([(bolt_x, bolt_y), at(neighbour, node_radius + 22)], LIGHT[1], width=1.4, dashed=True)
    bolt = [(0, -22), (-9, 2), (-1, 2), (-7, 22), (10, -4), (2, -4), (8, -22)]
    coordinates = " ".join(f"{bolt_x + dx:.1f},{bolt_y + dy:.1f}" for dx, dy in bolt)
    figure.raw(f'<polygon points="{coordinates}" fill="#f6c343" stroke="{LIGHT[1]}" stroke-width="1.2"/>')

    figure.line([(30, 738), (70, 738)], INK, width=3)
    figure.text(80, 743, "a lane, along an arm: 4 wires (rail +, GND, SDA, SCL), its own PCA9548A channel and PTC", 12)
    figure.text(30, 768, "Every sensor answers at 0x10: the channel is its place; each reports its own id.", 12)
    figure.text(30, 790, "A flash at 70°: the two brightest neighbours, NE and E, give the bearing, to about ±20°.", 12)
    return figure.save()


def flash_sensor():
    figure = Figure("flash_sensor", 1010, 690)
    figure.text(20, 30, "One flash sensor: two light channels, two sound channels, awake while its lane is on",
                16, bold=True)

    # Row one: the photodiode's chain. One dual op-amp is both the amplifier and the peak detector.
    photodiode = figure.box(30, 64, 140, "BPW34", ["photodiode"], LIGHT)
    amplifier = figure.box(200, 64, 200, "Transimpedance amp", ["op-amp A; gain: day, night"], LIGHT)
    coupling = figure.box(430, 64, 150, "AC coupling", ["corner ~1 kHz"], LIGHT)
    peak = figure.box(610, 64, 190, "Peak detector", ["op-amp B; ~1 ms decay"], LIGHT)
    temt = figure.box(30, 244, 250, "TEMT6000", ["phototransistor: the level"], LIGHT)
    microphone = figure.box(30, 319, 250, "MAX4466 + electret", ["gain by trimmer, 25–125×"], SOUND)
    digital = figure.box(30, 394, 250, "MS3625", ["I2S, for close strikes"], SOUND)

    chip_x, chip_top, chip_bottom = 830, 45, 520
    figure.rect(chip_x, chip_top, 170, chip_bottom - chip_top, STATION, radius=10)
    figure.text(chip_x + 14, chip_bottom - 14, "ESP32-C3", 16, bold=True, colour=STATION[1], check=False)
    ports = [
        (90, "ADC: photodiode peak"),
        (150, "ADC: photodiode, AC"),
        (205, "GPIO: amplifier gain"),
        (270, "ADC: TEMT6000"),
        (345, "ADC: MAX4466"),
        (420, "I2S: MS3625"),
        (468, "I2C slave"),
    ]
    for y, label in ports:
        if text_width(label, 12) > 170 - 24:
            raise ValueError(f"port label '{label}' overflows the chip")
        figure.text(chip_x + 12, y + 4, label, 12, check=False)

    row = 90
    figure.line([photodiode["right"], amplifier["left"]], LIGHT[1], arrow_end=True)
    figure.line([amplifier["right"], coupling["left"]], LIGHT[1], arrow_end=True)
    figure.line([coupling["right"], peak["left"]], LIGHT[1], arrow_end=True)
    figure.line([peak["right"], (chip_x, row)], LIGHT[1], arrow_end=True)
    branch_x = 595
    figure.line([(branch_x, row), (branch_x, 150), (chip_x, 150)], LIGHT[1], arrow_end=True)
    figure.text(712, 143, "later strokes, their shape", 12, anchor="middle", colour=LIGHT[1])
    # The gain line runs back from the chip to the amplifier: an analog switch changes its feedback resistor.
    amplifier_bottom = amplifier["y"] + amplifier["h"]
    figure.line([(chip_x, 205), (300, 205), (300, amplifier_bottom)], INK, width=1.4, arrow_end=True)
    figure.text(560, 198, "GPIO21 → analog switch: the amplifier's gain", 12, anchor="middle", colour=INK)

    figure.line([temt["right"], (chip_x, 270)], LIGHT[1], arrow_end=True)
    figure.text(555, 263, "day or night, the background", 12, anchor="middle", colour=LIGHT[1])
    figure.line([microphone["right"], (chip_x, 345)], SOUND[1], arrow_end=True)
    figure.text(555, 338, "thunder: decimated to 1 kHz, 5–300 Hz", 12, anchor="middle", colour=SOUND[1])
    figure.line([digital["right"], (chip_x, 420)], SOUND[1], arrow_end=True)
    figure.text(555, 413, "BCLK, WS, SD", 12, anchor="middle", colour=SOUND[1])

    # The lane leaves the bottom of the chip.
    figure.line([(915, chip_bottom), (915, 548)], INK, width=3)
    figure.text(905, 556, "lane: rail +, GND, SDA, SCL", 12, bold=True, anchor="end")

    figure.box(30, 578, 310, "Awake while its lane is on",
               ["80 MHz; 40 to 160 is fine", "datasheet: 18–22 mA at 80 MHz", "ADC: 4 channels, ~20 kHz each"],
               PLAIN)
    figure.box(355, 578, 310, "Reports, polled over I2C",
               ["flash: time, peak, rise, length", "thunder: onset, level, 3 bands", "about 12 and 16 bytes"],
               PLAIN)
    figure.box(680, 578, 320, "Lane and time",
               ["address 0x10 on every sensor", "time marks in two steps", "1.5 kΩ pull-ups here, to its 3.3 V"],
               PLAIN)
    return figure.save()


def power():
    figure = Figure("power", 1060, 540)
    figure.text(20, 30, "Power: one panel, two cells -- one for the core, one for the switched rail", 16, bold=True)

    solar = figure.box(20, 115, 150, "Solar panel", ["5–6 V, about 5 W", "on the core box"], POWER, height=70)
    rows = []
    for number, top, role, bus in ((2, 60, "the rail", "B"), (1, 170, "the core", "A")):
        thermostat = figure.box(215, top, 190, f"Thermostat {number}", ["KSD9700, 45 °C, NC", f"taped to cell {number}"],
                                POWER, height=70)
        charger = figure.box(440, top, 170, f"CN3065 {number}", ["linear charger", "IN and BAT only"], POWER,
                             height=70)
        cell = figure.box(645, top, 190, f"Cell {number}, 5000 mAh", [f"{role}: about 18 Wh", f"NTC; MAX17043, bus {bus}"],
                          POWER, height=70)
        figure.line([thermostat["right"], charger["left"]], POWER[1], arrow_end=True)
        figure.line([charger["right"], cell["left"]], POWER[1], arrow_end=True)
        rows.append((thermostat, cell))
    # The panel feeds both chargers, each through its own thermostat.
    figure.line([solar["right"], (192, 150)], POWER[1])
    figure.line([(192, 95), (192, 205)], POWER[1])
    figure.line([(192, 95), rows[0][0]["left"]], POWER[1], arrow_end=True)
    figure.line([(192, 205), rows[1][0]["left"]], POWER[1], arrow_end=True)
    figure.text(20, 268, "each thermostat opens above 45 °C and closes near 35 °C: it works if the firmware hangs",
                12, colour=POWER[1])

    # Cell 2, on top, feeds the switched rail down the right; cell 1 feeds the core
    # and the octagon from below. The two lines never cross.
    core_bus_y = 300
    core_cell = rows[1][1]
    figure.line([core_cell["bottom"], (core_cell["bottom"][0], core_bus_y), (445, core_bus_y)], POWER[1], width=2.4)
    figure.line([rows[0][1]["right"], (955, 95), (955, 330)], POWER[1], width=2.4, arrow_end=True)
    regulator = figure.box(340, 330, 210, "3.3 V regulator", ["in the octagon, behind", "a switch that stays on"], POWER)
    boost = figure.box(580, 330, 220, "5 V boost", ["one of two on hand", "the quieter at light load"], POWER)
    switch = figure.box(870, 330, 170, "Switched rail", ["AO3401A + AO3400", "on for storms"], POWER)
    for x in (445, 690):
        figure.line([(x, core_bus_y), (x, 330)], POWER[1], arrow_end=True)
    loads = figure.box(340, 440, 210, "The octagon", ["SEN0290s, SHT45, PCA9548A"], STATION)
    beetle = figure.box(580, 440, 220, "FireBeetle 2", ["5 V only; its 3.3 V: the GNSS"], STATION)
    nodes = figure.box(870, 440, 170, "8 flash sensors", ["a PTC on each lane"], LIGHT)
    figure.line([(445, 330 + regulator["h"]), (445, loads["y"])], POWER[1], arrow_end=True)
    figure.line([(690, 330 + boost["h"]), (690, beetle["y"])], POWER[1], arrow_end=True)
    figure.line([(955, 330 + switch["h"]), (955, nodes["y"])], POWER[1], arrow_end=True)

    figure.box(20, 330, 290, "Why not a 150 kHz module",
               ["3rd harmonic at 450 kHz", "a cheap oscillator drifts", "pulse skipping at light load",
                "drops into the AS3935's band"],
               PLAIN)
    return figure.save()


def lane():
    figure = Figure("lane", 960, 450)
    figure.text(20, 30, "The switched rail: the eight lanes, off by default, on for storms", 16, bold=True)
    pull_up, series, miller = 100e3, 10e3, 220e-9
    # Assumptions for the switch-on estimate, marked approx. in the text: the
    # AO3401A's gate sits about 1.2 V below its source while the lane rises (its
    # Miller plateau at a small load), and each of the eight sensors puts about 10 µF
    # on the rail.
    plateau, lane_capacitance, cell_high = 1.2, 8 * 10e-6, 4.2

    def resistor(x, top, bottom, label, label_side):
        figure.rect(x - 13, top, 26, bottom - top, PLAIN, radius=3)
        if label_side == "left":
            figure.text(x - 20, (top + bottom) / 2 + 4, label, 12, anchor="end")
        else:
            figure.text(x + 20, (top + bottom) / 2 + 4, label, 12)

    def ground(x, y):
        figure.line([(x - 18, y), (x + 18, y)], INK, width=2)
        figure.line([(x - 10, y + 6), (x + 10, y + 6)], INK, width=2)

    # Cell 2, the P-channel switch on the high side, then up the mast.
    figure.text(40, 85, "cell 2 +", 13, bold=True)
    figure.line([(108, 80), (420, 80)], INK, width=2)
    figure.box(420, 52, 140, "AO3401A", ["P-channel"], POWER, height=56)
    figure.text(414, 74, "S", 11, bold=True, anchor="end")
    figure.text(566, 74, "D", 11, bold=True)
    figure.text(498, 124, "G", 11, bold=True)
    figure.line([(560, 80), (920, 80)], INK, width=2, arrow_end=True)
    figure.text(800, 70, "switched rail, up the mast", 12, anchor="middle")
    figure.text(800, 100, "to 8 lanes, a PTC on each", 11, anchor="middle", colour=MUTED)

    # The gate: 100 kΩ to the cell holds it off; 10 kΩ in series from the driver
    # pulls it on; 220 nF from gate to drain slows the rail's rise (a Miller soft start).
    gate_y = 190
    figure.line([(490, 108), (490, gate_y)], INK, width=1.6)
    figure.line([(300, 80), (300, 105)], INK, width=1.6)
    resistor(300, 105, 165, "100 kΩ", "left")
    figure.line([(300, 165), (300, gate_y), (490, gate_y)], INK, width=1.6)
    figure.line([(600, 80), (600, 122)], INK, width=1.6)
    figure.line([(584, 122), (616, 122)], INK, width=2.4)
    figure.line([(584, 132), (616, 132)], INK, width=2.4)
    figure.line([(600, 132), (600, gate_y), (490, gate_y)], INK, width=1.6)
    figure.text(624, 132, "220 nF", 12)
    for x in (300, 490, 600):
        figure.raw(f'<circle cx="{x}" cy="{gate_y}" r="3.5" fill="{INK}"/>')
    figure.raw(f'<circle cx="600" cy="80" r="3.5" fill="{INK}"/>')
    figure.line([(490, gate_y), (490, 210)], INK, width=1.6)
    resistor(490, 210, 262, "10 kΩ", "right")
    figure.line([(490, 262), (490, 285)], INK, width=1.6)

    # The driver: an N-channel the FireBeetle's 3.3 V pin can switch fully.
    figure.box(420, 285, 140, "AO3400", ["N-channel"], POWER, height=56)
    figure.text(498, 281, "D", 11, bold=True)
    figure.text(498, 358, "S", 11, bold=True)
    figure.text(414, 309, "G", 11, bold=True, anchor="end")
    figure.line([(490, 341), (490, 372)], INK, width=1.6)
    ground(490, 372)
    figure.line([(110, 313), (420, 313)], INK, width=1.6, arrow_end=True)
    figure.text(110, 303, "FireBeetle pin: high = rail on", 12)
    figure.raw(f'<circle cx="250" cy="313" r="3.5" fill="{INK}"/>')
    figure.line([(250, 313), (250, 325)], INK, width=1.6)
    resistor(250, 325, 372, "100 kΩ", "left")
    figure.line([(250, 372), (250, 386)], INK, width=1.6)
    ground(250, 386)

    on_fraction = pull_up / (pull_up + series)
    gate_current = (cell_high - plateau) / series - plateau / pull_up
    slew = gate_current / miller
    inrush = lane_capacitance * slew
    figure.box(640, 160, 300, "What it does",
               [
                   "at boot: 100 kΩ holds the AO3400 off",
                   f"gate when on: −{3.0 * on_fraction:.1f} to −{4.2 * on_fraction:.1f} V",
                   "AO3401A: specified down to −2.5 V",
                   f"rail rises ~{slew / 1000:.1f} V/ms: ~{inrush * 1000:.0f} mA into 80 µF",
                   "power the rail, then the channels",
                   "cut: the channels first, then power",
                   "never a sensor's USB on a lane",
               ],
               PLAIN)
    return figure.save()


def card():
    figure = Figure("card", 1000, 500)
    figure.text(20, 30, "The card: a small FAT32 partition and a log ring", 16, bold=True)
    card_bytes, block_bytes, header_bytes = 64e9, 512, 20
    blocks = card_bytes / block_bytes
    search_reads = math.ceil(math.log2(blocks))

    figure.text(40, 58, "64 GB microSD, high-endurance", 12, colour=MUTED)
    figure.rect(40, 68, 40, 40, PLAIN, radius=3)
    figure.text(60, 93, "MBR", 11, bold=True, anchor="middle", check=False)
    figure.rect(80, 68, 220, 40, TIME, radius=3)
    figure.text(190, 93, "FAT32: images, settings", 12, bold=True, anchor="middle", colour=TIME[1], check=False)
    figure.rect(300, 68, 660, 40, STATION, radius=3)
    figure.text(630, 93, "log ring: the rest of the card, in 512-byte blocks", 12, bold=True, anchor="middle",
                colour=STATION[1], check=False)

    # A stretch of the ring, magnified: physical order left to right, the newest
    # block just before the oldest, which is the next to be written over.
    figure.line([(600, 108), (120, 150)], MUTED, width=1, dashed=True)
    figure.line([(700, 108), (880, 150)], MUTED, width=1, dashed=True)
    streams = {7: LIGHT, 8: SOUND, 9: RADIO}
    cells = [
        (2001, 7, 40), (2002, 8, 0), (2003, 8, 1), (2004, 8, 2), (2005, 7, 41), (2006, 9, 0), (2007, 9, 1),
        (1990, 7, 33), (1991, 7, 34), (1992, 7, 35), (1993, 7, 36), (1994, 7, 37),
    ]
    width = (880 - 120) / len(cells)
    for index, (sequence, stream, position) in enumerate(cells):
        x = 120 + index * width
        figure.rect(x + 2, 160, width - 4, 46, streams[stream], radius=4)
        figure.text(x + width / 2, 179, f"{sequence}", 11, bold=True, anchor="middle", check=False)
        figure.text(x + width / 2, 196, f"s{stream} #{position}", 11, anchor="middle", check=False)
    newest_x = 120 + 6.5 * width
    oldest_x = 120 + 7.5 * width
    figure.text(newest_x, 150, "newest", 12, bold=True, anchor="middle")
    figure.line([(newest_x, 154), (newest_x, 160)], INK, width=1.4)
    figure.text(oldest_x, 226, "oldest: written over next", 12, bold=True, anchor="middle")
    figure.line([(oldest_x, 206), (oldest_x, 214)], INK, width=1.4)
    figure.text(120, 226, "s7 a log, s8 a recording, s9 an image (an 18-block ring)", 12, colour=MUTED)

    # One block, field by field.
    figure.text(60, 262, "One block = one 512-byte sector", 13, bold=True)
    fields = [
        ("magic+format", 2, 110), ("length", 2, 80), ("sequence", 4, 100), ("stream", 4, 90),
        ("index", 4, 80), ("CRC32", 4, 80), ("payload", block_bytes - header_bytes, 340),
    ]
    if sum(size for _, size, _ in fields) != block_bytes:
        raise ValueError("the block's fields do not add up to a sector")
    x = 60
    for name, size, box_width in fields:
        palette = PLAIN if name == "payload" else STATION
        figure.rect(x, 272, box_width, 36, palette, radius=2)
        if text_width(name, 12, True) > box_width - 8:
            raise ValueError(f"field label '{name}' overflows its box")
        figure.text(x + box_width / 2, 295, name, 12, bold=True, anchor="middle", check=False)
        figure.text(x + box_width / 2, 326, f"{size} B", 11, anchor="middle", colour=MUTED)
        x += box_width
    overhead = header_bytes / block_bytes * 100
    figure.text(60, 350, f"Header {header_bytes} bytes, payload {block_bytes - header_bytes}: {overhead:.1f} % overhead. "
                "The CRC covers the rest, seeded with the card's id.", 12)

    figure.box(40, 372, 300, "The newest block",
               ["the sequence drops at one place", f"binary search: {search_reads} reads", "then a scan past bad blocks"], PLAIN)
    figure.box(355, 372, 300, "Streams",
               ["the first block describes it", "NVS lists the open ones", "a cut transfer resumes"], PLAIN)
    figure.box(670, 372, 290, "Images",
               ["copied into FAT32 when landed", "read back before it counts", "the ring copy kept till then"], PLAIN)
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

    # PPS marks: one a second; each sensor is given station time over its lane.
    for second in range(0, 61):
        figure.line([(x_of(second), 92), (x_of(second), 102)], TIME[1], width=1.2)
    figure.text(left, 82, "Station time: PPS while the GNSS is on, the DS3231's 1 Hz between; each sensor gets it in two steps", 12,
                colour=TIME[1])

    figure.line([(left, axis_y), (right, axis_y)], INK, width=1.6, arrow_end=True)
    for second in range(0, 61, 10):
        figure.line([(x_of(second), axis_y), (x_of(second), axis_y + 6)], INK, width=1.2)
        figure.text(x_of(second), axis_y + 22, f"{second} s", 12, anchor="middle")

    # The flash, at zero.
    figure.line([(x_of(0), 112), (x_of(0), axis_y)], LIGHT[1], width=3)
    figure.text(x_of(0) + 8, 128, "flash: light at the ring in nanoseconds;", 12, colour=LIGHT[1])
    figure.text(x_of(0) + 8, 145, "each sensor's peak detector holds it ~1 ms for the ADC", 12, colour=LIGHT[1])

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
    paths = [overview(), layout(), receivers(), ring(), flash_sensor(), lane(), power(), card(), timing()]
    for path in paths:
        print(path.relative_to(HERE.parent.parent))


if __name__ == "__main__":
    main()
