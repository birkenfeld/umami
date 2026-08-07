# Part of the Unified Mechanism for Acquisition of Measured Intensity
# (UMAMI), see README and LICENSE files for more info.

"""Auxiliary/diagnostic histogram support.

A form-based add/edit dialog and the window that discovers a running
`aux_histo` output and live-plots its histograms.
"""

import json
from pathlib import Path

import numpy as np
from pyqtgraph import exporters
from pyqtgraph.Qt.QtCore import QSettings, Qt, QTimer, pyqtSignal
from pyqtgraph.Qt.QtWidgets import (
    QAbstractItemView,
    QApplication,
    QCheckBox,
    QDialog,
    QDialogButtonBox,
    QFileDialog,
    QFormLayout,
    QHBoxLayout,
    QHeaderView,
    QLabel,
    QLineEdit,
    QMenu,
    QMessageBox,
    QScrollArea,
    QSpinBox,
    QSplitter,
    QTableWidget,
    QTableWidgetItem,
    QVBoxLayout,
    QWidget,
)

from .aux_plot import AuxPlot, bin_values
from .icons import icon_button
from .shm import ShmHistogram

# QSettings key for all aux plots' display state, one JSON blob keyed by
# histo name.
AUX_DISPLAY_KEY = 'aux_histo_display'

# Kept in sync by hand with the grammar/field table documented in
# src/expr.rs.
EXPR_SYNTAX_HELP = '''\
Fields: time, rel_time, raw_0, raw_1, channel, ampl, x, y, t, i, \
flags, evtype, auxnum, monnum, gateup

Named constants (for evtype comparisons): neutron, monitor, edge, gate, \
tzero, auxsignal, heartbeat, void

Operators (usual precedence): + - * / & << >> == != < <= > >= && || !

Numbers: decimal (100), hex (0xFF), binary (0b1010)

Bit-slice: expr[offset..end] (unsigned) or expr[offset..end:signed] \
(sign-extended), e.g. raw_0[0..12:signed] for the first 12 bits as a \
signed integer

A filter's result is treated as a boolean (0 = drop, nonzero = keep); \
an axis expression's result is binned into [min, max] -- values outside \
that range are silently dropped, not clamped.'''


def discover_aux_histo_output(params):
    """Get the name of the aux_histo output in a `full` get-params map."""
    return next(
        (key[:-len('._info')] for key, info in sorted(params.items())
         if key.endswith('._info') and info['kind'] == 'output'
         and info['type'] == 'aux_histo'),
        None)


def help_text(aliases):
    """Format the help text including `available_aliases` param for display.

    Takes a list of {name, expr, help} dicts and renders them as an
    appendix to EXPR_SYNTAX_HELP.
    """
    if not aliases:
        return EXPR_SYNTAX_HELP
    lines = ['', '', 'Aliases (contributed by recipes or the config file):']
    for alias in sorted(aliases, key=lambda a: a['name']):
        help_text = f' -- {alias["help"]}' if alias.get('help') else ''
        lines.append(f'  {alias["name"]} = {alias["expr"]}{help_text}')
    return EXPR_SYNTAX_HELP + '\n'.join(lines)


