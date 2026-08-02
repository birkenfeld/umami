# Part of the Unified Mechanism for Acquisition of Measured Intensity
# (UMAMI), see README and LICENSE files for more info.

"""TOF histogram bin configuration: friendly equal-width-bin generator.

`generate_time_bins()` only computes a plain list of ints -- no client or
networking dependency -- so it (and `TofBinsWidget`) can be reused by an
offline config-file editor later, not just `TofConfigWindow`.
"""

from pyqtgraph.Qt import QtWidgets

from .icons import icon_button

MODE_NUMBER_WIDTH = 0
MODE_NUMBER_TOTAL = 1
MODE_WIDTH_TOTAL = 2

# ns per displayed unit -- time_bins is always in ns, entry fields let the
# user pick a more convenient unit to type numbers in.
UNIT_FACTORS = {'ns': 1, 'µs': 1_000, 'ms': 1_000_000}
DEFAULT_UNIT = 'µs'


def discover_tof_recipes(params):
    """Names of histo_tof recipes/modes in a `full` get-params map, sorted.

    Identified via each module's `<name>._info` entry reporting kind
    "recipe" and type "histo_tof".
    """
    return sorted(
        key[:-len('._info')] for key, info in params.items()
        if key.endswith('._info') and info['kind'] == 'recipe'
        and info['type'] == 'histo_tof'
    )


def generate_time_bins(mode, number=None, width=None, total=None, offset=0):
    """Generate a list of nanosecond bin-end times, as `time_bins` expects.

    Cumulative, no need to include a trailing overflow bin (umami adds one).
    `number` counts only the equal-width bins -- a nonzero `offset` adds one
    more, extra leading bin of its own width before them. Exactly two of
    number/width/total are used, matching `mode`:

    - MODE_NUMBER_WIDTH: `number` and `width` given, `total` is derived.
    - MODE_NUMBER_TOTAL: `number` and `total` given, `width` is derived.
    - MODE_WIDTH_TOTAL: `width` and `total` given, `number` is derived.
    """
    if mode == MODE_NUMBER_WIDTH:
        if number < 1 or width <= 0:
            raise ValueError('Number of bins and width must be positive')
    elif mode == MODE_NUMBER_TOTAL:
        if number < 1 or total <= offset:
            raise ValueError('Number of bins must be positive, total > offset')
        width = (total - offset) / number
    elif mode == MODE_WIDTH_TOTAL:
        if width <= 0 or total <= offset:
            raise ValueError('Width must be positive, total > offset')
        number = max(1, round((total - offset) / width))
    else:
        raise ValueError(f'Unknown mode {mode!r}')

    bins = [round(offset)] if offset else []
    bins.extend(round(offset + i * width) for i in range(1, number + 1))
    return bins


