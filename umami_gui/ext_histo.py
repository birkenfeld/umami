# Part of the Unified Mechanism for Acquisition of Measured Intensity
# (UMAMI), see README and LICENSE files for more info.

"""Live view of histograms declared by `ext_process` outputs.

Unlike aux_histo, these histograms are produced entirely by an external
process, not UMAMI itself -- this window only discovers their declared shape
via `get_params` and displays whatever the external process has written to
the matching shm segment. Read-only: there is nothing here for UMAMI to
reconfigure, so no add/edit/delete like `AuxHistoWindow` has.
"""

import json

from pyqtgraph.Qt.QtCore import QSettings, Qt, QTimer
from pyqtgraph.Qt.QtWidgets import (
    QApplication,
    QHBoxLayout,
    QLabel,
    QScrollArea,
    QSpinBox,
    QSplitter,
    QVBoxLayout,
    QWidget,
)

from .aux_plot import AuxPlot
from .shm import ShmHistogram

DISPLAY_KEY = 'ext_histo_display'


def discover_ext_histos(params):
    """Every histogram declared by any `ext_process` output's `histos` param.

    Returns a list of `{name, output_name, x, y, t}` dicts, where `x` is
    always present (`{name, bins, min, max}`) and `y`/`t` are `None` for a
    1-D/2-D histogram.
    """
    result = []
    for key, info in sorted(params.items()):
        if not (key.endswith('._info') and info['kind'] == 'output'
                and info['type'] == 'ext_process'):
            continue
        module = key[:-len('._info')]
        histos_info = params.get(f'{module}.histos') or {}
        result.extend({**spec, 'output_name': module}
                      for spec in histos_info.get('value') or [])
    return result


def _tile_key(spec):
    return f"{spec['output_name']}.{spec['name']}"


def _to_aux_axis(axis):
    """Translate an `ext_process` axis into `AuxPlot`'s own convention.

    `min`/`max` here are the *values of the first and last bin*; `AuxPlot`
    (built for `aux_histo`) instead expects an inclusive range spanning all
    bins, with `bin_width = (max - min + 1) / bins`. Solving for an
    equivalent `max` reproduces the same per-bin values through AuxPlot's
    existing math, without a parallel implementation here.
    """
    bins = axis['bins']
    if bins <= 1:
        return {'expr': axis['name'], 'bins': bins,
                'min': axis['min'], 'max': axis['min']}
    step = (axis['max'] - axis['min']) / (bins - 1)
    return {'expr': axis['name'], 'bins': bins, 'min': axis['min'],
            'max': axis['min'] + bins * step - 1}


def _as_axis_spec(spec):
    result = {'x': _to_aux_axis(spec['x'])}
    if spec.get('y'):
        result['y'] = _to_aux_axis(spec['y'])
    return result


class _SlicedTile(QWidget):
    """Wraps an `AuxPlot` with a T-axis slice selector.

    For a 3-D `ext_process` histogram -- `aux_histo`'s own histograms never
    have a third axis, so `AuxPlot` has no notion of slicing; the selector
    (and the plane index it drives) lives here instead.
    """

    def __init__(self, plot, t_axis):
        super().__init__()
        self.plot = plot
        self._t_axis = t_axis

        layout = QVBoxLayout(self)
        layout.setContentsMargins(0, 0, 0, 0)
        layout.setSpacing(2)

        row = QHBoxLayout()
        row.addWidget(QLabel(f"{t_axis['name']}:"))
        self.slice_spin = QSpinBox()
        self.slice_spin.setRange(0, max(t_axis['bins'] - 1, 0))
        row.addWidget(self.slice_spin)
        self.value_label = QLabel()
        row.addWidget(self.value_label)
        row.addStretch()
        layout.addLayout(row)
        layout.addWidget(plot)

        self.slice_spin.valueChanged.connect(self._update_value_label)
        self._update_value_label(0)

    def _update_value_label(self, index):
        bins, lo, hi = self._t_axis['bins'], self._t_axis['min'], self._t_axis['max']
        step = (hi - lo) / (bins - 1) if bins > 1 else 0
        self.value_label.setText(f'= {lo + index * step:g}')

    @property
    def slice_index(self):
        return self.slice_spin.value()

    def update_data(self, name, buf):
        self.plot.update_data(name, buf)

    def display_state(self):
        return self.plot.display_state()


