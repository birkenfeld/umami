# Part of the Unified Mechanism for Acquisition of Measured Intensity
# (UMAMI), see README and LICENSE files for more info.

"""Mesytec MCPD live configuration."""

import typing

from pyqtgraph.Qt import QtCore, QtWidgets

from .icons import icon_button

N_SLOTS = 8
MODULE_TYPES = ('mpsd', 'mstd')
PULSER_POSITIONS = ('left', 'right', 'middle')
CELL_TRIGGERS = (
    'none', 'aux1', 'aux2', 'aux3', 'aux4', 'digital1', 'digital2', 'compare')


def discover_mesy_inputs(params):
    """Names of mesy inputs present in a `full` get-params map, sorted."""
    return sorted(
        key[:-len('._info')] for key, info in params.items()
        if key.endswith('._info') and info['kind'] == 'input' and info['type'] == 'mesy'
    )


def _centered(widget):
    cell = QtWidgets.QWidget()
    layout = QtWidgets.QHBoxLayout(cell)
    layout.setContentsMargins(0, 0, 0, 0)
    layout.setAlignment(QtCore.Qt.AlignmentFlag.AlignCenter)
    layout.addWidget(widget)
    return cell


class MesyCellsTable(QtWidgets.QTableWidget):
    """8 fixed rows (cell index 0-7): Configure / Source / Compare.

    Loads/reports a dict shaped like the `cells` param value, e.g.
    `{"1": {"source": "aux2", "compare": 5}}` -- "Configure" means umami
    manages that slot, not that the cell is enabled; unset rows are
    omitted. Edits are local until read via `current()`.
    """

    def __init__(self):
        super().__init__(N_SLOTS, 3)
        self.setHorizontalHeaderLabels(['Configure', 'Source', 'Compare'])
        self.setVerticalHeaderLabels([f'Cell {i}' for i in range(N_SLOTS)])
        self.horizontalHeader().setSectionResizeMode(
            QtWidgets.QHeaderView.ResizeMode.Stretch)
        self._loading = False
        self._checks = []
        self._sources = []
        self._compares = []
        for row in range(N_SLOTS):
            check = QtWidgets.QCheckBox()
            check.setToolTip("Manage this cell's settings from umami")
            check.toggled.connect(self._on_toggle)
            self.setCellWidget(row, 0, _centered(check))
            self._checks.append(check)

            source = QtWidgets.QComboBox()
            source.addItems(CELL_TRIGGERS)
            source.setToolTip(
                'Trigger source: none, one of 4 aux timers, one of 2 digital inputs, '
                'or a bit of the compare register (see Compare)')
            self.setCellWidget(row, 1, source)
            self._sources.append(source)

            compare = QtWidgets.QSpinBox()
            compare.setRange(0, 22)
            compare.setToolTip(
                'Only used when source = compare: bit 0-20 of the compare/status '
                'register, 21 = counter overflow, 22 = rising edge')
            self.setCellWidget(row, 2, compare)
            self._compares.append(compare)

    def set_cells(self, cells):
        """Load from a get-params `cells` value: `{"<idx>": {source, compare}}`."""
        self._loading = True
        for row in range(N_SLOTS):
            entry = cells.get(str(row))
            enabled = entry is not None
            self._checks[row].setChecked(enabled)
            if enabled:
                index = self._sources[row].findText(entry['source'])
                if index >= 0:
                    self._sources[row].setCurrentIndex(index)
                self._compares[row].setValue(entry['compare'])
            self._sources[row].setEnabled(enabled)
            self._compares[row].setEnabled(enabled)
        self._loading = False

    def _on_toggle(self, *_args):
        if self._loading:
            return
        for row in range(N_SLOTS):
            enabled = self._checks[row].isChecked()
            self._sources[row].setEnabled(enabled)
            self._compares[row].setEnabled(enabled)

    def current(self):
        return {
            str(row): {'source': self._sources[row].currentText(),
                       'compare': self._compares[row].value()}
            for row in range(N_SLOTS) if self._checks[row].isChecked()
        }


