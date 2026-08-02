# Part of the Unified Mechanism for Acquisition of Measured Intensity
# (UMAMI), see README and LICENSE files for more info.

"""Diffractogram and TOF spectrum windows.

Separate windows projecting the main histogram onto one axis, created
lazily on request by `MainWindow`.
"""

import math

import numpy as np
import pyqtgraph as pg
from pyqtgraph.Qt import QtGui, QtWidgets

from .axis_items import RotatedAxisItem, ZoomViewBox

# Cap on labeled bin-edge ticks on the TOF spectrum's x axis -- with many
# TOF bins, labeling every one overlaps into unreadable clutter, so they're
# thinned out like pyqtgraph's own automatic ticks would.
MAX_T_AXIS_LABELS = 20


class DiffractogramWindow(pg.PlotWidget):
    """X-projection (sum over t) of the main histogram, with a hover tooltip.

    Follows the same "closing just hides" pattern as the other quick-setup
    windows -- created lazily on first open, kept around after.
    """

    def __init__(self):
        super().__init__(title='Diffractogram', viewBox=ZoomViewBox())
        self.setWindowTitle('UMAMI diffractogram')
        self.setLabel('bottom', 'x channel')
        self.setLabel('left', 'counts')
        self.resize(700, 400)
        self.curve = self.plot(stepMode='center', fillLevel=0, brush=(0, 0, 255, 80))
        self.scene().sigMouseMoved.connect(self._on_mouse_moved)

    def update_data(self, x_edges, y):
        if self.isVisible():
            self.curve.setData(x_edges, y, stepMode='center')

    def _on_mouse_moved(self, scene_pos):
        plot_item = self.getPlotItem()
        if not plot_item.sceneBoundingRect().contains(scene_pos):
            QtWidgets.QToolTip.hideText()
            return
        view_pos = plot_item.vb.mapSceneToView(scene_pos)
        x = int(np.floor(view_pos.x() + 0.5))
        _xdata, ydata = self.curve.getData()
        if ydata is None or not 0 <= x < len(ydata):
            QtWidgets.QToolTip.hideText()
            return
        text = f'x={x}\ncounts={int(ydata[x])}'
        QtWidgets.QToolTip.showText(QtGui.QCursor.pos(), text)


class TofSpectrumWindow(pg.PlotWidget):
    """Time-projection (sum over x/y) of the main histogram.

    Labels the x axis with actual time-of-flight values (in ms) when the
    active mode's `time_bins` are known (see `set_bin_edges_ns`), falling
    back to plain bin-index labeling otherwise. Follows the same "closing
    just hides" pattern as the other quick-setup windows.
    """

    def __init__(self):
        super().__init__(
            title='TOF spectrum', viewBox=ZoomViewBox(),
            axisItems={'bottom': RotatedAxisItem(orientation='bottom')})
        self.setWindowTitle('UMAMI TOF spectrum')
        self.setLabel('bottom', 'time of flight')
        self.setLabel('left', 'counts')
        self.getAxis('bottom').setHeight(70)
        self.resize(700, 400)
        self.curve = self.plot(stepMode='center', fillLevel=0, brush=(0, 0, 255, 80))
        # raw time_bins of the active mode (nanoseconds, trailing overflow
        # sentinel included), or None to fall back to plain bin-index labeling
        self.bin_edges_ns = None
        self.scene().sigMouseMoved.connect(self._on_mouse_moved)
        self._apply_ticks()

    def set_bin_edges_ns(self, value):
        self.bin_edges_ns = value if value and len(value) > 1 else None
        self._apply_ticks()

    def update_data(self, n_bins, y):
        if self.isVisible():
            edges = np.arange(n_bins + 1) - 0.5
            self.curve.setData(edges, y, stepMode='center')

    def _apply_ticks(self):
        axis = self.getAxis('bottom')
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

    def _on_mouse_moved(self, scene_pos):
        plot_item = self.getPlotItem()
        if not plot_item.sceneBoundingRect().contains(scene_pos):
            QtWidgets.QToolTip.hideText()
            return
        view_pos = plot_item.vb.mapSceneToView(scene_pos)
        i = int(np.floor(view_pos.x() + 0.5))
        _xdata, ydata = self.curve.getData()
        if ydata is None or not 0 <= i < len(ydata):
            QtWidgets.QToolTip.hideText()
            return
        counts = int(ydata[i])
        if self.bin_edges_ns is not None and i < len(self.bin_edges_ns):
            lo, hi = self._bin_range_ms(i)
            edge_text = (f'{lo:.3f}-{hi:.3f} ms' if hi is not None
                         else f'{lo:.3f} ms-overflow')
        else:
            edge_text = f'bin {i}'
        text = f'{edge_text}\ncounts={counts}'
        QtWidgets.QToolTip.showText(QtGui.QCursor.pos(), text)
