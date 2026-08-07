# Part of the Unified Mechanism for Acquisition of Measured Intensity
# (UMAMI), see README and LICENSE files for more info.

"""One auxiliary histogram's plot widget, with its own display controls."""

import html

import numpy as np
import pyqtgraph as pg
from pyqtgraph.Qt.QtCore import QRectF, pyqtSignal
from pyqtgraph.Qt.QtWidgets import (
    QCheckBox,
    QComboBox,
    QDoubleSpinBox,
    QHBoxLayout,
    QLabel,
    QSizePolicy,
    QVBoxLayout,
    QWidget,
)

from .axis_items import ZoomViewBox
from .plot_utils import COLORMAPS, set_image_data, step_histogram_curve

# 'log': None resolves to `is_2d` at construction -- 2-D defaults to log
# (matching the previous hard-coded behavior), 1-D to linear.
DEFAULT_STATE = {
    'log': None,
    'colormap': 'viridis',
    'auto_levels': True,
    'level_min': 0.0,
    'level_max': 100.0,
}


def bin_values(axis):
    """Each bin's lower edge, in the axis expression's own units.

    Inverts the binning done server-side: `bin = (v - min) * bins / (max - min
    + 1)`, where `max` is inclusive. E.g. bins=8, min=0, max=7 (one bin per
    representable integer) gives 0, 1, ..., 7.
    """
    bins, lo, hi = axis['bins'], axis['min'], axis['max']
    width = (hi - lo + 1) / bins
    return lo + np.arange(bins) * width


def bin_width(axis):
    return (axis['max'] - axis['min'] + 1) / axis['bins']


def bin_edges(axis):
    """Real-value edges of each bin (bins+1 points), for step-mode 1d plots."""
    edges = np.append(bin_values(axis), axis['max'] + 1)
    # shift back by half a bin width so the value a bin represents sits
    # at the center of its rendered bar
    return edges - bin_width(axis) / 2


def axis_extent(axis):
    """(low, span) of an axis's real-value range, for setRect() in 2d plots."""
    # shifted by half a bin width, for the same reason as `bin_edges`
    return axis['min'] - bin_width(axis) / 2, axis['max'] - axis['min'] + 1