class MesyModulesTable(QtWidgets.QTableWidget):
    """8 fixed rows: Detected / Configure / Type / Threshold / Gain 0-15.

    Loads/reports a dict shaped like the `modules` param value, e.g.
    `{"3": {"type": "mpsd", "threshold": 42, "gain": [1,2,3,4,5,6,7,8]}}` --
    "Configure" means umami manages that slot, not that the module is
    enabled; unset rows are omitted. "Detected" is read-only, from `found`.
    `gain` from get-params can be a single number (loaded into every channel
    spinbox) or an already-per-channel array; `current()` always reports a
    full per-channel array (functionally identical to a uniform value on the
    wire, just explicit). `mpsd` modules have 8 channels, `mstd` 16 -- the
    unused upper gain columns are disabled for an `mpsd` row. Edits are local
    until read via `current()`.
    """

    N_GAIN_CHANS = 16
    CHANNELS_BY_TYPE: typing.ClassVar = {'mpsd': 8, 'mstd': 16}

    def __init__(self):
        super().__init__(N_SLOTS, 4 + self.N_GAIN_CHANS)
        self.setHorizontalHeaderLabels(
            ['Detected', 'Configure', 'Type', 'Threshold']
            + [f'Gain {c}' for c in range(self.N_GAIN_CHANS)])
        self.setVerticalHeaderLabels([f'Module {i}' for i in range(N_SLOTS)])
        self.horizontalHeader().setSectionResizeMode(
            QtWidgets.QHeaderView.ResizeMode.Stretch)
        self._loading = False
        self._detected = []
        self._checks = []
        self._types = []
        self._thresholds = []
        self._gains = []  # one list of N_GAIN_CHANS QSpinBox per row
        for row in range(N_SLOTS):
            detected = QtWidgets.QTableWidgetItem('-')
            detected.setFlags(detected.flags() & ~QtCore.Qt.ItemFlag.ItemIsEditable)
            self.setItem(row, 0, detected)
            self._detected.append(detected)

            check = QtWidgets.QCheckBox()
            check.setToolTip("Manage this module's settings from umami")
            check.toggled.connect(self._on_toggle)
            self.setCellWidget(row, 1, _centered(check))
            self._checks.append(check)

            combo = QtWidgets.QComboBox()
            combo.addItems(MODULE_TYPES)
            combo.currentIndexChanged.connect(
                lambda _idx, r=row: self._update_gain_enabled(r))
            self.setCellWidget(row, 2, combo)
            self._types.append(combo)

            threshold = QtWidgets.QSpinBox()
            threshold.setRange(0, 0xFFFF)
            self.setCellWidget(row, 3, threshold)
            self._thresholds.append(threshold)

            row_gains = []
            for chan in range(self.N_GAIN_CHANS):
                gain = QtWidgets.QSpinBox()
                gain.setRange(0, 0xFFFF)
                gain.setToolTip(f'Gain for channel/tube {chan}')
                self.setCellWidget(row, 4 + chan, gain)
                row_gains.append(gain)
            self._gains.append(row_gains)

    def _channels(self, row):
        return self.CHANNELS_BY_TYPE[self._types[row].currentText()]

    def _update_gain_enabled(self, row):
        if self._loading:
            return
        enabled = self._checks[row].isChecked()
        channels = self._channels(row)
        for chan, spin in enumerate(self._gains[row]):
            spin.setEnabled(enabled and chan < channels)

    def set_detected_types(self, found):
        """Load the 8 `{mod_type, fw_version}` entries from the `found` param."""
        for row, entry in enumerate(found):
            major, minor = entry['fw_version']
            text = entry['mod_type']
            if major or minor:
                text += f' (v{major}.{minor})'
            self._detected[row].setText(text)

    def set_modules(self, modules):
        """Load from a get-params `modules` value: `{"<idx>": {type, ...}}`."""
        self._loading = True
        for row in range(N_SLOTS):
            entry = modules.get(str(row))
            enabled = entry is not None
            self._checks[row].setChecked(enabled)
            if enabled:
                index = self._types[row].findText(entry['type'])
                if index >= 0:
                    self._types[row].setCurrentIndex(index)
                self._thresholds[row].setValue(entry['threshold'])
                gain = entry['gain']
                gains = gain if isinstance(gain, list) else [gain] * self._channels(row)
                for chan, value in enumerate(gains):
                    self._gains[row][chan].setValue(value)
            self._types[row].setEnabled(enabled)
            self._thresholds[row].setEnabled(enabled)
        self._loading = False
        for row in range(N_SLOTS):
            self._update_gain_enabled(row)

    def _on_toggle(self, *_args):
        if self._loading:
            return
        for row in range(N_SLOTS):
            enabled = self._checks[row].isChecked()
            self._types[row].setEnabled(enabled)
            self._thresholds[row].setEnabled(enabled)
            self._update_gain_enabled(row)

    def current(self):
        result = {}
        for row in range(N_SLOTS):
            if self._checks[row].isChecked():
                gains = self._gains[row][:self._channels(row)]
                result[str(row)] = {
                    'type': self._types[row].currentText(),
                    'threshold': self._thresholds[row].value(),
                    'gain': [spin.value() for spin in gains],
                }
        return result


