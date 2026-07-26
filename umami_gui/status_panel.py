"""Connection/mode/per-input state indicator panel."""

from typing import ClassVar

from pyqtgraph.Qt import QtCore, QtWidgets


class StatusPanel(QtWidgets.QFrame):
    """Connection indicator, current mode, and per-input state at a glance."""

    STATE_COLORS: ClassVar = {'idle': 'gray', 'running': 'green', 'ended': 'blue'}
    ERROR_COLOR = 'red'
    INPUT_ROWS = 3
    INPUT_FONT_SIZE = 8

    def __init__(self):
        super().__init__()
        self.setLayout(QtWidgets.QHBoxLayout())
        self.layout().setContentsMargins(8, 2, 8, 2)
        self.setSizePolicy(QtWidgets.QSizePolicy.Policy.Preferred,
                            QtWidgets.QSizePolicy.Policy.Fixed)

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
        if connected:
            self.conn_label.setText('● connected')
            self.conn_label.setStyleSheet(f'color: {self.STATE_COLORS["running"]}; '
                                          'font-weight: bold;')
        else:
            self.conn_label.setText('● disconnected')
            self.conn_label.setStyleSheet(f'color: {self.ERROR_COLOR}; font-weight: bold;')

    def reset_inputs(self):
        """Drop known inputs so they're rebuilt from scratch on the next
        update_state -- used after a reconnect, since the input set may
        have changed along with the rest of the config."""
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
                label.setStyleSheet(f'color: {self.ERROR_COLOR};')
                label.setToolTip(st['error'])
            else:
                label.setText(f'{name}: {st}')
                label.setStyleSheet(f'color: {self.STATE_COLORS.get(st, "#333")};')
                label.setToolTip('')
