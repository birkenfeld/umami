"""Bundled Material Symbols icons used throughout the GUI."""

from pathlib import Path

from pyqtgraph.Qt import QtCore, QtGui, QtWidgets

_ICONS_DIR = Path(__file__).parent
ICON_SIZE = QtCore.QSize(16, 16)


def load_icon(name):
    """Load one of the bundled SVG icons (by base filename, no extension)."""
    return QtGui.QIcon(str(_ICONS_DIR / f'{name}.svg'))


def icon_button(name, text=''):
    """A QPushButton with one of the bundled icons, sized consistently."""
    btn = QtWidgets.QPushButton(load_icon(name), text)
    btn.setIconSize(ICON_SIZE)
    return btn
