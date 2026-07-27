# Part of the Unified Mechanism for Acquisition of Measured Intensity
# (UMAMI), see README and LICENSE files for more info.

"""Connection/mode/per-input state indicator panel."""

from typing import ClassVar

from pyqtgraph.Qt import QtCore, QtWidgets

from .icons import is_dark_mode, load_icon

CONN_ICON_SIZE = QtCore.QSize(16, 16)


class StatusPanel(QtWidgets.QFrame):
    """Connection indicator, current mode, and per-input state at a glance."""

    # plain named colors read fine on a light background but are too dark/
    # muddy (or, for the default-text fallback, nearly invisible) on a dark
    # one -- pick brighter/lighter equivalents per theme instead
    STATE_COLORS: ClassVar = [
        {'idle': 'gray', 'running': 'green', 'ended': 'blue'},
        {'idle': '#aaaaaa', 'running': '#66bb6a', 'ended': '#64b5f6'},
    ]
    ERROR_COLOR = ('red', '#ef5350')
    DEFAULT_TEXT = ('#333333', '#cccccc')
    INPUT_ROWS = 3
    INPUT_FONT_SIZE = 8

    def __init__(self):
        super().__init__()
        dark = is_dark_mode()
        self.state_colors = self.STATE_COLORS[dark]
        self.error_color = self.ERROR_COLOR[dark]
        self.default_text_color = self.DEFAULT_TEXT[dark]
        self.setLayout(QtWidgets.QHBoxLayout())
        self.layout().setContentsMargins(8, 2, 8, 2)
        self.setSizePolicy(QtWidgets.QSizePolicy.Policy.Preferred,
                            QtWidgets.QSizePolicy.Policy.Fixed)

        self.conn_icon = QtWidgets.QLabel()
        self.layout().addWidget(self.conn_icon)
        self.conn_label = QtWidgets.QLabel()
        self.layout().addWidget(self.conn_label)
        self.layout().addSpacing(20)

        self.mode_label = QtWidgets.QLabel('mode: -')
        self.layout().addWidget(self.mode_label)
        self.layout().addSpacing(20)

        # a config can have many inputs; lay them out in a compact grid
        inputs_widget = QtWidgets.QWidget()
        self.inputs_layout = QtWidgets.QGridLayout(inputs_widget)
        self.inputs_layout.setContentsMargins(0, 0, 0, 0)
        self.inputs_layout.setHorizontalSpacing(4)
        self.inputs_layout.setVerticalSpacing(0)
        inputs_scroll = QtWidgets.QScrollArea()
        inputs_scroll.setWidget(inputs_widget)
        inputs_scroll.setWidgetResizable(True)
        inputs_scroll.setHorizontalScrollBarPolicy(QtCore.Qt.ScrollBarPolicy.ScrollBarAsNeeded)
        inputs_scroll.setVerticalScrollBarPolicy(QtCore.Qt.ScrollBarPolicy.ScrollBarAlwaysOff)
        inputs_scroll.setFixedHeight(14 * self.INPUT_ROWS + 8)
        inputs_scroll.setFrameShape(QtWidgets.QFrame.Shape.NoFrame)
        inputs_scroll.setStyleSheet('background: transparent;')
        inputs_scroll.viewport().setStyleSheet('background: transparent;')
        inputs_widget.setAutoFillBackground(False)
        self.layout().addWidget(inputs_scroll, stretch=1)
        self._input_labels = {}

        self.set_connected(False)

    def set_connected(self, connected):
        color = self.state_colors['running'] if connected else self.error_color
        icon = load_icon('connected' if connected else 'disconnected', color=color)
        self.conn_icon.setPixmap(icon.pixmap(CONN_ICON_SIZE))
        self.conn_label.setText('connected' if connected else 'disconnected')
        self.conn_label.setStyleSheet(f'color: {color}; font-weight: bold;')

    def reset_inputs(self):
        """Drop known inputs so they're rebuilt from scratch on the next update_state.

        Used after a reconnect, since the input set may have changed along
        with the rest of the config.
        """
        for label in self._input_labels.values():
            self.inputs_layout.removeWidget(label)
            label.deleteLater()
        self._input_labels.clear()

    def update_state(self, state):
        if state is None:
            return
        self.mode_label.setText(f"mode: {state.get('mode', '-')}")
        for name, st in state.get('inputs', {}).items():
            if name not in self._input_labels:
                label = QtWidgets.QLabel()
                font = label.font()
                font.setPointSize(self.INPUT_FONT_SIZE)
                label.setFont(font)
                index = len(self._input_labels)
                self.inputs_layout.addWidget(label, index % self.INPUT_ROWS,
                                             index // self.INPUT_ROWS)
                self._input_labels[name] = label
            label = self._input_labels[name]
            if isinstance(st, dict) and 'error' in st:
                label.setText(f'{name}: error')
                label.setStyleSheet(f'color: {self.error_color};')
                label.setToolTip(st['error'])
            else:
                label.setText(f'{name}: {st}')
                color = self.state_colors.get(st, self.default_text_color)
                label.setStyleSheet(f'color: {color};')
                label.setToolTip('')
