# Part of the Unified Mechanism for Acquisition of Measured Intensity
# (UMAMI), see README and LICENSE files for more info.

"""Mesytec MCPD live configuration: friendly per-slot cells/modules/pulser tables.

`MesyCellsTable`, `MesyModulesTable`, and `MesyPulserTable` only load/report
plain dicts shaped like the `cells`/`modules`/`pulser` get-params/set-params
values -- no client or networking dependency -- so they can be reused by an
offline config-file editor later, not just `McpdConfigWindow`. Edits are
local until explicitly applied (see `McpdConfigWindow`'s Apply button).
"""

from pyqtgraph.Qt import QtCore, QtWidgets

from .icons import icon_button

N_SLOTS = 8
MODULE_TYPES = ('mpsd', 'mstd')
PULSER_POSITIONS = ('left', 'right', 'middle')


def discover_mesy_inputs(params):
    """Names of mesy inputs present in a `full` get-params map, sorted.

    Identified via each module's `<name>._info` entry reporting kind
    "input" and type "mesy".
    """
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
    `{"1": {"source": 2, "compare": 5}}` -- "Configure" means umami manages
    that slot, not that the cell is enabled; unset rows are omitted. Edits
    are local until read via `current()`.
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

            source = QtWidgets.QSpinBox()
            source.setRange(0, 7)
            source.setToolTip('Trigger source (0 = no trigger, 7 = compare)')
            self.setCellWidget(row, 1, source)
            self._sources.append(source)

            compare = QtWidgets.QSpinBox()
            compare.setRange(0, 0xFFFF)
            self.setCellWidget(row, 2, compare)
            self._compares.append(compare)

    def set_cells(self, cells):
        """Load from a get-params `cells` value: `{"<idx>": {source, compare}}`."""
        self._loading = True
        for row in range(N_SLOTS):
            entry = cells.get(str(row))
            enabled = entry is not None
            self._checks[row].setChecked(enabled)
            self._sources[row].setValue(entry['source'] if enabled else 0)
            self._compares[row].setValue(entry['compare'] if enabled else 0)
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
            str(row): {'source': self._sources[row].value(),
                       'compare': self._compares[row].value()}
            for row in range(N_SLOTS) if self._checks[row].isChecked()
        }


class MesyModulesTable(QtWidgets.QTableWidget):
    """8 fixed rows (module index 0-7): Detected / Configure / Type / Threshold / Gain.

    Loads/reports a dict shaped like the `modules` param value, e.g.
    `{"3": {"type": "mpsd", "threshold": 42, "gain": 7}}` -- "Configure"
    means umami manages that slot, not that the module is enabled; unset
    rows are omitted. "Detected" is read-only, from `mod_types`. Edits are
    local until read via `current()`.
    """

    def __init__(self):
        super().__init__(N_SLOTS, 5)
        self.setHorizontalHeaderLabels(
            ['Detected', 'Configure', 'Type', 'Threshold', 'Gain'])
        self.setVerticalHeaderLabels([f'Module {i}' for i in range(N_SLOTS)])
        self.horizontalHeader().setSectionResizeMode(
            QtWidgets.QHeaderView.ResizeMode.Stretch)
        self._loading = False
        self._detected = []
        self._checks = []
        self._types = []
        self._thresholds = []
        self._gains = []
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
            self.setCellWidget(row, 2, combo)
            self._types.append(combo)

            threshold = QtWidgets.QSpinBox()
            threshold.setRange(0, 0xFFFF)
            self.setCellWidget(row, 3, threshold)
            self._thresholds.append(threshold)

            gain = QtWidgets.QSpinBox()
            gain.setRange(0, 0xFFFF)
            gain.setToolTip('Applies to all channels of this module')
            self.setCellWidget(row, 4, gain)
            self._gains.append(gain)

    def set_detected_types(self, mod_types):
        """`mod_types`: the 8 strings from the read-only `mod_types` param."""
        for row, name in enumerate(mod_types):
            self._detected[row].setText(name)

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
                self._gains[row].setValue(entry['gain'])
            self._types[row].setEnabled(enabled)
            self._thresholds[row].setEnabled(enabled)
            self._gains[row].setEnabled(enabled)
        self._loading = False

    def _on_toggle(self, *_args):
        if self._loading:
            return
        for row in range(N_SLOTS):
            enabled = self._checks[row].isChecked()
            self._types[row].setEnabled(enabled)
            self._thresholds[row].setEnabled(enabled)
            self._gains[row].setEnabled(enabled)

    def current(self):
        return {
            str(row): {'type': self._types[row].currentText(),
                       'threshold': self._thresholds[row].value(),
                       'gain': self._gains[row].value()}
            for row in range(N_SLOTS) if self._checks[row].isChecked()
        }


