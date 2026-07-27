# Part of the Unified Mechanism for Acquisition of Measured Intensity
# (UMAMI), see README and LICENSE files for more info.

"""Animated build-up of the pixel-grid logo, for the About dialog.

Replays the concept the original logo artwork was drawn from, but generated
fresh each time rather than replayed from a fixed image: a random letter out
of UMAMI is rasterized to a coverage mask, each tile gets a noisy "heat"
value biased by that mask, and every tile starts cold (heat 0) and, after a
random per-tile delay, ramps up to its generated heat -- mimicking counts
randomly accumulating into a live detector histogram, with a hot letter
shape emerging from the noise.
"""

import random
import time
from itertools import pairwise

from pyqtgraph.Qt import QtCore, QtGui, QtWidgets

# Grid geometry matching the original logo artwork's 22x22 tiles (a 300x300
# viewBox), reused here purely as an animation resolution/aspect choice.
GRID = 22
VIEWBOX = 300
CELL = 12.6
BACKGROUND_COLOR = '#0a1224'

LETTERS = ('U', 'M', 'A', 'I')
LETTER_FONT_FAMILY = 'Fira Sans'  # matches icons/wordmark.svg; falls back if missing
LETTER_SIZE_FRACTION = 1.05  # of the supersampled canvas height
ROTATION_RANGE_DEG = (-15, 15)
# how much of a tile's heat comes from the letter mask vs. plain noise
NOISE_HEAT_RANGE = (0.0, 0.4)
LETTER_BOOST_RANGE = (0.55, 1.0)

# Blue -> yellow -> red thermal colormap (no green) -- the same one the
# original logo's tile colors were generated from.
THERMAL_STOPS = [
    (0.00, (10, 20, 60)),
    (0.35, (30, 110, 190)),
    (0.65, (230, 200, 40)),
    (1.00, (220, 60, 30)),
]

MIN_DELAY_MS, MAX_DELAY_MS = 0, 1200
MIN_RAMP_MS, MAX_RAMP_MS = 500, 900


def thermal_rgb(t):
    for (t0, c0), (t1, c1) in pairwise(THERMAL_STOPS):
        if t0 <= t <= t1:
            f = (t - t0) / (t1 - t0)
            return tuple(round(c0[i] + (c1[i] - c0[i]) * f) for i in range(3))
    return THERMAL_STOPS[-1][1]


def _letter_font(pixel_size):
    """Bold font for rasterizing a letter.

    Uses `LETTER_FONT_FAMILY`, or the platform's default sans-serif if that
    family isn't installed.
    """
    font = QtGui.QFont(LETTER_FONT_FAMILY)
    if QtGui.QFontInfo(font).family() != LETTER_FONT_FAMILY:
        font = QtGui.QFont()
        font.setStyleHint(QtGui.QFont.StyleHint.SansSerif,
                         QtGui.QFont.StyleStrategy.PreferMatch)
    font.setPixelSize(pixel_size)
    return font


def _letter_coverage(letter, grid, rng):
    """Rasterize `letter` to a grid x grid coverage map in [0, 1].

    Renders bold at high resolution, slightly rotated, and lets QImage's
    smooth downscale do the antialiased box-filtering, so letter edges land
    as partial coverage instead of a hard on/off mask.
    """
    supersample = grid * 8
    image_format = QtGui.QImage.Format.Format_Grayscale8
    image = QtGui.QImage(supersample, supersample, image_format)
    image.fill(0)
    painter = QtGui.QPainter(image)
    painter.setRenderHint(QtGui.QPainter.RenderHint.Antialiasing)
    center = supersample / 2
    painter.translate(center, center)
    painter.rotate(rng.uniform(*ROTATION_RANGE_DEG))
    painter.translate(-center, -center)
    painter.setFont(_letter_font(round(supersample * LETTER_SIZE_FRACTION)))
    painter.setPen(QtGui.QColor('white'))
    painter.drawText(image.rect(), QtCore.Qt.AlignmentFlag.AlignCenter, letter)
    painter.end()
    small = image.scaled(grid, grid, QtCore.Qt.AspectRatioMode.IgnoreAspectRatio,
                         QtCore.Qt.TransformationMode.SmoothTransformation)
    return [QtGui.qGray(small.pixel(col, row)) / 255
            for row in range(grid) for col in range(grid)]


def _generate_heat(letter, grid, rng):
    """Per-tile heat in [0, 1].

    Low-range noise everywhere, boosted by `letter`'s coverage so its shape
    emerges as the hot region.
    """
    coverage = _letter_coverage(letter, grid, rng)
    return [min(1.0, rng.uniform(*NOISE_HEAT_RANGE)
                 + c * rng.uniform(*LETTER_BOOST_RANGE))
            for c in coverage]


class LogoBuildupWidget(QtWidgets.QWidget):
    """Fixed-size widget animating the logo's pixel grid from cold to final.

    Starts the animation on construction and repaints on a timer until every
    tile has reached its final heat, then stops -- the last frame painted is
    already the plain static logo, so nothing further needs to tick.
    """

    def __init__(self, width, letter=None, parent=None):
        super().__init__(parent)
        scale = width / VIEWBOX
        pitch = VIEWBOX / GRID * scale
        cell = CELL * scale
        rng = random.Random()  # noqa: S311 -- cosmetic animation, not security-sensitive
        heats = _generate_heat(letter or rng.choice(LETTERS), GRID, rng)
        self._tiles = [
            (col * pitch, row * pitch, cell, cell, heat,
             rng.uniform(MIN_DELAY_MS, MAX_DELAY_MS),
             rng.uniform(MIN_RAMP_MS, MAX_RAMP_MS))
            for i, heat in enumerate(heats)
            for row, col in [divmod(i, GRID)]
        ]
        self._total_duration_ms = MAX_DELAY_MS + MAX_RAMP_MS
        self.setFixedSize(width, width)

        self._start_time = time.monotonic()
        self._timer = QtCore.QTimer(self)
        self._timer.setInterval(33)
        self._timer.timeout.connect(self.update)
        self._timer.start()

    def paintEvent(self, event):  # noqa: N802, ARG002
        elapsed_ms = (time.monotonic() - self._start_time) * 1000
        painter = QtGui.QPainter(self)
        painter.fillRect(self.rect(), QtGui.QColor(BACKGROUND_COLOR))
        for x, y, w, h, heat, delay_ms, ramp_ms in self._tiles:
            progress = max(0.0, min(1.0, (elapsed_ms - delay_ms) / ramp_ms))
            eased = 1 - (1 - progress) ** 3
            painter.fillRect(QtCore.QRectF(x, y, w, h),
                             QtGui.QColor(*thermal_rgb(heat * eased)))
        if elapsed_ms >= self._total_duration_ms:
            self._timer.stop()
