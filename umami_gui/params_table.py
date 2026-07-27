# Part of the Unified Mechanism for Acquisition of Measured Intensity
# (UMAMI), see README and LICENSE files for more info.

"""Live recipe/output parameter table, editable in place."""

import json

from pyqtgraph.Qt import QtCore, QtWidgets


class ParamsTable(QtWidgets.QTableWidget):
    """Shows current recipe/output parameters from get_params.

    Editing a cell pushes the change live via set_params.
    """

    def __init__(self, client):
        super().__init__(0, 2)
        self.client = client
        self.setHorizontalHeaderLabels(['Parameter', 'Value'])
        self.horizontalHeader().setStretchLastSection(True)
        self.itemChanged.connect(self._on_item_changed)
        self._keys = []
        self.params = None

    def refresh(self):
        params = self.client.get_params()
        if params is None:
            return
        self.params = params
        self.blockSignals(True)
        self.setRowCount(len(params))
        self._keys = []
        for row, (key, info) in enumerate(sorted(params.items())):
            name_item = QtWidgets.QTableWidgetItem(key)
            name_item.setFlags(name_item.flags() & ~QtCore.Qt.ItemFlag.ItemIsEditable)
            name_item.setToolTip(f"{info.get('datatype', '')}: {info.get('help', '')}")
            self.setItem(row, 0, name_item)
            self._keys.append(key)

            value = info.get('value')
            if isinstance(value, bool):
                checkbox = QtWidgets.QCheckBox()
                checkbox.setChecked(value)
                checkbox.toggled.connect(lambda checked, k=key: self._send(k, checked))
                cell = QtWidgets.QWidget()
                cell_layout = QtWidgets.QHBoxLayout(cell)
                cell_layout.setContentsMargins(6, 0, 0, 0)
                cell_layout.addWidget(checkbox)
                self.setCellWidget(row, 1, cell)
            else:
                if isinstance(value, list):
                    text = json.dumps(value)
                elif value is None:
                    text = ''
                else:
                    text = str(value)
                self.setItem(row, 1, QtWidgets.QTableWidgetItem(text))
        self.blockSignals(False)

    def _on_item_changed(self, item):
        if item.column() != 1 or item.row() >= len(self._keys):
            return
        self._send(self._keys[item.row()], self._parse(item.text()))

    @staticmethod
    def _parse(text):
        if text == '':
            return None
        for conv in (int, float):
            try:
                return conv(text)
            except ValueError:
                pass
        try:
            value = json.loads(text)
        except (json.JSONDecodeError, ValueError):
            return text
        return value if isinstance(value, list) else text

    def _send(self, key, value):
        self.client.set_params({key: value})
