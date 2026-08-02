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

        self.current_label = QtWidgets.QLabel('-')
        self.current_label.setWordWrap(True)
        layout.addRow('Current bins:', self.current_label)

        self.number_radio = QtWidgets.QRadioButton('Number + width -> total')
        self.total_radio = QtWidgets.QRadioButton('Number + total -> width')
        self.width_radio = QtWidgets.QRadioButton('Width + total -> number')
        self.number_radio.setChecked(True)
        for radio in (self.number_radio, self.total_radio, self.width_radio):
            radio.toggled.connect(self._update_enabled)
            layout.addRow(radio)

        self.unit_combo = QtWidgets.QComboBox()
        self.unit_combo.addItems(list(UNIT_FACTORS))
        self.unit_combo.setCurrentText(DEFAULT_UNIT)
        self.unit_combo.currentTextChanged.connect(self._update_suffixes)
        layout.addRow('Unit:', self.unit_combo)

        self.number_spin = QtWidgets.QSpinBox()
        self.number_spin.setRange(1, 1_000_000)
        layout.addRow('Number of bins:', self.number_spin)

        self.width_spin = QtWidgets.QDoubleSpinBox()
        self.width_spin.setDecimals(0)
        self.width_spin.setRange(1, 1e15)
        layout.addRow('Bin width:', self.width_spin)

        self.total_spin = QtWidgets.QDoubleSpinBox()
        self.total_spin.setDecimals(0)
        self.total_spin.setRange(1, 1e15)
        layout.addRow('Total time:', self.total_spin)

        offset_row = QtWidgets.QHBoxLayout()
        self.offset_check = QtWidgets.QCheckBox('Offset (extra first bin)')
        self.offset_spin = QtWidgets.QDoubleSpinBox()
        self.offset_spin.setDecimals(0)
        self.offset_spin.setRange(1, 1e15)
        self.offset_spin.setEnabled(False)
        self.offset_check.toggled.connect(self.offset_spin.setEnabled)
        offset_row.addWidget(self.offset_check)
        offset_row.addWidget(self.offset_spin)
        layout.addRow(offset_row)

        generate_btn = QtWidgets.QPushButton('Generate')
        generate_btn.clicked.connect(self._generate)
        layout.addRow(generate_btn)

        self.preview = QtWidgets.QPlainTextEdit()
        self.preview.setReadOnly(True)
        self.preview.setMaximumHeight(80)
        layout.addRow('New bins:', self.preview)

        self._update_enabled()
        self._update_suffixes()

    def _mode(self):
        if self.total_radio.isChecked():
            return MODE_NUMBER_TOTAL
        if self.width_radio.isChecked():
            return MODE_WIDTH_TOTAL
        return MODE_NUMBER_WIDTH

    def _update_enabled(self, *_args):
        mode = self._mode()
        self.number_spin.setEnabled(mode != MODE_WIDTH_TOTAL)
        self.width_spin.setEnabled(mode != MODE_NUMBER_TOTAL)
        self.total_spin.setEnabled(mode != MODE_NUMBER_WIDTH)

    def _update_suffixes(self, *_args):
        suffix = f' {self.unit_combo.currentText()}'
        self.width_spin.setSuffix(suffix)
        self.total_spin.setSuffix(suffix)
        self.offset_spin.setSuffix(suffix)

    def _generate(self):
        mode = self._mode()
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

    def _apply_all(self):
        """Push every tab's freshly generated bins live, in one set_params call."""
        params = {}
        for name, widget in self._tabs_by_name.items():
            bins = widget.current()
            if bins is not None:
                params[f'{name}.time_bins'] = bins
        if params:
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