class HistoDefDialog(QDialog):
    """Add/edit one auxiliary histogram definition (name/filter/x/y-axis).

    Uses a form, with an inline expression-syntax reference, instead of
    hand-writing the equivalent JSON.
    """

    RANGE_BOX_WIDTH = 80

    @classmethod
    def _range_spinbox(cls, value, lo=-2_147_483_648, hi=2_147_483_647):
        box = QSpinBox()
        box.setRange(lo, hi)
        box.setValue(value)
        box.setFixedWidth(cls.RANGE_BOX_WIDTH)
        return box

    @classmethod
    def _make_range_row(cls, bins_default, min_default, max_default):
        """One line combining an axis's Bins/Min/Max fields, for space economy.

        Returns (row_widget, bins_spinbox, min_spinbox, max_spinbox).
        """
        row = QWidget()
        row_layout = QHBoxLayout(row)
        row_layout.setContentsMargins(0, 0, 0, 0)
        bins = cls._range_spinbox(bins_default, 1, 65535)
        row_layout.addWidget(bins)
        row_layout.addWidget(QLabel('bins from'))
        min_box = cls._range_spinbox(min_default)
        row_layout.addWidget(min_box)
        row_layout.addWidget(QLabel('to'))
        max_box = cls._range_spinbox(max_default)
        row_layout.addWidget(max_box)
        row_layout.addStretch()
        return row, bins, min_box, max_box

    def __init__(self, parent=None, spec=None, aliases=None):
        super().__init__(parent)
        self.setWindowTitle('Histogram Definition')
        spec = spec or {}
        x = spec.get('x') or {}
        y = spec.get('y')

        form = QFormLayout()
        self.name_edit = QLineEdit(spec.get('name', ''))
        form.addRow('Name:', self.name_edit)
        self.filter_edit = QLineEdit(spec.get('filter') or '')
        self.filter_edit.setPlaceholderText(
            'e.g. evtype == neutron  (empty = always true)')
        form.addRow('Filter:', self.filter_edit)

        form.addRow(QLabel('<b>X axis</b>'))
        self.x_expr = QLineEdit(x.get('expr', ''))
        form.addRow('  Expr:', self.x_expr)
        x_row, self.x_bins, self.x_min, self.x_max = self._make_range_row(
            x.get('bins', 256), x.get('min', 0), x.get('max', 255))
        form.addRow('  Axis:', x_row)

        self.y_check = QCheckBox('2-D (add Y axis)')
        self.y_check.setChecked(y is not None)
        form.addRow(self.y_check)
        form.addRow(QLabel('<b>Y axis</b>'))
        self.y_expr = QLineEdit((y or {}).get('expr', ''))
        form.addRow('  Expr:', self.y_expr)
        y_row, self.y_bins, self.y_min, self.y_max = self._make_range_row(
            (y or {}).get('bins', 256), (y or {}).get('min', 0),
            (y or {}).get('max', 255))
        form.addRow('  Axis:', y_row)

        def sync_y_enabled(checked):
            for w in (self.y_expr, self.y_bins, self.y_min, self.y_max):
                w.setEnabled(checked)
        self.y_check.toggled.connect(sync_y_enabled)
        sync_y_enabled(self.y_check.isChecked())

        help_title = QLabel('<b>Expression syntax</b>')
        help_label = QLabel(help_text(aliases))
        help_label.setWordWrap(True)
        help_label.setStyleSheet('color: #555; font-size: 9pt;')
        help_scroll = QScrollArea()
        help_scroll.setWidget(help_label)
        help_scroll.setWidgetResizable(True)
        help_scroll.setMaximumHeight(250)

        buttons = QDialogButtonBox(QDialogButtonBox.StandardButton.Ok |
                                   QDialogButtonBox.StandardButton.Cancel)
        buttons.accepted.connect(self._validate_and_accept)
        buttons.rejected.connect(self.reject)

        layout = QVBoxLayout(self)
        layout.addLayout(form)
        layout.addWidget(help_title)
        layout.addWidget(help_scroll)
        layout.addWidget(buttons)

    def _validate_and_accept(self):
        if not self.name_edit.text().strip():
            QMessageBox.warning(self, 'Invalid', 'Name is required.')
            return
        if not self.x_expr.text().strip():
            QMessageBox.warning(self, 'Invalid', 'X expression is required.')
            return
        if self.x_max.value() <= self.x_min.value():
            QMessageBox.warning(
                self, 'Invalid', 'X max must be greater than X min.')
            return
        if self.y_check.isChecked():
            if not self.y_expr.text().strip():
                QMessageBox.warning(
                    self, 'Invalid', 'Y expression is required.')
                return
            if self.y_max.value() <= self.y_min.value():
                QMessageBox.warning(
                    self, 'Invalid', 'Y max must be greater than Y min.')
                return
        # Note: this is a client-side sanity check only -- the authoritative
        # validation (expression syntax, etc.) happens server-side and any
        # rejection there still surfaces as a logged error via set_params.
        self.accept()

    def spec(self):
        result = {
            'name': self.name_edit.text().strip(),
            'x': {'expr': self.x_expr.text().strip(),
                  'bins': self.x_bins.value(),
                  'min': self.x_min.value(),
                  'max': self.x_max.value()},
        }
        filt = self.filter_edit.text().strip()
        if filt:
            result['filter'] = filt
        if self.y_check.isChecked():
            result['y'] = {'expr': self.y_expr.text().strip(),
                           'bins': self.y_bins.value(),
                           'min': self.y_min.value(),
                           'max': self.y_max.value()}
        return result


