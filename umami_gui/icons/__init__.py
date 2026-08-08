# Part of the Unified Mechanism for Acquisition of Measured Intensity
# (UMAMI), see README and LICENSE files for more info.

"""Bundled Material Symbols icons used throughout the GUI.

Also includes dark/light theme detection.
"""

from pathlib import Path

from pyqtgraph.Qt.QtCore import QSize, Qt
from pyqtgraph.Qt.QtGui import (
    QColor,
    QGuiApplication,
    QIcon,
    QPainter,
    QPalette,
    QPixmap,
)
from pyqtgraph.Qt.QtWidgets import QApplication, QPushButton

_ICONS_DIR = Path(__file__).parent
ICON_SIZE = QSize(16, 16)


def is_dark_mode():
    """Best-effort dark-mode detection."""
    scheme = QGuiApplication.styleHints().colorScheme()
    if scheme != Qt.ColorScheme.Unknown:
        return scheme == Qt.ColorScheme.Dark
    window_color = QApplication.palette().color(QPalette.ColorRole.Window)
    return window_color.lightness() < 128


def load_icon(name, color=None):
    """Load one of the bundled SVG icons (by base filename, no extension).

    If `color` is given, the icon (a fixed dark fill in the source SVG) is
    re-tinted to that solid color using its alpha channel as a mask.
    """
    icon = QIcon(str(_ICONS_DIR / f'{name}.svg'))
    if color is None:
        return icon
    # tinting bakes the vector icon down to a plain QPixmap -- render it at
    # the screen's actual device-pixel-ratio (and tag it as such), or it
    # comes out a fixed 16x16 physical pixels and looks tiny on HiDPI
    dpr = QApplication.primaryScreen().devicePixelRatio()
    physical_size = QSize(round(ICON_SIZE.width() * dpr),
                          round(ICON_SIZE.height() * dpr))
    pixmap = icon.pixmap(physical_size)
    pixmap.setDevicePixelRatio(dpr)
    tinted = QPixmap(pixmap.size())
    tinted.setDevicePixelRatio(dpr)
    tinted.fill(Qt.GlobalColor.transparent)
    painter = QPainter(tinted)
    painter.drawPixmap(0, 0, pixmap)
    painter.setCompositionMode(QPainter.CompositionMode.CompositionMode_SourceIn)
    painter.fillRect(tinted.rect(), QColor(color))
    painter.end()
    return QIcon(tinted)


def icon_button(name, text='', tint=True, cls=QPushButton):
    """Create a QPushButton with one of the bundled icons.

    By default the icon is tinted to match the current palette's button-text
    color, so it stays visible against both light- and dark-mode button faces.
    """
    color = None
    if tint:
        color = QApplication.palette().color(QPalette.ColorRole.ButtonText)
    btn = cls()
    btn.setIcon(load_icon(name, color=color))
    btn.setText(text)
    btn.setIconSize(ICON_SIZE)
    return btn
