# Part of the Unified Mechanism for Acquisition of Measured Intensity
# (UMAMI), see README and LICENSE files for more info.

"""Live recipe/output parameter table, editable in place."""

import json

from pyqtgraph.Qt import QtCore, QtGui, QtWidgets

from .icons import icon_button


class ValueEditDialog(QtWidgets.QDialog):
    """Edit one parameter's value as pretty-printed JSON in a bigger box.

    Simple/scalar values are still edited in place via the table's own cell
    editor or checkbox; this is for values too large or nested (lists,
    dicts) to comfortably edit in one table cell.
    """

    def __init__(self, parent, key, info):
        super().__init__(parent)
        self.setWindowTitle(f'Edit {key}')
        self.resize(500, 400)

        layout = QtWidgets.QVBoxLayout(self)
        datatype = info.get('datatype', '')
        help_text = info.get('help', '')
        if datatype or help_text:
            text = f'{datatype}: {help_text}' if datatype else help_text
            label = QtWidgets.QLabel(text)
            label.setWordWrap(True)
            label.setStyleSheet('color: #555;')
            layout.addWidget(label)

        self.edit = QtWidgets.QPlainTextEdit(json.dumps(info.get('value'), indent=2))
        self.edit.setFont(QtGui.QFont('monospace'))
        layout.addWidget(self.edit)

        buttons = QtWidgets.QDialogButtonBox(
            QtWidgets.QDialogButtonBox.StandardButton.Ok |
            QtWidgets.QDialogButtonBox.StandardButton.Cancel)
        buttons.accepted.connect(self._validate_and_accept)
        buttons.rejected.connect(self.reject)
        layout.addWidget(buttons)

        self._value = None

    def _validate_and_accept(self):
        try:
            self._value = json.loads(self.edit.toPlainText())
        except json.JSONDecodeError as e:
            QtWidgets.QMessageBox.warning(self, 'Invalid JSON', str(e))
            return
        self.accept()

    def value(self):
        return self._value


class ParamsTable(QtWidgets.QTableWidget):
    """Shows current recipe/output parameters from get_params.

    Editing a cell pushes the change live via set_params.
    """

    def __init__(self, client):
        super().__init__(0, 3)
        self.client = client
        self.setHorizontalHeaderLabels(['Parameter', 'Value', ''])
        self.horizontalHeader().setSectionResizeMode(
            1, QtWidgets.QHeaderView.ResizeMode.Stretch)
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
                if isinstance(value, (list, dict)):
                    text = json.dumps(value)
                elif value is None:
                    text = ''
                else:
                    text = str(value)
                self.setItem(row, 1, QtWidgets.QTableWidgetItem(text))

            if isinstance(value, (list, dict)):
                edit_btn = icon_button('edit')
                edit_btn.setToolTip('Edit in a larger dialog')
                edit_btn.clicked.connect(
                    lambda _, k=key, i=info: self._edit_dialog(k, i))
                cell = QtWidgets.QWidget()
                cell_layout = QtWidgets.QHBoxLayout(cell)
                cell_layout.setContentsMargins(2, 0, 2, 0)
                cell_layout.addWidget(edit_btn)
                self.setCellWidget(row, 2, cell)
            else:
                self.removeCellWidget(row, 2)
        self.resizeColumnToContents(2)
        self.blockSignals(False)

    def _edit_dialog(self, key, info):
        dialog = ValueEditDialog(self, key, info)
        if dialog.exec() != QtWidgets.QDialog.DialogCode.Accepted:
            return
        self._send(key, dialog.value())
        self.refresh()

    def _on_item_changed(self, item):
        if item.column() != 1 or item.row() >= len(self._keys):
            return
        self._send(self._keys[item.row()], self._parse(item.text()))
        self.refresh()

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
        return value if isinstance(value, (list, dict)) else text

    def _send(self, key, value):
        self.client.set_params({key: value})
