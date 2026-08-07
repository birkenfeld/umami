# Part of the Unified Mechanism for Acquisition of Measured Intensity
# (UMAMI), see README and LICENSE files for more info.

"""Extended, per-run event/counter breakdown as a live-updating grid."""

from pyqtgraph.Qt import QtWidgets

from .icons import icon_button
from .status_panel import THIN_SPACE, format_elapsed, rich_text_width

MONITOR_COUNT = 5

# rows shown top to bottom; None is a separator line spanning all columns
ROW_NAMES = [
    None,
    'Counts in slice', 'Counts in ROI', None,
    'Total counts', 'Life time', None,
    'T-zero', *(f'Monitor {i}' for i in range(MONITOR_COUNT)), None,
    'Total events',
]


class EventsPanel(QtWidgets.QWidget):
    """Name/value/rate grid of counters, extending the compact status line.

    `set_roi_btn`/`clear_roi_btn` are exposed for `MainWindow` to wire up --
    picking a region on the main plot and drawing/persisting it there is
    that plot's business, this panel only displays the resulting ROI count.
    """

    def __init__(self):
        super().__init__()
        outer = QtWidgets.QVBoxLayout(self)
        outer.setContentsMargins(8, 8, 5, 5)

        layout = QtWidgets.QGridLayout()
        layout.setVerticalSpacing(int(layout.verticalSpacing() * 1.5))
        layout.setHorizontalSpacing(int(layout.horizontalSpacing() * 1.5))
        outer.addLayout(layout)

        outer.addSpacing(24)

        btn_row = QtWidgets.QHBoxLayout()
        self.set_roi_btn = icon_button('select', 'Set ROI')
        self.set_roi_btn.setCheckable(True)
        self.set_roi_btn.setToolTip(
            'Drag a rectangle on the histogram plot to set the ROI')
        btn_row.addWidget(self.set_roi_btn)
        self.clear_roi_btn = icon_button('remove_selection', 'Clear')
        btn_row.addWidget(self.clear_roi_btn)
        btn_row.addStretch()
        outer.addLayout(btn_row)
        outer.addStretch()

        header_font = self.font()
        header_font.setBold(True)
        for col, text in enumerate(['Counter', 'Value', 'Rate']):
            label = QtWidgets.QLabel(text)
            label.setFont(header_font)
            layout.addWidget(label, 0, col)

        self._value_labels = {}
        self._rate_labels = {}
        row = 1
        for name in ROW_NAMES:
            if name is None:
                layout.addWidget(self._separator(), row, 0, 1, 3)
                row += 1
                continue
            layout.addWidget(QtWidgets.QLabel(name), row, 0)
            value_label = QtWidgets.QLabel('-')
            layout.addWidget(value_label, row, 1)
            self._value_labels[name] = value_label
            rate_label = QtWidgets.QLabel('-')
            layout.addWidget(rate_label, row, 2)
            self._rate_labels[name] = rate_label
            row += 1

        # fix the value/rate columns' width to fit large counts without
        # the whole grid resizing/jumping as digits are added
        self._value_labels['Total events'].setMinimumWidth(
            rich_text_width(self.font(), '<b>100,000,000</b>'))
        self._rate_labels['Total events'].setMinimumWidth(
            rich_text_width(self.font(), f'10,000.0{THIN_SPACE}<b>k</b>/s'))

    @staticmethod
    def _separator():
        line = QtWidgets.QFrame()
        line.setFrameShape(QtWidgets.QFrame.Shape.HLine)
        line.setFrameShadow(QtWidgets.QFrame.Shadow.Sunken)
        return line

    @staticmethod
    def _fmt_rate(rate):
        if rate is None:
            return '-'
        if rate >= 1_000:
            return f'{rate/1_000:,.1f}{THIN_SPACE}<b>k</b>/s'
        return f'{rate:,.1f}{THIN_SPACE}/s'

    def update_counts(self, in_slice, roi, total_counts, total_events, tzero,
                      monitor_counts, lifetime_ns, rates):
        """Update every row's value/rate.

        `rates` is `(in_slice_rate, roi_rate, total_counts_rate,
        events_rate, tzero_rate, *monitor_rates)`; entries are `None` where
        not enough samples are available yet or the underlying counter
        decreased (e.g. a Clear happened mid-window).
        """
        (in_slice_rate, roi_rate, total_counts_rate, events_rate,
         tzero_rate, *monitor_rates) = rates

        def set_row(name, value, rate):
            self._value_labels[name].setText(f'<b>{value:,}</b>')
            self._rate_labels[name].setText(self._fmt_rate(rate))

        set_row('Counts in slice', in_slice, in_slice_rate)
        set_row('Counts in ROI', roi, roi_rate)
        set_row('Total counts', total_counts, total_counts_rate)
        set_row('Total events', total_events, events_rate)
        set_row('T-zero', tzero, tzero_rate)
        for i, (count, rate) in enumerate(zip(monitor_counts, monitor_rates)):
            set_row(f'Monitor {i}', count, rate)

        lifetime_s = lifetime_ns / 1_000_000_000
        self._value_labels['Life time'].setText(format_elapsed(int(lifetime_s)))
        avg_rate = total_counts / lifetime_s if lifetime_s > 0 else None
        self._rate_labels['Life time'].setText(self._fmt_rate(avg_rate))
