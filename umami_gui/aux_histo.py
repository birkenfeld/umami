# Part of the Unified Mechanism for Acquisition of Measured Intensity
# (UMAMI), see README and LICENSE files for more info.

"""Auxiliary/diagnostic histogram support.

A form-based add/edit dialog and the window that discovers a running
`aux_histo` output and live-plots its histograms.
"""

from pathlib import Path

import numpy as np
import pyqtgraph as pg
from pyqtgraph.Qt import QtCore, QtWidgets

from .axis_items import ZoomViewBox
from .icons import icon_button
from .shm import ShmHistogram

# Kept in sync by hand with the grammar/field table documented in
# src/expr.rs -- there is no machine-readable source for this on the wire.
EXPR_SYNTAX_HELP = '''\
Fields: time, rel_time, raw_0, raw_1, channel, ampl, x, y, t, i, \
flags, evtype, auxnum, gateup

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


class HistoDefDialog(QtWidgets.QDialog):
    """Add/edit one auxiliary histogram definition (name/filter/x/y-axis).

    Uses a form, with an inline expression-syntax reference, instead of
    hand-writing the equivalent JSON.
    """

    @staticmethod
    def _make_range_row(bins_default, min_default, max_default):
        """One line combining an axis's Bins/Min/Max fields, for space economy.

        Returns (row_widget, bins_spinbox, min_edit, max_edit).
        """
        row = QtWidgets.QWidget()
        row_layout = QtWidgets.QHBoxLayout(row)
        row_layout.setContentsMargins(0, 0, 0, 0)
        row_layout.addWidget(QtWidgets.QLabel('Bins:'))
        bins = QtWidgets.QSpinBox()
        bins.setRange(1, 65535)
        bins.setValue(bins_default)
        row_layout.addWidget(bins)
        row_layout.addWidget(QtWidgets.QLabel('Min:'))
        min_edit = QtWidgets.QLineEdit(str(min_default))
        min_edit.setMinimumWidth(70)
        row_layout.addWidget(min_edit)
        row_layout.addWidget(QtWidgets.QLabel('Max:'))
        max_edit = QtWidgets.QLineEdit(str(max_default))
        max_edit.setMinimumWidth(70)
        row_layout.addWidget(max_edit)
        return row, bins, min_edit, max_edit

    def __init__(self, parent=None, spec=None, aliases=None):
        super().__init__(parent)
        self.setWindowTitle('Histogram Definition')
        spec = spec or {}
        x = spec.get('x') or {}
        y = spec.get('y')

        form = QtWidgets.QFormLayout()
        self.name_edit = QtWidgets.QLineEdit(spec.get('name', ''))
        form.addRow('Name:', self.name_edit)
        self.filter_edit = QtWidgets.QLineEdit(spec.get('filter') or '')
        self.filter_edit.setPlaceholderText(
            'e.g. evtype == neutron  (empty = always true)')
        form.addRow('Filter:', self.filter_edit)

        form.addRow(QtWidgets.QLabel('<b>X axis</b>'))
        self.x_expr = QtWidgets.QLineEdit(x.get('expr', ''))
        form.addRow('  Expr:', self.x_expr)
        x_row, self.x_bins, self.x_min, self.x_max = self._make_range_row(
            x.get('bins', 256), x.get('min', 0), x.get('max', 256))
        form.addRow('  Range:', x_row)

        self.y_check = QtWidgets.QCheckBox('2-D (add Y axis)')
        self.y_check.setChecked(y is not None)
        form.addRow(self.y_check)
        form.addRow(QtWidgets.QLabel('<b>Y axis</b>'))
        self.y_expr = QtWidgets.QLineEdit((y or {}).get('expr', ''))
        form.addRow('  Expr:', self.y_expr)
        y_row, self.y_bins, self.y_min, self.y_max = self._make_range_row(
            (y or {}).get('bins', 256), (y or {}).get('min', 0),
            (y or {}).get('max', 256))
        form.addRow('  Range:', y_row)

        def sync_y_enabled(checked):
            for w in (self.y_expr, self.y_bins, self.y_min, self.y_max):
                w.setEnabled(checked)
        self.y_check.toggled.connect(sync_y_enabled)
        sync_y_enabled(self.y_check.isChecked())

        help_title = QtWidgets.QLabel('<b>Expression syntax</b>')
        help_label = QtWidgets.QLabel(help_text(aliases))
        help_label.setWordWrap(True)
        help_label.setStyleSheet('color: #555; font-size: 9pt;')
        help_scroll = QtWidgets.QScrollArea()
        help_scroll.setWidget(help_label)
        help_scroll.setWidgetResizable(True)
        help_scroll.setMaximumHeight(250)

        buttons = QtWidgets.QDialogButtonBox(
            QtWidgets.QDialogButtonBox.StandardButton.Ok |
            QtWidgets.QDialogButtonBox.StandardButton.Cancel)
        buttons.accepted.connect(self._validate_and_accept)
        buttons.rejected.connect(self.reject)

        layout = QtWidgets.QVBoxLayout(self)
        layout.addLayout(form)
        layout.addWidget(help_title)
        layout.addWidget(help_scroll)
        layout.addWidget(buttons)

    def _int(self, field, label):
        try:
            return int(field.text().strip())
        except ValueError:
            QtWidgets.QMessageBox.warning(
                self, 'Invalid', f'{label} must be an integer.')
            return None

    def _validate_and_accept(self):   # noqa: PLR0911
        if not self.name_edit.text().strip():
            QtWidgets.QMessageBox.warning(self, 'Invalid', 'Name is required.')
            return
        if not self.x_expr.text().strip():
            QtWidgets.QMessageBox.warning(self, 'Invalid', 'X expression is required.')
            return
        x_min, x_max = self._int(self.x_min, 'X min'), self._int(self.x_max, 'X max')
        if x_min is None or x_max is None:
            return
        if x_max <= x_min:
            QtWidgets.QMessageBox.warning(
                self, 'Invalid', 'X max must be greater than X min.')
            return
        if self.y_check.isChecked():
            if not self.y_expr.text().strip():
                QtWidgets.QMessageBox.warning(
                    self, 'Invalid', 'Y expression is required.')
                return
            y_min, y_max = (self._int(self.y_min, 'Y min'),
                            self._int(self.y_max, 'Y max'))
            if y_min is None or y_max is None:
                return
            if y_max <= y_min:
                QtWidgets.QMessageBox.warning(
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
                  'min': int(self.x_min.text().strip()),
                  'max': int(self.x_max.text().strip())},
        }
        filt = self.filter_edit.text().strip()
        if filt:
            result['filter'] = filt
        if self.y_check.isChecked():
            result['y'] = {'expr': self.y_expr.text().strip(),
                           'bins': self.y_bins.value(),
                           'min': int(self.y_min.text().strip()),
                           'max': int(self.y_max.text().strip())}
        return result


class AuxHistoWindow(QtWidgets.QWidget):
    """Separate window for user-defined auxiliary/diagnostic histograms.

    Handles the `aux_histo` output type: discovers the first active one via
    get_params (the first "<module>.histos" key found -- only one such
    output is supported here, matching the backend's own one-output
    convenience assumption), lets the user add/edit/delete definitions
    through a form instead of hand-written JSON, and live-plots each one
    (1-D as a step curve, 2-D as a log-scale image) from its own shm
    segment. Follows the same "closing just hides, state is kept" pattern
    as the diffractogram/TOF-spectrum windows -- reopening is instant.
    """

    REFRESH_MS = 500

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
        self._plots = {}     # histo_name -> (PlotItem, ImageItem|PlotDataItem, is_2d)
        self._plot_specs = {}  # histo_name -> the spec last used to build its shm/plot

        self.table = QtWidgets.QTableWidget(0, 5)
        self.table.setHorizontalHeaderLabels(['Name', 'X', 'Y', 'Filter', ''])
        header = self.table.horizontalHeader()
        header.setSectionResizeMode(QtWidgets.QHeaderView.ResizeMode.Interactive)
        header.setSectionResizeMode(3, QtWidgets.QHeaderView.ResizeMode.Stretch)
        self.table.setEditTriggers(QtWidgets.QAbstractItemView.EditTrigger.NoEditTriggers)
        self.table.setSelectionBehavior(QtWidgets.QAbstractItemView.SelectionBehavior.SelectRows)
        self.table.cellDoubleClicked.connect(self._on_row_double_clicked)

        btn_col = QtWidgets.QVBoxLayout()
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

        table_row = QtWidgets.QHBoxLayout()
        table_row.setContentsMargins(5, 5, 8, 5)
        table_row.addWidget(self.table)
        table_row.addLayout(btn_col)
        table_container = QtWidgets.QWidget()
        table_container.setLayout(table_row)

        # a splitter of row-splitters: every plot starts out evenly sized,
        # and the user can still drag to rebalance
        self.plot_area = QtWidgets.QSplitter(QtCore.Qt.Orientation.Vertical)
        self._row_splitters = []
        scroll = QtWidgets.QScrollArea()
        scroll.setWidget(self.plot_area)
        scroll.setWidgetResizable(True)

        splitter = QtWidgets.QSplitter(QtCore.Qt.Orientation.Vertical)
        splitter.addWidget(table_container)
        splitter.addWidget(scroll)
        splitter.setSizes([200, 500])

        layout = QtWidgets.QVBoxLayout(self)
        layout.setContentsMargins(0, 0, 0, 0)
        layout.addWidget(splitter)

        # Only ticks while the window is actually visible (see showEvent /
        # hideEvent) -- no point polling params or shm segments for a window
        # the user has never opened.
        self._timer = QtCore.QTimer(self)
        self._timer.timeout.connect(self._update_plots)

    def showEvent(self, event):  # noqa: N802
        super().showEvent(event)
        self.refresh()
        self._timer.start(self.REFRESH_MS)

    def hideEvent(self, event):  # noqa: N802
        super().hideEvent(event)
        self._timer.stop()

    def refresh(self):
        """Re-pull histogram definitions and rebuild the table/plot grid.

        Uses the first "<module>.histos" key found from get_params, adding
        new histograms and dropping removed ones. Safe to call often -- e.g.
        piggybacked on the main window's own params refresh -- since it
        no-ops on a failed/empty get_params.
        """
        params = self.client.get_params()
        if params is None:
            return
        self._module = None
        self._histos = []
        for key, info in sorted(params.items()):
            if key.endswith('.histos') and isinstance(info.get('value'), list):
                self._module = key[:-len('.histos')]
                self._histos = info['value']
                break
        if self._module is not None:
            aliases_info = params.get(f'{self._module}.available_aliases')
            self._aliases = (aliases_info or {}).get('value') or []
        else:
            self._aliases = []
        self._rebuild()

    def _forget(self, name):
        self._shms.pop(name).close()
        plot_widget, _, _, _ = self._plots.pop(name)
        plot_widget.setParent(None)
        plot_widget.deleteLater()
        self._plot_specs.pop(name, None)

    def invalidate_all(self):
        """Force every cached shm segment to be reopened on the next refresh.

        Needed after the umami process itself restarts: same-named segments
        are recreated from scratch with a new backing shm object, which
        _rebuild()'s own before/after spec comparison has no way to notice
        on its own (the specs it fetches via get_params look identical).
        """
        for name in list(self._shms):
            self._forget(name)

    def _rebuild(self):  # noqa: PLR0915
        # forget anything removed, or whose definition changed since we last
        # opened its shm segment -- the server unlinks and recreates the
        # segment for *any* change (even just a tweaked axis range), so a
        # pure name-based diff would keep reading a now-orphaned mapping
        current = {h['name']: h for h in self._histos}
        for name in list(self._shms):
            if current.get(name) != self._plot_specs.get(name):
                self._forget(name)

        self.table.setRowCount(len(self._histos))
        for row, spec in enumerate(self._histos):
            self.table.setItem(row, 0, QtWidgets.QTableWidgetItem(spec['name']))
            self.table.setItem(row, 1, QtWidgets.QTableWidgetItem(spec['x']['expr']))
            self.table.setItem(row, 2,
                QtWidgets.QTableWidgetItem(spec['y']['expr'] if spec.get('y') else ''))
            self.table.setItem(
                row, 3, QtWidgets.QTableWidgetItem(spec.get('filter') or ''))
            btn_widget = QtWidgets.QWidget()
            btn_layout = QtWidgets.QHBoxLayout(btn_widget)
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
            if name in self._plots:
                continue
            shm_name = f'{self.ipc_name}_{self._module}_{name}'
            try:
                shm = ShmHistogram(shm_name)
            except RuntimeError as e:
                self.log.warning(f'Could not open {shm_name!r}: {e}')
                continue
            self._shms[name] = shm
            self._plot_specs[name] = spec
            is_2d = shm.ny > 1
            row = i // col_count
            while row >= len(self._row_splitters):
                row_splitter = QtWidgets.QSplitter(QtCore.Qt.Orientation.Horizontal)
                self._row_splitters.append(row_splitter)
                self.plot_area.addWidget(row_splitter)
            plot_widget = pg.PlotWidget(title=name, viewBox=ZoomViewBox())
            self._row_splitters[row].addWidget(plot_widget)
            plot_item = plot_widget.getPlotItem()
            plot_item.setLabel('bottom', spec['x']['expr'])
            if is_2d:
                plot_item.setLabel('left', spec['y']['expr'])
                img = pg.ImageItem(border='w', axisOrder='row-major')
                plot_item.addItem(img)
                img.setColorMap(pg.colormap.get('viridis'))
                # real axis values (not bin index) -- setRect() must come
                # after this histo's first setImage() in _update_plots(),
                # since it derives its scale from the image's current
                # dimensions, which are unset (None, falling back to 1)
                # before any image has been assigned
                x_lo, x_span = self._axis_extent(spec['x'])
                y_lo, y_span = self._axis_extent(spec['y'])
                extent = QtCore.QRectF(x_lo, y_lo, x_span, y_span)
                self._plots[name] = (plot_widget, img, True, extent)
            else:
                curve = plot_item.plot(
                    stepMode='center', fillLevel=0, brush=(0, 0, 255, 80))
                edges = self._bin_edges(spec['x'])
                self._plots[name] = (plot_widget, curve, False, edges)

    def _update_plots(self):
        for name, shm in list(self._shms.items()):
            if name not in self._plots:
                continue
            _, item, is_2d, extent = self._plots[name]
            try:
                if is_2d:
                    buf = shm.read_plane(0)
                    item.setImage(np.log10(buf.astype(float) + 0.1), autoLevels=True)
                    item.setRect(extent)
                else:
                    # pyqtgraph keeps this array reference alive indefinitely
                    # (until the next setData) -- must be a copy, not a raw
                    # view into the mmap, or closing this shm later fails
                    # with "cannot close exported pointers exist"
                    buf = shm.read_plane(0)[0].copy()
                    item.setData(extent, buf, stepMode='center')
            except OSError as e:
                self.log.warning(f'Error reading aux histogram {name!r}: {e}')

    def _on_row_double_clicked(self, row, _column):
        if 0 <= row < len(self._histos):
            self._edit_histogram(self._histos[row])

    def _save_histogram(self):
        if not self._histos:
            QtWidgets.QMessageBox.warning(
                self, 'No histograms', 'No histograms to save.')
            return
        menu = QtWidgets.QMenu(self)
        for spec in self._histos:
            name = spec['name']
            menu.addAction(
                name, lambda _=False, n=name: self._save_histogram_to_file(n))
        button = self.sender()
        menu.exec(button.mapToGlobal(button.rect().bottomLeft()))

    @staticmethod
    def _bin_values(axis):
        """Each bin's lower edge, in the axis expression's own units.

        Inverts the binning done server-side: `bin = (v - min) * bins /
        (max - min + 1)`, where `max` is inclusive. E.g. bins=8, min=0,
        max=7 (one bin per representable integer) gives 0, 1, ..., 7.
        """
        bins, lo, hi = axis['bins'], axis['min'], axis['max']
        width = (hi - lo + 1) / bins
        return lo + np.arange(bins) * width

    @staticmethod
    def _bin_width(axis):
        return (axis['max'] - axis['min'] + 1) / axis['bins']

    @classmethod
    def _bin_edges(cls, axis):
        """Real-value edges of every bin (bins+1 points), for step-mode plots.

        Shifted back by half a bin width so the value a bin represents sits
        at the center of its rendered bar -- e.g. bin 0 of bins=8, min=0,
        max=7 renders as a bar centered on 0, spanning [-0.5, 0.5], matching
        the old bin-index convention (there, an implicit width of 1) rather
        than a plain edge-aligned histogram.
        """
        edges = np.append(cls._bin_values(axis), axis['max'] + 1)
        return edges - cls._bin_width(axis) / 2

    @classmethod
    def _axis_extent(cls, axis):
        """(low, span) of an axis's real-value range, for setRect().

        Also shifted by half a bin width, for the same reason as
        `_bin_edges` -- see there.
        """
        return axis['min'] - cls._bin_width(axis) / 2, axis['max'] - axis['min'] + 1

    def _save_histogram_to_file(self, name):
        shm = self._shms.get(name)
        if shm is None:
            QtWidgets.QMessageBox.warning(
                self, 'Not available', f'Histogram {name!r} is not currently open.')
            return
        path, _ = QtWidgets.QFileDialog.getSaveFileName(
            self, f'Save Histogram {name!r}', '', 'Text files (*.txt);;All files (*)')
        if not path:
            return
        if not Path(path).suffix:
            path += '.txt'
        _, _, is_2d, _ = self._plots[name]
        spec = next(h for h in self._histos if h['name'] == name)
        if is_2d:
            x = self._bin_values(spec['x'])
            y = self._bin_values(spec['y'])
            header = ('x: ' + ' '.join(f'{v:g}' for v in x) + '\n'
                      'y: ' + ' '.join(f'{v:g}' for v in y))
            np.savetxt(path, shm.read_plane(0), fmt='%d', header=header)
        else:
            x = self._bin_values(spec['x'])
            counts = shm.read_plane(0)[0]
            np.savetxt(path, np.column_stack([x, counts]), fmt=['%g', '%d'],
                       header='x y', comments='')
        self.log.info(f'Saved aux histogram {name!r} to {path}')

    def _add_histogram(self):
        if self._module is None:
            QtWidgets.QMessageBox.warning(
                self, 'No output',
                'No active aux_histo output found -- configure one first.')
            return
        dialog = HistoDefDialog(self, aliases=self._aliases)
        if dialog.exec() != QtWidgets.QDialog.DialogCode.Accepted:
            return
        new_specs = [*self._histos, dialog.spec()]
        self.client.set_params({f'{self._module}.histos': new_specs})
        self.refresh()

    def _edit_histogram(self, spec):
        dialog = HistoDefDialog(self, spec, aliases=self._aliases)
        if dialog.exec() != QtWidgets.QDialog.DialogCode.Accepted:
            return
        new_specs = [dialog.spec() if h['name'] == spec['name'] else h
                     for h in self._histos]
        self.client.set_params({f'{self._module}.histos': new_specs})
        self.refresh()

    def _delete_histogram(self, name):
        reply = QtWidgets.QMessageBox.question(
            self, 'Delete', f'Delete histogram {name!r}?',
            QtWidgets.QMessageBox.StandardButton.Yes |
            QtWidgets.QMessageBox.StandardButton.No)
        if reply != QtWidgets.QMessageBox.StandardButton.Yes:
            return
        new_specs = [h for h in self._histos if h['name'] != name]
        self.client.set_params({f'{self._module}.histos': new_specs})
        self.refresh()
