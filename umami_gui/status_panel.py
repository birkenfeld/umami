# Part of the Unified Mechanism for Acquisition of Measured Intensity
# (UMAMI), see README and LICENSE files for more info.

"""Connection/mode/run/per-input state indicator panel."""

from typing import ClassVar

from pyqtgraph.Qt import QtCore, QtGui, QtWidgets

from .icons import is_dark_mode, load_icon

CONN_ICON_SIZE = QtCore.QSize(16, 16)


class StatusPanel(QtWidgets.QFrame):
    """Connection, mode, run/counts, and per-input state at a glance."""

    STATE_COLORS: ClassVar = [
        {'idle': 'gray', 'running': '#00c853', 'ended': 'blue'},
        {'idle': '#aaaaaa', 'running': '#00e676', 'ended': '#64b5f6'},
    ]
    CONNECTED_COLOR = ('green', '#66bb6a')
    ERROR_COLOR = ('red', '#ef5350')
    DEFAULT_TEXT = ('#333333', '#cccccc')
    # explicit QToolTip colors -- the transparent background stylesheet set
    # on the LED row below otherwise leaks into tooltip rendering too
    TOOLTIP_STYLE: ClassVar = [
        ('QToolTip { background-color: #ffffe1; color: black; '
         'border: 1px solid #888; }'),
        ('QToolTip { background-color: #3c3c3c; color: #eeeeee; '
         'border: 1px solid #888; }'),
    ]
    LED_SIZE = 16
    LED_SIZE_MULTI_ROW = 12  # once wrapped to >1 row, shrink to fit more
    LEDS_PER_ROW = 10
    VALUE_FONT_SIZE_FACTOR = 1.2

    def __init__(self):
        super().__init__()
        dark = is_dark_mode()
        self.state_colors = self.STATE_COLORS[dark]
        self.connected_color = self.CONNECTED_COLOR[dark]
        self.error_color = self.ERROR_COLOR[dark]
        self.default_text_color = self.DEFAULT_TEXT[dark]
        # applies to any tooltip triggered by a descendant (e.g. the LEDs
        # below) -- see the TOOLTIP_STYLE comment for why this is needed
        self.setStyleSheet(self.TOOLTIP_STYLE[dark])
        self.setLayout(QtWidgets.QHBoxLayout())
        self.layout().setContentsMargins(8, 2, 13, 2)
        self.setSizePolicy(QtWidgets.QSizePolicy.Policy.Preferred,
                            QtWidgets.QSizePolicy.Policy.Fixed)

        value_font = self.font()
        value_font.setPointSize(
            int(value_font.pointSize() * self.VALUE_FONT_SIZE_FACTOR))

        self.conn_icon = QtWidgets.QLabel()
        self.layout().addWidget(self.conn_icon)
        self.conn_label = QtWidgets.QLabel()
        self.layout().addWidget(self.conn_label)
        self.layout().addSpacing(20)

        self.mode_label = QtWidgets.QLabel('mode: <b>-</b>')
        self.mode_label.setFont(value_font)
        self.layout().addWidget(self.mode_label)
        self.layout().addSpacing(20)

        self.run_label = QtWidgets.QLabel('run: <b>-</b>')
        self.run_label.setFont(value_font)
        self.layout().addWidget(self.run_label)
        self.layout().addSpacing(20)

        self.time_label = QtWidgets.QLabel('time: <b>-</b>')
        self.time_label.setFont(value_font)
        self.layout().addWidget(self.time_label)
        self.layout().addSpacing(20)

        self.total_label = QtWidgets.QLabel('total: <b>-</b>')
        self.total_label.setFont(value_font)
        # a stable minimum width up to 4 digits avoids the whole status row
        # visibly reflowing on every order-of-magnitude count increase
        self.total_label.setMinimumWidth(
            self._rich_text_width(value_font, 'total: <b>1,000</b> cts'))
        self.layout().addWidget(self.total_label)
        self.layout().addSpacing(20)

        self.rate_label = QtWidgets.QLabel('rate: <b>-</b>')
        self.rate_label.setFont(value_font)
        self.layout().addWidget(self.rate_label)
        self.layout().addSpacing(20)

        # a config can have many inputs; one LED-style dot per input, wrapping
        # to a new row every LEDS_PER_ROW -- never scrolled/clipped, all are
        # always shown, so the panel grows taller instead. The whole
        # (possibly multi-row) block stays pinned to the right edge via the
        # layout's own alignment.
        inputs_widget = QtWidgets.QWidget()
        self.inputs_layout = QtWidgets.QGridLayout(inputs_widget)
        self.inputs_layout.setContentsMargins(0, 0, 0, 0)
        self.inputs_layout.setSpacing(6)
        self.inputs_layout.setAlignment(
            QtCore.Qt.AlignmentFlag.AlignRight | QtCore.Qt.AlignmentFlag.AlignVCenter)
        self.layout().addWidget(inputs_widget, stretch=1)
        self._input_leds = {}

        self.set_connected(False)

    def set_connected(self, connected):
        color = self.connected_color if connected else self.error_color
        icon = load_icon('connected' if connected else 'disconnected', color=color)
        self.conn_icon.setPixmap(icon.pixmap(CONN_ICON_SIZE))
        self.conn_label.setText('connected' if connected else 'disconnected')
        self.conn_label.setStyleSheet(f'color: {color}; font-weight: bold;')

    def reset_inputs(self):
        """Drop known inputs so they're rebuilt from scratch on the next update_state.

        Used after a reconnect, since the input set may have changed along
        with the rest of the config.
        """
        for led in self._input_leds.values():
            self.inputs_layout.removeWidget(led)
            led.deleteLater()
        self._input_leds.clear()

    @staticmethod
    def _rich_text_width(font, html):
        """Width a QLabel would need to show `html` (rich text) in `font`."""
        label = QtWidgets.QLabel(html)
        label.setFont(font)
        return label.sizeHint().width()

    @staticmethod
    def _led_pixmap(color, size):
        pixmap = QtGui.QPixmap(size, size)
        pixmap.fill(QtCore.Qt.GlobalColor.transparent)
        painter = QtGui.QPainter(pixmap)
        painter.setRenderHint(QtGui.QPainter.RenderHint.Antialiasing)
        painter.setPen(QtGui.QColor('black'))
        painter.setBrush(QtGui.QColor(color))
        # inset by half a pixel on every side -- the pen's stroke is centered
        # on the ellipse's path and extends outward by half its width, which
        # otherwise gets clipped by the pixmap edge on the top/left (a rect
        # starting exactly at (0, 0) leaves no room there, unlike the
        # bottom/right which already have a full pixel of margin)
        painter.drawEllipse(QtCore.QRectF(0.5, 0.5, size - 1, size - 1))
        painter.end()
        return pixmap

    def update_state(self, state):
        if state is None:
            return
        self.mode_label.setText(f"mode: <b>{state.get('mode', '-')}</b>")
        inputs = state.get('inputs', {})

        # first pass: make sure every input has a LED at a fixed grid slot
        for name in inputs:
            if name not in self._input_leds:
                led = QtWidgets.QLabel()
                index = len(self._input_leds)
                self.inputs_layout.addWidget(led, index // self.LEDS_PER_ROW,
                                             index % self.LEDS_PER_ROW)
                self._input_leds[name] = led

        # once wrapped to >1 row, every LED (not just newly-added ones)
        # shrinks to LED_SIZE_MULTI_ROW so more fit without ever scrolling
        rows = -(-len(self._input_leds) // self.LEDS_PER_ROW) or 1  # ceil div
        led_size = self.LED_SIZE if rows <= 1 else self.LED_SIZE_MULTI_ROW

        # second pass: (re)apply size, color, and tooltip for every input
        for name, st in inputs.items():
            led = self._input_leds[name]
            led.setFixedSize(led_size, led_size)
            if isinstance(st, dict) and 'error' in st:
                led.setPixmap(self._led_pixmap(self.error_color, led_size))
                led.setToolTip(f'{name}: error -- {st["error"]}')
            else:
                color = self.state_colors.get(st, self.default_text_color)
                led.setPixmap(self._led_pixmap(color, led_size))
                led.setToolTip(f'{name}: {st}')

    def update_run_info(self, run_id, elapsed_s, total, rate):
        """Update the run/time/total/rate fields.

        `elapsed_s` (seconds since run start) isn't available from the
        backend yet -- pass None until a shm field for it exists; this
        already renders that as a plain placeholder.
        """
        self.run_label.setText(f'run: <b>{run_id}</b>')
        if elapsed_s is not None:
            time_text = f'time: <b>{elapsed_s}</b>s'
        else:
            time_text = 'time: <b>-</b>'
        self.time_label.setText(time_text)
        self.total_label.setText(f'total: <b>{total:,}</b> cts')
        if rate is not None:
            rate_text = f'rate: <b>{rate:,.1f}</b>/sec'
        else:
            rate_text = 'rate: <b>-</b>'
        self.rate_label.setText(rate_text)
