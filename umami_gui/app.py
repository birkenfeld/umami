# Part of the Unified Mechanism for Acquisition of Measured Intensity
# (UMAMI), see README and LICENSE files for more info.

"""Entry point for the umami-gui console script."""

import signal
import sys

import pyqtgraph as pg
from pyqtgraph.Qt.QtWidgets import QApplication

from .icons import is_dark_mode, load_icon
from .main_window import MainWindow


def main():
    shm_name = sys.argv[1] if len(sys.argv) > 1 else 'umami'

    app = QApplication(['umami-histogram'])
    app.setOrganizationName('mlz')
    app.setApplicationName('umami')
    app.setWindowIcon(load_icon('logo_mark'))
    if is_dark_mode():
        pg.setConfigOption('background', '#232323')
        pg.setConfigOption('foreground', '#dddddd')
    else:
        pg.setConfigOption('background', 'w')
        pg.setConfigOption('foreground', 'k')
    # Qt's event loop otherwise blocks Python's own SIGINT handling; the
    # periodic image_timer below keeps ticking into Python so this fires
    # promptly instead of only between mouse/keyboard events
    signal.signal(signal.SIGINT, lambda *_args: app.quit())

    window = MainWindow(shm_name)
    window.show()
    app.exec()