class ExtHistoWindow(QWidget):
    """Separate, read-only window for `ext_process`-declared histograms."""

    REFRESH_MS = 500

    def __init__(self, client, ipc_name, log):
        super().__init__()
        self.client = client
        self.ipc_name = ipc_name
        self.log = log
        self.setWindowTitle('UMAMI other histograms')
        self.resize(1000, 700)

        self._specs = []  # last-seen list of {name, output_name, x, y, t}
        self._shms = {}   # tile key -> ShmHistogram
        self._plots = {}  # tile key -> AuxPlot or _SlicedTile
        self._last_seen = None

        self.settings = QSettings()
        self._display_state = self._load_display_state()

        self.plot_area = QSplitter(Qt.Orientation.Vertical)
        self._row_splitters = []
        scroll = QScrollArea()
        scroll.setWidget(self.plot_area)
        scroll.setWidgetResizable(True)

        self.cursor_label = QLabel('')
        self.cursor_label.setTextFormat(Qt.TextFormat.PlainText)
        self.cursor_label.setMinimumWidth(250)
        self.cursor_label.setContentsMargins(8, 0, 8, 4)

        layout = QVBoxLayout(self)
        layout.setContentsMargins(0, 0, 0, 0)
        layout.addWidget(scroll)
        layout.addWidget(self.cursor_label)

        # Only ticks while the window is visible, same as AuxHistoWindow.
        self._timer = QTimer(self)
        self._timer.timeout.connect(self._update_plots)
        QApplication.instance().aboutToQuit.connect(self._save_display_state)

    def showEvent(self, event):  # noqa: N802
        super().showEvent(event)
        self.refresh()
        self._timer.start(self.REFRESH_MS)

    def hideEvent(self, event):  # noqa: N802
        super().hideEvent(event)
        self._timer.stop()

    def refresh(self):
        """Re-pull declared histograms and rebuild the plot grid.

        Safe to call often -- no-ops on a failed/empty get_params.
        """
        params = self.client.get_params(full=True)
        if params is None:
            return
        self._specs = discover_ext_histos(params)
        self._rebuild()

    def invalidate_all(self):
        """Force every cached shm segment to be reopened on the next refresh.

        Needed after the external process (or UMAMI itself) restarts -- see
        AuxHistoWindow.invalidate_all() for the same rationale.
        """
        for key in list(self._shms):
            self._forget(key)

    def _load_display_state(self):
        raw = self.settings.value(DISPLAY_KEY)
        if not isinstance(raw, str):
            return {}
        try:
            state = json.loads(raw)
        except ValueError:
            return {}
        return state if isinstance(state, dict) else {}

    def _save_display_state(self):
        state = {**self._display_state,
                 **{key: plot.display_state() for key, plot in self._plots.items()}}
        self.settings.setValue(DISPLAY_KEY, json.dumps(state))

    def _forget(self, key):
        del self._shms[key]
        plot = self._plots.pop(key)
        self._display_state[key] = plot.display_state()
        plot.setParent(None)
        plot.deleteLater()

    def _create_plot(self, spec):
        """Open the shm segment and build the plot widget for one histogram.

        Returns False (logging a warning) if the segment isn't there yet --
        the external process may not have created it -- retried on the next
        refresh().
        """
        key = _tile_key(spec)
        shm_name = f"{self.ipc_name}_{spec['output_name']}_{spec['name']}"
        try:
            shm = ShmHistogram(shm_name)
        except RuntimeError as e:
            self.log.warning(f'Could not open {shm_name!r}: {e}')
            return False
        is_2d = spec.get('y') is not None
        plot = AuxPlot(spec['name'], _as_axis_spec(spec), is_2d,
                       self._display_state.get(key, {}))
        plot.cursor_moved.connect(self.cursor_label.setText)
        widget = _SlicedTile(plot, spec['t']) if spec.get('t') else plot
        self._shms[key] = shm
        self._plots[key] = widget
        return True

    def _rebuild(self):
        old_by_key = {_tile_key(s): s for s in (self._last_seen or [])}
        new_by_key = {_tile_key(s): s for s in self._specs}
        stale_keys = {key for key, spec in old_by_key.items()
                      if new_by_key.get(key) != spec}
        for key in stale_keys:
            if key in self._plots:
                self._forget(key)
        self._last_seen = list(self._specs)

        # up to 3 per row, except exactly 4 which reads better as 2x2 than 3+1
        col_count = 2 if len(self._specs) == 4 else 3
        for i, spec in enumerate(self._specs):
            key = _tile_key(spec)
            if key not in self._plots and not self._create_plot(spec):
                continue
            row = i // col_count
            while row >= len(self._row_splitters):
                row_splitter = QSplitter(Qt.Orientation.Horizontal)
                self._row_splitters.append(row_splitter)
                self.plot_area.addWidget(row_splitter)
            self._row_splitters[row].insertWidget(i % col_count, self._plots[key])

        needed_rows = -(-len(self._specs) // col_count) if self._specs else 0
        while len(self._row_splitters) > needed_rows:
            row_splitter = self._row_splitters.pop()
            row_splitter.setParent(None)
            row_splitter.deleteLater()

    def _update_plots(self):
        for key, shm in list(self._shms.items()):
            plot = self._plots.get(key)
            if plot is None:
                continue
            t = plot.slice_index if isinstance(plot, _SlicedTile) else 0
            try:
                buf = shm.read_plane(t)
            except OSError as e:
                self.log.warning(f'Error reading live histogram {key!r}: {e}')
                continue
            plot.update_data(key, buf)