class MesyPulserTable(QtWidgets.QTableWidget):
    """8 fixed rows (module index 0-7): Configure / On / Channel / Position / Amplitude.

    Loads/reports a dict shaped like the `pulser` param value, e.g.
    `{"2": {"chan": 3, "pos": "middle", "amp": 60, "on": true}}` --
    "Configure" means umami manages that slot's pulser setting, not that
    the pulser is currently injecting; "On" is the actual toggle, part of
    the pushed value like the other fields. Unset rows are omitted. Edits
    are local until read via `current()`.
    """

    def __init__(self):
        super().__init__(N_SLOTS, 5)
        self.setHorizontalHeaderLabels(
            ['Configure', 'On', 'Channel', 'Position', 'Amplitude'])
        self.setVerticalHeaderLabels([f'Module {i}' for i in range(N_SLOTS)])
        self.horizontalHeader().setSectionResizeMode(
            QtWidgets.QHeaderView.ResizeMode.Stretch)
        self._loading = False
        self._checks = []
        self._ons = []
        self._chans = []
        self._positions = []
        self._amps = []
        for row in range(N_SLOTS):
            check = QtWidgets.QCheckBox()
            check.setToolTip("Manage this module's pulser from umami")
            check.toggled.connect(self._on_toggle)
            self.setCellWidget(row, 0, _centered(check))
            self._checks.append(check)

            on = QtWidgets.QCheckBox()
            on.setToolTip('Actually inject test pulses -- leave off when not testing')
            self.setCellWidget(row, 1, _centered(on))
            self._ons.append(on)

            chan = QtWidgets.QSpinBox()
            chan.setRange(0, N_SLOTS - 1)
            chan.setToolTip('Channel to pulse')
            self.setCellWidget(row, 2, chan)
            self._chans.append(chan)

            pos = QtWidgets.QComboBox()
            pos.addItems(PULSER_POSITIONS)
            self.setCellWidget(row, 3, pos)
            self._positions.append(pos)

            amp = QtWidgets.QSpinBox()
            amp.setRange(0, 255)
            self.setCellWidget(row, 4, amp)
            self._amps.append(amp)

    def set_pulser(self, pulser):
        """Load from a get-params `pulser` value: `{"<idx>": {chan, pos, amp, on}}`."""
        self._loading = True
        for row in range(N_SLOTS):
            entry = pulser.get(str(row))
            configured = entry is not None
            self._checks[row].setChecked(configured)
            if configured:
                self._chans[row].setValue(entry['chan'])
                index = self._positions[row].findText(entry['pos'])
                if index >= 0:
                    self._positions[row].setCurrentIndex(index)
                self._amps[row].setValue(entry['amp'])
                self._ons[row].setChecked(entry['on'])
            self._chans[row].setEnabled(configured)
            self._positions[row].setEnabled(configured)
            self._amps[row].setEnabled(configured)
            self._ons[row].setEnabled(configured)
        self._loading = False

    def _on_toggle(self, *_args):
        if self._loading:
            return
        for row in range(N_SLOTS):
            configured = self._checks[row].isChecked()
            self._chans[row].setEnabled(configured)
            self._positions[row].setEnabled(configured)
            self._amps[row].setEnabled(configured)
            self._ons[row].setEnabled(configured)

    def current(self):
        return {
            str(row): {'chan': self._chans[row].value(),
                       'pos': self._positions[row].currentText(),
                       'amp': self._amps[row].value(),
                       'on': self._ons[row].isChecked()}
            for row in range(N_SLOTS) if self._checks[row].isChecked()
        }