class MesyPulserTable(QtWidgets.QTableWidget):
    """8 fixed rows (module index 0-7): Configure / Channel / Position / Amplitude / On.

    Loads/reports a dict shaped like the `pulser` param value, e.g.
    `{"2": {"chan": 3, "pos": "middle", "amp": 60, "on": true}}` --
    "Configure" means umami manages that slot's pulser setting, not that
    the pulser is currently injecting; "On" is the actual test-pulse
    toggle, part of the pushed value like the other fields. Unset rows are
    omitted. Edits are local until read via `current()`.
    """

    def __init__(self):
        super().__init__(N_SLOTS, 5)
        self.setHorizontalHeaderLabels(
            ['Configure', 'Channel', 'Position', 'Amplitude', 'On'])
        self.setVerticalHeaderLabels([f'Module {i}' for i in range(N_SLOTS)])
        self.horizontalHeader().setSectionResizeMode(
            QtWidgets.QHeaderView.ResizeMode.Stretch)
        self._loading = False
        self._checks = []
        self._chans = []
        self._positions = []
        self._amps = []
        self._ons = []
        for row in range(N_SLOTS):
            check = QtWidgets.QCheckBox()
            check.setToolTip("Manage this module's pulser from umami")
            check.toggled.connect(self._on_toggle)
            self.setCellWidget(row, 0, _centered(check))
            self._checks.append(check)

            chan = QtWidgets.QSpinBox()
            chan.setRange(0, N_SLOTS)
            chan.setToolTip(f'Channel to pulse, or {N_SLOTS} for all channels')
            self.setCellWidget(row, 1, chan)
            self._chans.append(chan)

            pos = QtWidgets.QComboBox()
            pos.addItems(PULSER_POSITIONS)
            self.setCellWidget(row, 2, pos)
            self._positions.append(pos)

            amp = QtWidgets.QSpinBox()
            amp.setRange(0, 255)
            self.setCellWidget(row, 3, amp)
            self._amps.append(amp)

            on = QtWidgets.QCheckBox()
            on.setToolTip('Actually inject test pulses -- leave off when not testing')
            self.setCellWidget(row, 4, _centered(on))
            self._ons.append(on)

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

    def __init__(self, client):
        super().__init__()
        self.client = client
        self.setWindowTitle('UMAMI MCPD setup')
        self.resize(700, 500)

        self._names = []  # mesy input names last seen, in tab order
        self._tables = {}

        self.tabs = QtWidgets.QTabWidget()

        refresh_btn = icon_button('refresh', 'Refresh')
        refresh_btn.clicked.connect(self.refresh)
        apply_btn = icon_button('apply', 'Apply')
        apply_btn.clicked.connect(self._apply_all)

        bottom_row = QtWidgets.QHBoxLayout()
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

        cells_box = QtWidgets.QGroupBox('Cells')
        cells_table = MesyCellsTable()
        QtWidgets.QVBoxLayout(cells_box).addWidget(cells_table)

        modules_box = QtWidgets.QGroupBox('Modules')
        modules_table = MesyModulesTable()
        QtWidgets.QVBoxLayout(modules_box).addWidget(modules_table)

        pulser_box = QtWidgets.QGroupBox('Pulser (test-pulse injection)')
        pulser_table = MesyPulserTable()
        QtWidgets.QVBoxLayout(pulser_box).addWidget(pulser_table)

        page_layout.addWidget(cells_box)
        page_layout.addWidget(modules_box)
        page_layout.addWidget(pulser_box)
        self.tabs.addTab(page, name)
        return cells_table, modules_table, pulser_table

    def _apply_all(self):
        """Push every tab's cells/modules/pulser edits live, in one set_params call."""
        params = {}
        for name, (cells_table, modules_table, pulser_table) in self._tables.items():
            params[f'{name}.cells'] = cells_table.current()
            params[f'{name}.modules'] = modules_table.current()
            params[f'{name}.pulser'] = pulser_table.current()
        if params:
            self.client.set_params(params)

    def refresh(self):
        """Re-pull cells/modules/mod_types/pulser for every detected mesy input."""
        params = self.client.get_params(full=True)
        if params is None:
            return

        names = discover_mesy_inputs(params)
        if names != self._names:
            self.tabs.clear()
            self._names = names
            self._tables = {name: self._add_tab(name) for name in names}

        for name, (cells_table, modules_table, pulser_table) in self._tables.items():
            cells = (params.get(f'{name}.cells') or {}).get('value') or {}
            modules = (params.get(f'{name}.modules') or {}).get('value') or {}
            pulser = (params.get(f'{name}.pulser') or {}).get('value') or {}
            mod_types = ((params.get(f'{name}.mod_types') or {}).get('value')
                         or ['-'] * N_SLOTS)
            cells_table.set_cells(cells)
            modules_table.set_modules(modules)
            modules_table.set_detected_types(mod_types)
            pulser_table.set_pulser(pulser)
