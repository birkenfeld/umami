"""Entry point for the umami-gui console script."""

import signal
import sys

import pyqtgraph as pg
from pyqtgraph.Qt import QtWidgets

from .main_window import MainWindow


def main():
    shm_name = sys.argv[1] if len(sys.argv) > 1 else 'umami'

    pg.setConfigOption('background', 'w')
    pg.setConfigOption('foreground', 'k')
    app = QtWidgets.QApplication(['umami-histogram'])
    # Qt's event loop otherwise blocks Python's own SIGINT handling; the
    # periodic image_timer below keeps ticking into Python so this fires
    # promptly instead of only between mouse/keyboard events
    signal.signal(signal.SIGINT, lambda *args: app.quit())

    window = MainWindow(shm_name)
    window.show()
    app.exec()
