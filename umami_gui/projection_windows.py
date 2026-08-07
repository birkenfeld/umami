# Part of the Unified Mechanism for Acquisition of Measured Intensity
# (UMAMI), see README and LICENSE files for more info.

"""Diffractogram and TOF spectrum windows.

Separate windows projecting the main histogram onto one axis, created
lazily on request by `MainWindow`.
"""

import math

import numpy as np
import pyqtgraph as pg
from pyqtgraph.Qt import QtCore, QtWidgets

from .axis_items import RotatedAxisItem, ZoomViewBox
from .plot_utils import step_histogram_curve

# Cap on labeled bin-edge ticks on the TOF spectrum's x axis -- with many
# TOF bins, labeling every one overlaps into unreadable clutter, so they're
# thinned out like pyqtgraph's own automatic ticks would.
MAX_T_AXIS_LABELS = 20


class _ScalarPlotWindow(QtWidgets.QWidget):
    """Shared scaffolding for a standalone single-curve plot window.

    Common to `DiffractogramWindow` and `TofSpectrumWindow`: a log-scale
    checkbox above the plot, and a cursor readout label below it (the same
    "controls above, readout below" shape as `aux_plot.AuxPlot`, scaled
    down to the one control a 1-D plot needs).
    """

    def __init__(self, title, window_title, bottom_label, axis_items=None):
        super().__init__()
        self.setWindowTitle(window_title)
        self.resize(700, 400)
        self._data = None  # raw (un-log-transformed) y values, see update_data()

        self.plot_widget = pg.PlotWidget(
            title=title, viewBox=ZoomViewBox(), axisItems=axis_items or {})
        self._plot_item = self.plot_widget.getPlotItem()
        self._plot_item.setLabel('bottom', bottom_label)
        self._plot_item.setLabel('left', 'counts')
        self.curve = step_histogram_curve(self.plot_widget)

        strip = QtWidgets.QWidget()
        row = QtWidgets.QHBoxLayout(strip)
        row.setContentsMargins(4, 0, 4, 0)
        strip.setSizePolicy(QtWidgets.QSizePolicy.Policy.Preferred,
                            QtWidgets.QSizePolicy.Policy.Fixed)
        self.log_check = QtWidgets.QCheckBox('Log scale')
        self.log_check.toggled.connect(
            lambda checked: self._plot_item.setLogMode(y=checked))
        row.addWidget(self.log_check)
        row.addStretch()

        self.cursor_label = QtWidgets.QLabel('')
        self.cursor_label.setTextFormat(QtCore.Qt.TextFormat.PlainText)
        self.cursor_label.setContentsMargins(8, 4, 8, 4)

        layout = QtWidgets.QVBoxLayout(self)
        layout.setContentsMargins(0, 0, 0, 0)
        layout.setSpacing(2)
        layout.addWidget(strip)
        layout.addWidget(self.plot_widget)
        layout.addWidget(self.cursor_label)

        self.plot_widget.scene().sigMouseMoved.connect(self._on_mouse_moved)

    def _on_mouse_moved(self, scene_pos):
        if not self._plot_item.sceneBoundingRect().contains(scene_pos):
            self.cursor_label.setText('')
            return
        view_pos = self._plot_item.vb.mapSceneToView(scene_pos)
        self.cursor_label.setText(self._cursor_text(view_pos.x()) or '')

    def leaveEvent(self, event):  # noqa: N802
        super().leaveEvent(event)
        self.cursor_label.setText('')

    def _cursor_text(self, view_x):
        """Readout text for the given x view-coordinate, or `None`/empty."""
        raise NotImplementedError