class AuxPlot(QWidget):
    """One auxiliary histogram's plot, with its own compact control strip.

    1-D histograms get a step curve with a log-y toggle; 2-D histograms get an
    image with log scale, colormap, and manual z-limit controls per plot.
    """

    cursor_moved = pyqtSignal(str)

    def __init__(self, name, spec, is_2d, state):
        super().__init__()
        self.name = name
        self.is_2d = is_2d
        self._extent = None  # 2-D only: QRectF real-value extent
        self._edges = None   # 1-D only: bin edges
        self._counts = None  # cached raw-counts buffer, see update_data()

        state = {**DEFAULT_STATE, **state}
        if state['log'] is None:
            state['log'] = is_2d

        layout = QVBoxLayout(self)
        layout.setContentsMargins(0, 0, 0, 0)
        layout.setSpacing(2)
        self._build_plot(name, spec)
        layout.addWidget(self._build_controls(state))
        layout.addWidget(self.plot_widget)

    # ---- construction ----

    def _build_plot(self, name, spec):
        self.plot_widget = pg.PlotWidget(title=html.escape(name),
                                         viewBox=ZoomViewBox())
        plot_item = self.plot_widget.getPlotItem()
        plot_item.setLabel('bottom', html.escape(spec['x']['expr']))
        if self.is_2d:
            y_expr = (spec.get('y') or {}).get('expr', '')
            plot_item.setLabel('left', html.escape(y_expr))
            self._img = pg.ImageItem(border='w', axisOrder='row-major')
            plot_item.addItem(self._img)
            x_lo, x_span = axis_extent(spec['x'])
            y_lo, y_span = axis_extent(spec['y'])
            self._extent = QRectF(x_lo, y_lo, x_span, y_span)
        else:
            plot_item.setLabel('left', 'counts')
            self._curve = step_histogram_curve(plot_item)
            self._edges = bin_edges(spec['x'])
        self.plot_widget.scene().sigMouseMoved.connect(self._on_mouse_moved)

    def _build_controls(self, state):
        strip = QWidget()
        row = QHBoxLayout(strip)
        row.setContentsMargins(4, 0, 4, 0)
        row.setSpacing(4)
        strip.setSizePolicy(QSizePolicy.Policy.Preferred,
                            QSizePolicy.Policy.Fixed)
        font = strip.font()
        font.setPointSizeF(max(font.pointSizeF() - 1.5, 7.0))
        strip.setFont(font)

        self.log_check = QCheckBox('Log')
        self.log_check.setToolTip('Log scale')
        self.log_check.setChecked(state['log'])
        row.addWidget(self.log_check)

        if self.is_2d:
            self.colormap_combo = QComboBox()
            self.colormap_combo.setToolTip('Colormap')
            self.colormap_combo.addItems(list(COLORMAPS))
            self.colormap_combo.setCurrentText(state['colormap'])
            self.colormap_combo.setFixedWidth(84)
            row.addWidget(self.colormap_combo)

            self.auto_check = QCheckBox('Auto')
            self.auto_check.setToolTip('Auto levels')
            self.auto_check.setChecked(state['auto_levels'])
            row.addWidget(self.auto_check)

            self.level_min_spin = self._level_spinbox(state['level_min'])
            self.level_max_spin = self._level_spinbox(state['level_max'])
            self.level_min_spin.setEnabled(not state['auto_levels'])
            self.level_max_spin.setEnabled(not state['auto_levels'])
            row.addWidget(self.level_min_spin)
            row.addWidget(QLabel('-'))
            row.addWidget(self.level_max_spin)

            self._img.setColorMap(pg.colormap.get(COLORMAPS[state['colormap']]))
            self.colormap_combo.currentTextChanged.connect(
                lambda n: self._img.setColorMap(pg.colormap.get(COLORMAPS[n])))
            self.auto_check.toggled.connect(self._on_auto_levels_toggled)
            self.level_min_spin.valueChanged.connect(lambda _: self._redraw())
            self.level_max_spin.valueChanged.connect(lambda _: self._redraw())
        else:
            self.plot_widget.getPlotItem().setLogMode(y=state['log'])

        self.log_check.toggled.connect(self._on_log_toggled)
        row.addStretch()
        return strip

    def _level_spinbox(self, value):
        box = QDoubleSpinBox()
        box.setRange(0, 1e9)
        box.setDecimals(0)
        box.setValue(value)
        box.setFixedWidth(64)
        return box

    # ---- display state ----

    def display_state(self):
        state = {'log': self.log_check.isChecked()}
        if self.is_2d:
            state.update(
                colormap=self.colormap_combo.currentText(),
                auto_levels=self.auto_check.isChecked(),
                level_min=self.level_min_spin.value(),
                level_max=self.level_max_spin.value())
        return state

    def _on_auto_levels_toggled(self, checked):
        self.level_min_spin.setEnabled(not checked)
        self.level_max_spin.setEnabled(not checked)
        self._redraw()

    def _on_log_toggled(self, checked):
        if not self.is_2d:
            # setLogMode() auto-fits the range, acceptable here
            self.plot_widget.getPlotItem().setLogMode(y=checked)
        self._redraw()

    # ---- data ----

    def update_data(self, name, buf):  # noqa: ARG002 -- symmetry with AuxOverlayPlot
        """Feed a freshly-read data (`shm.read_plane(0)`) to this plot.

        Caches a *copy* of the raw counts (we should not keep the mmap alive)
        for the cursor readout and for redrawing without re-reading shm on
        every control change.
        """
        if self.is_2d:
            self._counts = buf.copy()
        else:
            counts = buf[0]
            if len(counts) + 1 != len(self._edges):
                # stale shm reopened against a since-changed spec; the next
                # _rebuild() will re-pair them once the server catches up
                return
            self._counts = counts.copy()
        self._redraw()

    def _redraw(self):
        if self._counts is None:
            return
        if self.is_2d:
            set_image_data(
                self._img, self._counts, log=self.log_check.isChecked(),
                auto_levels=self.auto_check.isChecked(),
                level_min=self.level_min_spin.value(),
                level_max=self.level_max_spin.value())
            self._img.setRect(self._extent)
        else:
            self._curve.setData(self._edges, self._counts, stepMode='center')

    # ---- cursor readout ----

    def _on_mouse_moved(self, scene_pos):
        plot_item = self.plot_widget.getPlotItem()
        if not plot_item.sceneBoundingRect().contains(scene_pos):
            self.cursor_moved.emit('')
            return
        view_pos = plot_item.vb.mapSceneToView(scene_pos)
        text = self._cursor_text_2d(view_pos) if self.is_2d \
            else self._cursor_text_1d(view_pos)
        self.cursor_moved.emit(text)

    def leaveEvent(self, event):  # noqa: N802
        super().leaveEvent(event)
        self.cursor_moved.emit('')

    def _cursor_text_2d(self, view_pos):
        if self._counts is None:
            return ''
        rect, (nrows, ncols) = self._extent, self._counts.shape
        col = int(np.floor((view_pos.x() - rect.left()) / rect.width() * ncols))
        row = int(np.floor((view_pos.y() - rect.top()) / rect.height() * nrows))
        if not (0 <= row < nrows and 0 <= col < ncols):
            return ''
        x = rect.left() + (col + 0.5) * rect.width() / ncols
        y = rect.top() + (row + 0.5) * rect.height() / nrows
        return f'{self.name}:  x={x:g}  y={y:g}  counts={int(self._counts[row, col])}'

    def _cursor_text_1d(self, view_pos):
        if self._counts is None:
            return ''
        i = int(np.searchsorted(self._edges, view_pos.x(), side='right')) - 1
        if not 0 <= i < len(self._counts):
            return ''
        x = (self._edges[i] + self._edges[i + 1]) / 2
        return f'{self.name}:  x={x:g}  counts={int(self._counts[i])}'


