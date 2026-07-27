"""Bundled Material Symbols icons used throughout the GUI, plus dark/light
theme detection."""

from pathlib import Path

from pyqtgraph.Qt import QtCore, QtGui, QtWidgets

_ICONS_DIR = Path(__file__).parent
ICON_SIZE = QtCore.QSize(16, 16)


def is_dark_mode():
    """Best-effort dark-mode detection."""
    scheme = QtGui.QGuiApplication.styleHints().colorScheme()
    if scheme != QtCore.Qt.ColorScheme.Unknown:
        return scheme == QtCore.Qt.ColorScheme.Dark
    window_color = QtWidgets.QApplication.palette().color(QtGui.QPalette.ColorRole.Window)
    return window_color.lightness() < 128


def load_icon(name, color=None):
    """Load one of the bundled SVG icons (by base filename, no extension).

    If `color` is given, the icon (a fixed dark fill in the source SVG) is
    re-tinted to that solid color using its alpha channel as a mask.
    """
    icon = QtGui.QIcon(str(_ICONS_DIR / f'{name}.svg'))
    if color is None:
        return icon
    pixmap = icon.pixmap(ICON_SIZE)
    tinted = QtGui.QPixmap(pixmap.size())
    tinted.fill(QtCore.Qt.GlobalColor.transparent)
    painter = QtGui.QPainter(tinted)
    painter.drawPixmap(0, 0, pixmap)
    painter.setCompositionMode(QtGui.QPainter.CompositionMode.CompositionMode_SourceIn)
    painter.fillRect(tinted.rect(), QtGui.QColor(color))
    painter.end()
    return QtGui.QIcon(tinted)


def icon_button(name, text='', tint=True):
    """A QPushButton with one of the bundled icons.

    By default the icon is tinted to match the current palette's button-text
    color, so it stays visible against both light- and dark-mode button faces.
    """
    color = None
    if tint:
        color = QtWidgets.QApplication.palette().color(QtGui.QPalette.ColorRole.ButtonText)
    btn = QtWidgets.QPushButton(load_icon(name, color=color), text)
    btn.setIconSize(ICON_SIZE)
    return btn