class DiffractogramWindow(_ScalarPlotWindow):
    """X-projection (sum over t) of the main histogram.

    Follows the same "closing just hides" pattern as the other quick-setup
    windows -- created lazily on first open, kept around after.
    """

    def __init__(self):
        super().__init__('Diffractogram', 'UMAMI diffractogram', 'x channel')

    def update_data(self, x_edges, y):
        if self.isVisible():
            self._data = y
            self.curve.setData(x_edges, y, stepMode='center')

    def _cursor_text(self, view_x):
        x = int(np.floor(view_x + 0.5))
        if self._data is None or not 0 <= x < len(self._data):
            return None
        return f'x={x}  counts={int(self._data[x])}'


class TofSpectrumWindow(_ScalarPlotWindow):
    """Time-projection (sum over x/y) of the main histogram.

    Labels the x axis with actual time-of-flight values (in ms) when the
    active mode's `time_bins` are known (see `set_bin_edges_ns`), falling
    back to plain bin-index labeling otherwise. Follows the same "closing
    just hides" pattern as the other quick-setup windows.
    """

    def __init__(self):
        super().__init__(
            'TOF spectrum', 'UMAMI TOF spectrum', 'time of flight',
            axis_items={'bottom': RotatedAxisItem(orientation='bottom')})
        self.plot_widget.getAxis('bottom').setHeight(70)
        # raw time_bins of the active mode (nanoseconds, trailing overflow
        # sentinel included), or None to fall back to plain bin-index labeling
        self.bin_edges_ns = None
        self._apply_ticks()

    def set_bin_edges_ns(self, value):
        self.bin_edges_ns = value if value and len(value) > 1 else None
        self._apply_ticks()

    def update_data(self, n_bins, y):
        if self.isVisible():
            self._data = y
            edges = np.arange(n_bins + 1) - 0.5
            self.curve.setData(edges, y, stepMode='center')

    def _apply_ticks(self):
        axis = self.plot_widget.getAxis('bottom')
        if self.bin_edges_ns is not None:
            axis.setTicks(self._tick_labels(self.bin_edges_ns))
        else:
            axis.setTicks(None)

    @staticmethod
    def _tick_labels(edges_ns):
        """[major, minor] tick levels for `AxisItem.setTicks()`.

        Position is the plot's bin-index x-coordinate (bin i spans
        [i-0.5, i+0.5)); edges_ns holds each real bin's upper edge in ns,
        plus a trailing overflow sentinel that isn't itself a plotted edge.
        Labeling every one of e.g. 4096 bins would overlap into mush, so
        only an evenly-spaced subset (at most `MAX_T_AXIS_LABELS`) is
        labeled -- offset by one stride from the fixed leading "0" tick, so
        it doesn't crowd its neighbor -- while every real edge still gets an
        unlabeled minor tick, like automatic ticks would show.
        """
        real_edges = edges_ns[:-1]
        n = len(real_edges)
        stride = math.ceil(n / MAX_T_AXIS_LABELS) or 1
        major = [(-0.5, '0')]
        major.extend((i + 0.5, f'{real_edges[i] / 1e6:.3f}ms')
                     for i in range(stride - 1, n, stride))
        major.append((len(edges_ns) - 1, 'overflow'))
        minor = [(i + 0.5, '') for i in range(n)]
        return [major, minor]

    def _bin_range_ms(self, i):
        """(lo, hi) in ms for time-bin `i`; `hi` is `None` for the overflow bin."""
        edges = self.bin_edges_ns
        lo = edges[i - 1] / 1e6 if i > 0 else 0.0
        if i == len(edges) - 1:
            return lo, None
        return lo, edges[i] / 1e6

    def _cursor_text(self, view_x):
        i = int(np.floor(view_x + 0.5))
        if self._data is None or not 0 <= i < len(self._data):
            return None
        counts = int(self._data[i])
        if self.bin_edges_ns is not None and i < len(self.bin_edges_ns):
            lo, hi = self._bin_range_ms(i)
            edge_text = (f'{lo:.3f}-{hi:.3f} ms' if hi is not None
                         else f'{lo:.3f} ms-overflow')
        else:
            edge_text = f'bin {i}'
        return f'{edge_text}  counts={counts}'