class TofBinsWidget(QtWidgets.QWidget):
    """Equal-width `time_bins` generator for one histo_tof mode.

    Local state only -- load the current value via `set_current()`, read a
    freshly generated one via `current()` (`None` until "Generate" is
    clicked at least once). The caller (an Apply button, typically) decides
    when to push it.
    """

    def __init__(self):
        super().__init__()
        self._bins = None

        layout = QtWidgets.QFormLayout(self)

        self.unit_combo = QtWidgets.QComboBox()
        self.unit_combo.addItems(list(UNIT_FACTORS))
        self.unit_combo.setCurrentText(DEFAULT_UNIT)
        self.unit_combo.currentTextChanged.connect(self._update_suffixes)
        layout.addRow('Unit:', self.unit_combo)

        # Which two of number/width/total to use is picked implicitly: each
        # starts unchecked, and editing a field checks its own box (the user
        # can also un/check by hand). Exactly two checked selects the mode,
        # the unchecked one is the value `generate_time_bins()` derives.
        self.number_spin = QtWidgets.QSpinBox()
        self.number_spin.setRange(1, 1_000_000)
        self.number_check, number_row = self._given_field(self.number_spin)
        layout.addRow('Number of bins:', number_row)

        self.width_spin = QtWidgets.QDoubleSpinBox()
        self.width_spin.setDecimals(0)
        self.width_spin.setRange(1, 1e15)
        self.width_check, width_row = self._given_field(self.width_spin)
        layout.addRow('Bin width:', width_row)

        self.total_spin = QtWidgets.QDoubleSpinBox()
        self.total_spin.setDecimals(0)
        self.total_spin.setRange(1, 1e15)
        self.total_check, total_row = self._given_field(self.total_spin)
        layout.addRow('Total time:', total_row)

        self.offset_spin = QtWidgets.QDoubleSpinBox()
        self.offset_spin.setDecimals(0)
        self.offset_spin.setRange(1, 1e15)
        self.offset_spin.setEnabled(False)
        self.offset_check = QtWidgets.QCheckBox()
        self.offset_check.toggled.connect(self.offset_spin.setEnabled)
        layout.addRow('Offset (extra bin):', self._field_row(self.offset_check, self.offset_spin))

        generate_btn = QtWidgets.QPushButton('Generate')
        generate_btn.clicked.connect(self._generate)
        layout.addRow(generate_btn)

        self.preview = QtWidgets.QPlainTextEdit()
        self.preview.setReadOnly(True)
        self.preview.setMaximumHeight(80)
        layout.addRow('New bins:', self.preview)

        self._update_suffixes()

    @staticmethod
    def _field_row(checkbox, spin):
        """A checkbox + `spin` row, aligned like every other field row."""
        row = QtWidgets.QHBoxLayout()
        row.addWidget(checkbox)
        row.addWidget(spin)
        return row

    @classmethod
    def _given_field(cls, spin):
        """Build a checkbox + `spin` row; editing `spin` auto-checks the box."""
        checkbox = QtWidgets.QCheckBox()
        spin.valueChanged.connect(lambda _v: checkbox.setChecked(True))
        return checkbox, cls._field_row(checkbox, spin)

    def _mode(self):
        """Mode implied by which two of the three checkboxes are checked, or `None`."""
        given = (self.number_check.isChecked(), self.width_check.isChecked(),
                 self.total_check.isChecked())
        if sum(given) != 2:
            return None
        if not given[2]:
            return MODE_NUMBER_WIDTH
        if not given[1]:
            return MODE_NUMBER_TOTAL
        return MODE_WIDTH_TOTAL

    def _update_suffixes(self, *_args):
        suffix = f' {self.unit_combo.currentText()}'
        self.width_spin.setSuffix(suffix)
        self.total_spin.setSuffix(suffix)
        self.offset_spin.setSuffix(suffix)

    def _generate(self):
        mode = self._mode()
        if mode is None:
            QtWidgets.QMessageBox.warning(
                self, 'Invalid input', 'Check exactly two of number/width/total.')
            return
        factor = UNIT_FACTORS[self.unit_combo.currentText()]
        checked = self.offset_check.isChecked()
        offset = self.offset_spin.value() * factor if checked else 0
        try:
            self._bins = generate_time_bins(
                mode, number=self.number_spin.value(),
                width=self.width_spin.value() * factor,
                total=self.total_spin.value() * factor, offset=offset)
        except ValueError as e:
            QtWidgets.QMessageBox.warning(self, 'Invalid input', str(e))
            return
        self.preview.setPlainText(str(self._bins))

    def set_current(self, bins):
        """Load from a get-params `time_bins` value (list of ints)."""
        self.current_label.setText(str(bins))

    def current(self):
        """Return the last-generated bins, or `None` if "Generate" was never clicked."""
        return self._bins


class TofConfigWindow(QtWidgets.QWidget):
    """Separate window with one tab per detected histo_tof mode.

    Discovers modes via `discover_tof_recipes()`. Follows the same
    "closing just hides" pattern as the other quick-setup windows.
    """

    def __init__(self, client):
        super().__init__()
        self.client = client
        self.setWindowTitle('UMAMI TOF setup')
        self.resize(500, 500)

        self._names = []
        self._tabs_by_name = {}

        self.tabs = QtWidgets.QTabWidget()

        close_btn = QtWidgets.QPushButton('Close')
        close_btn.clicked.connect(self.close)
        refresh_btn = icon_button('refresh', 'Refresh')
        refresh_btn.clicked.connect(self.refresh)
        apply_btn = icon_button('apply', 'Apply')
        apply_btn.setToolTip(
            'Push each tab\'s generated bins live -- tabs where "Generate"\n'
            'was not clicked (since the last Refresh) are left untouched.')
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

    def _apply_all(self):
        """Push every tab's freshly generated bins live, in one set_params call.

        Tabs where "Generate" was never clicked contribute nothing -- if that
        leaves nothing to push at all, say so instead of silently no-op-ing.
        """
        params = {}
        for name, widget in self._tabs_by_name.items():
            bins = widget.current()
            if bins is not None:
                params[f'{name}.time_bins'] = bins
        if not params:
            QtWidgets.QMessageBox.information(
                self, 'Nothing to apply',
                'Click "Generate" on at least one tab first.')
            return
        self.client.set_params(params)

    def refresh(self):
        """Re-pull the current time_bins for every detected histo_tof mode."""
        params = self.client.get_params(full=True)
        if params is None:
            return

        names = discover_tof_recipes(params)
        if names != self._names:
            self.tabs.clear()
            self._names = names
            self._tabs_by_name = {}
            for name in names:
                widget = TofBinsWidget()
                self.tabs.addTab(widget, name)
                self._tabs_by_name[name] = widget

        for name, widget in self._tabs_by_name.items():
            bins_info = params.get(f'{name}.time_bins') or {}
            widget.set_current(bins_info.get('value') or [])