class McpdConfigWindow(QtWidgets.QWidget):
    """Separate window with one tab per detected Mesytec MCPD input.

    Discovers mesy inputs via `discover_mesy_inputs()`. Follows the same
    "closing just hides" pattern as the aux-histogram window.
    """

    applied = QtCore.pyqtSignal()

    def __init__(self, client):
        super().__init__()
        self.client = client
        self.setWindowTitle('UMAMI MCPD setup')
        self.resize(1200, 800)

        self._names = []  # mesy input names last seen, in tab order
        self._tables = {}

        self.tabs = QtWidgets.QTabWidget()

        close_btn = QtWidgets.QPushButton('Close')
        close_btn.clicked.connect(self.close)
        refresh_btn = icon_button('refresh', 'Refresh')
        refresh_btn.clicked.connect(self.refresh)
        apply_btn = icon_button('apply', 'Apply')
        apply_btn.clicked.connect(self._apply_all)

        bottom_row = QtWidgets.QHBoxLayout()
        bottom_row.addWidget(close_btn)
        bottom_row.addStretch()
        bottom_row.addWidget(refresh_btn)
        bottom_row.addWidget(apply_btn)

        layout = QtWidgets.QVBoxLayout(self)
        layout.addWidget(self.tabs)
        layout.addLayout(bottom_row)

    def showEvent(self, event):  # noqa: N802
        super().showEvent(event)
        self.refresh()

    def _add_tab(self, name):
        page = QtWidgets.QWidget()
        page_layout = QtWidgets.QVBoxLayout(page)

        version_label = QtWidgets.QLabel('MCPD firmware: -')
        page_layout.addWidget(version_label)

        modules_box = QtWidgets.QGroupBox('Modules')
        modules_table = MesyModulesTable()
        QtWidgets.QVBoxLayout(modules_box).addWidget(modules_table)

        cells_box = QtWidgets.QGroupBox('Cells')
        cells_table = MesyCellsTable()
        QtWidgets.QVBoxLayout(cells_box).addWidget(cells_table)

        pulser_box = QtWidgets.QGroupBox('Pulser')
        pulser_table = MesyPulserTable()
        QtWidgets.QVBoxLayout(pulser_box).addWidget(pulser_table)

        bottom_row = QtWidgets.QHBoxLayout()
        bottom_row.addWidget(cells_box)
        bottom_row.addWidget(pulser_box)

        page_layout.addWidget(modules_box)
        page_layout.addLayout(bottom_row)
        self.tabs.addTab(page, name)
        return cells_table, modules_table, pulser_table, version_label

    def _apply_all(self):
        """Push every tab's cells/modules/pulser edits live, in one set_params call."""
        params = {}
        for name, tables in self._tables.items():
            cells_table, modules_table, pulser_table, _version_label = tables
            params[f'{name}.cells'] = cells_table.current()
            params[f'{name}.modules'] = modules_table.current()
            params[f'{name}.pulser'] = pulser_table.current()
        if params:
            self.client.set_params(params)
            self.applied.emit()

    def refresh(self):
        """Re-pull cells/modules/found/mcpd_version/pulser for every mesy input."""
        params = self.client.get_params(full=True)
        if params is None:
            return

        names = discover_mesy_inputs(params)
        if names != self._names:
            self.tabs.clear()
            self._names = names
            self._tables = {name: self._add_tab(name) for name in names}

        for name, tables in self._tables.items():
            cells_table, modules_table, pulser_table, version_label = tables
            cells = (params.get(f'{name}.cells') or {}).get('value') or {}
            modules = (params.get(f'{name}.modules') or {}).get('value') or {}
            pulser = (params.get(f'{name}.pulser') or {}).get('value') or {}
            found = ((params.get(f'{name}.found') or {}).get('value')
                     or [{'mod_type': '-', 'fw_version': (0, 0)}] * N_SLOTS)
            mcpd_version = (params.get(f'{name}.mcpd_version') or {}).get('value')
            cells_table.set_cells(cells)
            modules_table.set_modules(modules)
            modules_table.set_detected_types(found)
            pulser_table.set_pulser(pulser)
            if mcpd_version:
                cpu = mcpd_version['cpu']
                fpga = mcpd_version['fpga']
                version_label.setText(
                    f'MCPD firmware: CPU {cpu[0]}.{cpu[1]}, FPGA {fpga[0]}.{fpga[1]}')
            else:
                version_label.setText('MCPD firmware: -')
