# Part of the Unified Mechanism for Acquisition of Measured Intensity
# (UMAMI), see README and LICENSE files for more info.

"""Extended, per-run event/counter breakdown as a live-updating grid."""

from pyqtgraph.Qt import QtWidgets

from .status_panel import THIN_SPACE, format_elapsed

MONITOR_COUNT = 5

# rows shown top to bottom; 'Life time' has no rate column (see _build_rows)
ROW_NAMES = [
    'Counts in slice', 'Counts in ROI', 'Total counts', 'Life time',
    'Total events', 'T-zero', *(f'Monitor {i}' for i in range(MONITOR_COUNT)),
]


class EventsPanel(QtWidgets.QWidget):
    """Name/value/rate grid of counters, extending the compact status line."""

    def __init__(self):
        super().__init__()
        layout = QtWidgets.QGridLayout(self)
        layout.setContentsMargins(8, 13, 0, 0)
        layout.setVerticalSpacing(int(layout.verticalSpacing() * 1.5))

        header_font = self.font()
        header_font.setBold(True)
        for col, text in enumerate(['Counter', 'Value', 'Rate']):
            label = QtWidgets.QLabel(text)
            label.setFont(header_font)
            layout.addWidget(label, 0, col)

        self._value_labels = {}
        self._rate_labels = {}
        for row, name in enumerate(ROW_NAMES, start=1):
            layout.addWidget(QtWidgets.QLabel(name), row, 0)
            value_label = QtWidgets.QLabel('-')
            layout.addWidget(value_label, row, 1)
            self._value_labels[name] = value_label
            if name != 'Life time':
                rate_label = QtWidgets.QLabel('-')
                layout.addWidget(rate_label, row, 2)
                self._rate_labels[name] = rate_label

        # ROI counting isn't implemented yet -- pinned at zero, never updated
        self._value_labels['Counts in ROI'].setText('<b>0</b>')
        self._rate_labels['Counts in ROI'].setText(self._fmt_rate(None))

        layout.setRowStretch(len(ROW_NAMES) + 1, 1)

    @staticmethod
    def _fmt_rate(rate):
        return f'{rate:,.1f}{THIN_SPACE}/s' if rate is not None else '-'

    def update_counts(self, in_slice, in_slice_rate, total_counts, total_events,
                      tzero, monitor_counts, lifetime_ns, rates):
        """Update every row's value/rate.

        `in_slice_rate` is computed by the caller over the same trailing
        window as the compact status line's rate, so both stay consistent.
        `rates` is `(total_counts_rate, events_rate, tzero_rate,
        *monitor_rates)`; entries are `None` where not enough samples are
        available yet or the underlying counter decreased (e.g. a Clear
        happened mid-window).
        """
        total_counts_rate, events_rate, tzero_rate, *monitor_rates = rates

        def set_row(name, value, rate):
            self._value_labels[name].setText(f'<b>{value:,}</b>')
            self._rate_labels[name].setText(self._fmt_rate(rate))

        set_row('Counts in slice', in_slice, in_slice_rate)
        set_row('Total counts', total_counts, total_counts_rate)
        set_row('Total events', total_events, events_rate)
        set_row('T-zero', tzero, tzero_rate)
        for i, (count, rate) in enumerate(zip(monitor_counts, monitor_rates)):
            set_row(f'Monitor {i}', count, rate)

        lifetime_s = int(lifetime_ns / 1_000_000_000)
        self._value_labels['Life time'].setText(format_elapsed(lifetime_s))