class AuxHistoWindow(QWidget):
    """Separate window for user-defined auxiliary/diagnostic histograms.

    Handles the `aux_histo` output type: discovers the first active one via
    each module's `_info` entry in a `full` get-params reply (only one such
    output is supported here, matching the backend's own one-output
    convenience assumption), lets the user add/edit/delete definitions
    through a form instead of hand-written JSON, and live-plots each one
    (1-D as a step curve, 2-D as an image), each with its own display
    controls (`AuxPlot`), from its own shm segment.
    """

    REFRESH_MS = 500

    applied = pyqtSignal()

    def __init__(self, client, ipc_name, log):  # noqa: PLR0915
        super().__init__()
        self.client = client
        self.ipc_name = ipc_name
        self.log = log
        self.setWindowTitle('UMAMI auxiliary histograms')
        self.resize(1000, 700)

        self._module = None  # name of the first aux_histo output found
        self._histos = []    # its histogram specs, as last seen from get_params
        self._aliases = []   # its available_aliases, as last seen from get_params
        self._shms = {}      # histo_name -> ShmHistogram
        self._plots = {}     # histo_name -> AuxPlot
        self._last_seen_histos = None  # the whole histos list as of the last rebuild

        self.settings = QSettings()
        # per-plot display state (log/colormap/levels) of histos that have
        # been destroyed by _forget() -- merged with the live plots' own
        # state in _save_settings() so it survives a _rebuild() or restart
        self._display_state = self._load_display_state()

        self.table = QTableWidget(0, 5)
        self.table.setHorizontalHeaderLabels(['Name', 'X', 'Y', 'Filter', ''])
        header = self.table.horizontalHeader()
        header.setSectionResizeMode(QHeaderView.ResizeMode.Interactive)
        header.setSectionResizeMode(3, QHeaderView.ResizeMode.Stretch)
        self.table.setEditTriggers(
            QAbstractItemView.EditTrigger.NoEditTriggers)
        self.table.setSelectionBehavior(
            QAbstractItemView.SelectionBehavior.SelectRows)
        self.table.cellDoubleClicked.connect(self._on_row_double_clicked)

        btn_col = QVBoxLayout()
        add_btn = icon_button('add', 'Add')
        add_btn.clicked.connect(self._add_histogram)
        btn_col.addWidget(add_btn)
        refresh_btn = icon_button('refresh', 'Refresh')
        refresh_btn.clicked.connect(self.refresh)
        btn_col.addWidget(refresh_btn)
        save_btn = icon_button('save', 'Save')
        save_btn.clicked.connect(self._save_histogram)
        btn_col.addWidget(save_btn)
        btn_col.addStretch()

        table_row = QHBoxLayout()
        table_row.setContentsMargins(5, 5, 8, 5)
        table_row.addWidget(self.table)
        table_row.addLayout(btn_col)
        table_container = QWidget()
        table_container.setLayout(table_row)

        # a splitter of row-splitters: every plot starts out evenly sized,
        # and the user can still drag to rebalance
        self.plot_area = QSplitter(Qt.Orientation.Vertical)
        self._row_splitters = []
        scroll = QScrollArea()
        scroll.setWidget(self.plot_area)
        scroll.setWidgetResizable(True)

        splitter = QSplitter(Qt.Orientation.Vertical)
        splitter.addWidget(table_container)
        splitter.addWidget(scroll)
        splitter.setSizes([200, 500])

        self.cursor_label = QLabel('')
        self.cursor_label.setTextFormat(Qt.TextFormat.PlainText)
        self.cursor_label.setMinimumWidth(250)
        self.cursor_label.setContentsMargins(8, 0, 8, 4)

        layout = QVBoxLayout(self)
        layout.setContentsMargins(0, 0, 0, 0)
        layout.addWidget(splitter)
        layout.addWidget(self.cursor_label)

        # Only ticks while the window is visible (see showEvent /
        # hideEvent) -- no point polling params or shm segments for a window
        # the user has never opened.
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
        """Re-pull histogram definitions and rebuild the table/plot grid.

        Adding new histograms and dropping removed ones. Safe to call often
        -- e.g. piggybacked on the main window's own params refresh -- since
        it no-ops on a failed/empty get_params.
        """
        params = self.client.get_params(full=True)
        if params is None:
            return
        self._module = discover_aux_histo_output(params)
        if self._module is not None:
            histos_info = params.get(f'{self._module}.histos') or {}
            self._histos = histos_info.get('value') or []
            aliases_info = params.get(f'{self._module}.available_aliases')
            self._aliases = (aliases_info or {}).get('value') or []
        else:
            self._histos = []
            self._aliases = []
        self._rebuild()

    def _load_display_state(self):
        raw = self.settings.value(AUX_DISPLAY_KEY)
        # QSettings' INI backend can hand back a QStringList instead of a
        # str for a comma-containing value written by a hand-edited or
        # older config -- treat that the same as "nothing saved yet"
        if not isinstance(raw, str):
            return {}
        try:
            state = json.loads(raw)
        except ValueError:
            return {}
        return state if isinstance(state, dict) else {}

    def _save_display_state(self):
        # self._display_state only holds plots that have been destroyed by
        # _forget() -- merge the live ones' current settings on top so
        # what's open right now is what actually gets persisted
        state = {**self._display_state,
                 **{name: plot.display_state() for name, plot in self._plots.items()}}
        self.settings.setValue(AUX_DISPLAY_KEY, json.dumps(state))

    def _forget(self, name):
        self._shms.pop(name).close()
        plot = self._plots.pop(name)
        # per-plot display settings must outlive the widget -- _rebuild()
        # destroys and recreates every plot whenever the histos list changes
        self._display_state[name] = plot.display_state()
        plot.setParent(None)
        plot.deleteLater()

    def invalidate_all(self):
        """Force every cached shm segment to be reopened on the next refresh.

        Needed after the umami process itself restarts: same-named segments
        are recreated from scratch with a new backing shm object, which
        _rebuild()'s own before/after list comparison has no way to notice
        on its own (the histos list it fetches via get_params looks identical).
        Per-plot display settings survive, since this goes through _forget().
        """
        for name in list(self._shms):
            self._forget(name)

    def _create_plot(self, name, spec):
        """Open the shm segment and build the plot widget for a new histo.

        Returns False (logging a warning) if the shm segment isn't there yet.
        """
        shm_name = f'{self.ipc_name}_{self._module}_{name}'
        try:
            shm = ShmHistogram(shm_name)
        except RuntimeError as e:
            self.log.warning(f'Could not open {shm_name!r}: {e}')
            return False
        self._shms[name] = shm
        is_2d = shm.ny > 1
        plot = AuxPlot(name, spec, is_2d, self._display_state.get(name, {}))
        plot.cursor_moved.connect(self.cursor_label.setText)
        self._plots[name] = plot
        return True

    def _rebuild(self):
        # the server recreates every histogram's shm segment whenever the
        # histos list is replaced, even for an untouched entry -- so forget
        # everything whenever the list as a whole differs from last time
        if self._histos != self._last_seen_histos:
            for name in list(self._shms):
                self._forget(name)
            self._last_seen_histos = list(self._histos)

        self.table.setRowCount(len(self._histos))
        for row, spec in enumerate(self._histos):
            self.table.setItem(row, 0, QTableWidgetItem(spec['name']))
            self.table.setItem(row, 1, QTableWidgetItem(spec['x']['expr']))
            self.table.setItem(row, 2,
                QTableWidgetItem(spec['y']['expr'] if spec.get('y') else ''))
            self.table.setItem(
                row, 3, QTableWidgetItem(spec.get('filter') or ''))
            btn_widget = QWidget()
            btn_layout = QHBoxLayout(btn_widget)
            btn_layout.setContentsMargins(5, 0, 5, 0)
            edit_btn = icon_button('edit')
            edit_btn.setToolTip('Edit')
            edit_btn.clicked.connect(lambda _, s=spec: self._edit_histogram(s))
            del_btn = icon_button('delete')
            del_btn.setToolTip('Delete')
            del_btn.clicked.connect(lambda _, n=spec['name']: self._delete_histogram(n))
            btn_layout.addWidget(edit_btn)
            btn_layout.addWidget(del_btn)
            self.table.setCellWidget(row, 4, btn_widget)
        # the icon on the Delete button widens it beyond the buttons
        # column's default width, clipping its text -- widen the column to
        # fit now that the cell widgets (and their size hints) are in place
        self.table.resizeColumnToContents(4)

        # up to 3 per row, except exactly 4 which reads better as 2x2 than 3+1
        col_count = 2 if len(self._histos) == 4 else 3
        for i, spec in enumerate(self._histos):
            name = spec['name']
            if name not in self._plots and not self._create_plot(name, spec):
                continue
            row = i // col_count
            while row >= len(self._row_splitters):
                row_splitter = QSplitter(Qt.Orientation.Horizontal)
                self._row_splitters.append(row_splitter)
                self.plot_area.addWidget(row_splitter)
            # insertWidget also relocates a widget that's already placed (in
            # this or another row splitter), so the grid always matches
            # self._histos' current order even after an add/remove shifts it
            self._row_splitters[row].insertWidget(i % col_count, self._plots[name])

        # drop now-empty trailing rows (e.g. after removing histos shrank
        # the grid, or col_count itself changed)
        needed_rows = -(-len(self._histos) // col_count) if self._histos else 0
        while len(self._row_splitters) > needed_rows:
            row_splitter = self._row_splitters.pop()
            row_splitter.setParent(None)
            row_splitter.deleteLater()

    def _update_plots(self):
        for name, shm in list(self._shms.items()):
            plot = self._plots.get(name)
            if plot is None:
                continue
            try:
                buf = shm.read_plane(0)
            except OSError as e:
                self.log.warning(f'Error reading aux histogram {name!r}: {e}')
                continue
            plot.update_data(buf)

    def _on_row_double_clicked(self, row, _column):
        if 0 <= row < len(self._histos):
            self._edit_histogram(self._histos[row])

    def _save_histogram(self):
        if not self._histos:
            QMessageBox.warning(
                self, 'No histograms', 'No histograms to save.')
            return
        menu = QMenu(self)
        for spec in self._histos:
            name = spec['name']
            submenu = menu.addMenu(name)
            submenu.addAction(
                'ASCII text...',
                lambda _=False, n=name: self._save_histogram_to_file(n))
            submenu.addAction(
                'Image...',
                lambda _=False, n=name: self._save_histogram_image(n))
        button = self.sender()
        menu.exec(button.mapToGlobal(button.rect().bottomLeft()))

    def _save_histogram_to_file(self, name):
        shm = self._shms.get(name)
        if shm is None:
            QMessageBox.warning(
                self, 'Not available', f'Histogram {name!r} is not currently open.')
            return
        path, _ = QFileDialog.getSaveFileName(
            self, f'Save Histogram {name!r}', '', 'Text files (*.txt);;All files (*)')
        if not path:
            return
        if not Path(path).suffix:
            path += '.txt'
        is_2d = self._plots[name].is_2d
        spec = next(h for h in self._histos if h['name'] == name)
        header = self._histogram_export_header(spec, shm.read_run_id())
        if is_2d:
            x = bin_values(spec['x'])
            y = bin_values(spec['y'])
            header += ('\n# x values: ' + ' '.join(f'{v:g}' for v in x) +
                       '\n# y values: ' + ' '.join(f'{v:g}' for v in y))
            np.savetxt(path, shm.read_plane(0), fmt='%d', header=header,
                       comments='')
        else:
            x = bin_values(spec['x'])
            counts = shm.read_plane(0)[0]
            np.savetxt(path, np.column_stack([x, counts]), fmt=['%g', '%d'],
                       header=header, comments='')
        self.log.info(f'Saved aux histogram {name!r} to {path}')

    @staticmethod
    def _histogram_export_header(spec, run_id):
        lines = ['UMAMI auxiliary histogram export',
                f'run: {run_id}', f"name: {spec['name']}"]
        for axis_name in ('x', 'y'):
            axis = spec.get(axis_name)
            if axis is None:
                continue
            lines.append(f"{axis_name}: {axis['expr']} "
                        f"(bins={axis['bins']}, min={axis['min']}, max={axis['max']})")
        if spec.get('filter'):
            lines.append(f"filter: {spec['filter']}")
        return '\n'.join(f'# {line}' for line in lines)

    def _save_histogram_image(self, name):
        if name not in self._plots:
            QMessageBox.warning(
                self, 'Not available', f'Histogram {name!r} is not currently open.')
            return
        path, _ = QFileDialog.getSaveFileName(
            self, f'Save Histogram {name!r} Image', '',
            'PNG files (*.png);;All files (*)')
        if not path:
            return
        if not Path(path).suffix:
            path += '.png'
        plot_widget = self._plots[name].plot_widget
        exporters.ImageExporter(plot_widget.getPlotItem()).export(path)
        self.log.info(f'Saved aux histogram {name!r} image to {path}')

    def _add_histogram(self):
        if self._module is None:
            QMessageBox.warning(
                self, 'No output',
                'No active aux_histo output found -- configure one first.')
            return
        dialog = HistoDefDialog(self, aliases=self._aliases)
        if dialog.exec() != QDialog.DialogCode.Accepted:
            return
        new_specs = [*self._histos, dialog.spec()]
        self.client.set_params({f'{self._module}.histos': new_specs})
        self.applied.emit()
        self.refresh()

    def _edit_histogram(self, spec):
        dialog = HistoDefDialog(self, spec, aliases=self._aliases)
        if dialog.exec() != QDialog.DialogCode.Accepted:
            return
        new_specs = [dialog.spec() if h['name'] == spec['name'] else h
                     for h in self._histos]
        self.client.set_params({f'{self._module}.histos': new_specs})
        self.applied.emit()
        self.refresh()

    def _delete_histogram(self, name):
        reply = QMessageBox.question(
            self, 'Delete', f'Delete histogram {name!r}?',
            QMessageBox.StandardButton.Yes |
            QMessageBox.StandardButton.No)
        if reply != QMessageBox.StandardButton.Yes:
            return
        new_specs = [h for h in self._histos if h['name'] != name]
        self.client.set_params({f'{self._module}.histos': new_specs})
        self.applied.emit()
        self.refresh()