class AuxOverlayPlot(QWidget):
    """Several 1-D histograms sharing a `group`, overlaid on one plot.

    One step curve per member, distinguished by color (`pg.intColor`,
    pyqtgraph's built-in categorical color cycle) and a legend -- overlapping
    semi-transparent fills like `AuxPlot`'s single-curve look get muddy with
    2+ curves, so these are plain colored outlines instead. A single shared
    control strip (`Log` only -- colormap/levels don't apply to line curves)
    affects every curve's y-axis together.
    """

    cursor_moved = pyqtSignal(str)
    is_2d = False  # for _save_histogram_to_file's uniform is_2d lookup

    def __init__(self, specs, state):
        super().__init__()
        self._members = {}  # name -> {'edges', 'counts', 'curve'}

        title = specs[0]['group']
        x_exprs = {s['x']['expr'] for s in specs}
        x_label = next(iter(x_exprs)) if len(x_exprs) == 1 else title

        layout = QVBoxLayout(self)
        layout.setContentsMargins(0, 0, 0, 0)
        layout.setSpacing(2)
        self._build_plot(title, x_label, specs)
        layout.addWidget(self._build_controls(state))
        layout.addWidget(self.plot_widget)

    # ---- construction ----

    def _build_plot(self, title, x_label, specs):
        self.plot_widget = pg.PlotWidget(
            title=html.escape(title), viewBox=ZoomViewBox())
        plot_item = self.plot_widget.getPlotItem()
        plot_item.setLabel('bottom', html.escape(x_label))
        plot_item.setLabel('left', 'counts')
        plot_item.addLegend()
        for i, spec in enumerate(specs):
            name = spec['name']
            pen = pg.mkPen(pg.intColor(i, hues=len(specs)), width=2)
            curve = plot_item.plot(stepMode='center', pen=pen, name=name)
            self._members[name] = {'edges': bin_edges(spec['x']), 'counts': None,
                                    'curve': curve}
        self.plot_widget.scene().sigMouseMoved.connect(self._on_mouse_moved)

    def _build_controls(self, state):
        strip = QWidget()
        row = QHBoxLayout(strip)
        row.setContentsMargins(4, 0, 4, 0)
        row.setSpacing(4)
        strip.setSizePolicy(QSizePolicy.Policy.Preferred,
                            QSizePolicy.Policy.Fixed)
        font = strip.font()
        font.setPointSizeF(max(font.pointSizeF() - 1.5, 7.0))
        strip.setFont(font)

        self.log_check = QCheckBox('Log')
        self.log_check.setToolTip('Log scale')
        self.log_check.setChecked(state.get('log', False))
        self.plot_widget.getPlotItem().setLogMode(y=self.log_check.isChecked())
        self.log_check.toggled.connect(self._on_log_toggled)
        row.addWidget(self.log_check)
        row.addStretch()
        return strip

    # ---- display state ----

    def display_state(self):
        return {'log': self.log_check.isChecked()}

    def _on_log_toggled(self, checked):
        self.plot_widget.getPlotItem().setLogMode(y=checked)

    # ---- data ----

    def update_data(self, name, buf):
        member = self._members.get(name)
        if member is None:
            return
        counts = buf[0]
        if len(counts) + 1 != len(member['edges']):
            # stale shm reopened against a since-changed spec; the next
            # _rebuild() will re-pair them once the server catches up
            return
        member['counts'] = counts.copy()
        member['curve'].setData(member['edges'], member['counts'], stepMode='center')

    # ---- cursor readout ----

    def _on_mouse_moved(self, scene_pos):
        plot_item = self.plot_widget.getPlotItem()
        if not plot_item.sceneBoundingRect().contains(scene_pos):
            self.cursor_moved.emit('')
            return
        view_pos = plot_item.vb.mapSceneToView(scene_pos)
        x = view_pos.x()
        parts = [f'x={x:g}']
        for name, m in self._members.items():
            if m['counts'] is None:
                continue
            i = int(np.searchsorted(m['edges'], x, side='right')) - 1
            if 0 <= i < len(m['counts']):
                parts.append(f'{name}={int(m["counts"][i])}')
        self.cursor_moved.emit('  '.join(parts))

    def leaveEvent(self, event):  # noqa: N802
        super().leaveEvent(event)
        self.cursor_moved.emit('')
