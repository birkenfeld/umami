# Part of the Unified Mechanism for Acquisition of Measured Intensity
# (UMAMI), see README and LICENSE files for more info.

"""Live recipe/output parameter tree, editable in place."""

import json

from pyqtgraph.Qt.QtCore import Qt, QTimer
from pyqtgraph.Qt.QtGui import QFont, QPalette
from pyqtgraph.Qt.QtWidgets import (
    QCheckBox,
    QDialog,
    QDialogButtonBox,
    QHBoxLayout,
    QHeaderView,
    QLabel,
    QLineEdit,
    QMessageBox,
    QPlainTextEdit,
    QStyledItemDelegate,
    QToolButton,
    QTreeWidget,
    QTreeWidgetItem,
    QVBoxLayout,
    QWidget,
)

from .icons import ICON_SIZE, load_icon


class ValueEditDialog(QDialog):
    """Edit one parameter's value as pretty-printed JSON in a bigger box.

    Simple/scalar values are still edited in place via the tree's own item
    editor or checkbox; this is for values too large or nested (lists,
    dicts) to comfortably edit in one cell.
    """

    def __init__(self, parent, key, info):
        super().__init__(parent)
        readonly = info.get('readonly', False)
        self.setWindowTitle(f'View {key}' if readonly else f'Edit {key}')
        self.resize(500, 400)

        layout = QVBoxLayout(self)
        datatype = info.get('datatype', '')
        help_text = info.get('help', '')
        if datatype or help_text:
            text = f'{datatype}: {help_text}' if datatype else help_text
            label = QLabel(text)
            label.setWordWrap(True)
            label.setStyleSheet('color: #555;')
            layout.addWidget(label)

        self.edit = QPlainTextEdit(json.dumps(info.get('value'), indent=2))
        self.edit.setFont(QFont('monospace'))
        self.edit.setReadOnly(readonly)
        layout.addWidget(self.edit)

        if readonly:
            buttons = QDialogButtonBox(QDialogButtonBox.StandardButton.Ok)
            buttons.accepted.connect(self.reject)
        else:
            buttons = QDialogButtonBox(QDialogButtonBox.StandardButton.Ok |
                                       QDialogButtonBox.StandardButton.Cancel)
            buttons.accepted.connect(self._validate_and_accept)
            buttons.rejected.connect(self.reject)
        layout.addWidget(buttons)

        self._value = None

    def _validate_and_accept(self):
        try:
            self._value = json.loads(self.edit.toPlainText())
        except json.JSONDecodeError as e:
            QMessageBox.warning(self, 'Invalid JSON', str(e))
            return
        self.accept()

    def value(self):
        return self._value


class _ValueColumnDelegate(QStyledItemDelegate):
    """Only column 1 (Value) is ever editable via the tree's own editor.

    A `QTreeWidgetItem`'s `ItemIsEditable` flag applies to the whole row, so
    without this a param leaf's name in column 0 would open an editor too.

    A writable scalar's cell (1) normally shows a persistent label+button
    widget rather than the item's own text -- setting both at once paints
    one on top of the other. So the item's real text is never populated at
    rest, which also means the default `setEditorData` would open the
    editor pre-filled with nothing to edit; look the current value up from
    the tree's own `params` (keyed by the item's stored param key) instead.
    """

    def createEditor(self, parent, option, index):  # noqa: N802
        if index.column() != 1:
            return None
        return super().createEditor(parent, option, index)

    def setEditorData(self, editor, index):  # noqa: N802
        if index.column() == 1 and isinstance(editor, QLineEdit):
            tree = self.parent()
            item = tree.itemFromIndex(index)
            key = item.data(0, Qt.ItemDataRole.UserRole)
            info = (tree.params or {}).get(key) if key is not None else None
            if info is not None:
                value = info.get('value')
                editor.setText('' if value is None else str(value))
                return
        super().setEditorData(editor, index)


class ParamsTree(QTreeWidget):
    """Shows current recipe/output parameters from get_params, as a tree.

    Grouped by owning module. Editing a param pushes the change live via
    set_params.
    """

    def __init__(self, client):
        super().__init__()
        self.client = client
        self.setColumnCount(2)
        self.setHeaderLabels(['Parameter', 'Value'])
        header = self.header()
        header.setSectionResizeMode(0, QHeaderView.ResizeMode.ResizeToContents)
        header.setSectionResizeMode(1, QHeaderView.ResizeMode.Stretch)
        delegate = _ValueColumnDelegate(self)
        self.setItemDelegate(delegate)
        delegate.closeEditor.connect(self._on_edit_closed)
        self.itemChanged.connect(self._on_item_changed)
        self.params = None
        self._editing_item = None
        self._edit_committed = False

    def _make_tool_button(self, name, tooltip):
        # a small flat icon button, sized to actually fit in a tree row --
        # icon_button()'s QPushButton reserves too much horizontal padding
        color = self.palette().color(QPalette.ColorRole.ButtonText)
        btn = QToolButton()
        btn.setIcon(load_icon(name, color=color))
        btn.setIconSize(ICON_SIZE)
        btn.setAutoRaise(True)
        btn.setToolTip(tooltip)
        return btn

    def _build_button_cell(self, item, text, icon, tooltip, on_click):
        """Value cell showing `text` plus a small button (edit/view/...).

        Used both for list/dict values (button opens `ValueEditDialog`) and
        plain scalars (button starts in-place cell editing). The item's own
        text (column 1) is deliberately left unset while this widget is in
        place -- setting both paints the item's native text underneath the
        widget's own label, doubling up visibly.
        """
        label = QLabel(text)
        button = self._make_tool_button(icon, tooltip)
        button.clicked.connect(on_click)
        cell = QWidget()
        cell_layout = QHBoxLayout(cell)
        cell_layout.setContentsMargins(4, 0, 2, 0)
        cell_layout.addWidget(label, 1)
        cell_layout.addWidget(button)
        self.setItemWidget(item, 1, cell)
        return label

    def _start_inline_edit(self, item):
        # a persistent cell widget hides the tree's own editor underneath
        # it, so it has to be cleared first -- _on_edit_closed() restores it
        # once editing finishes, whether committed or cancelled
        self._editing_item = item
        self._edit_committed = False
        self.removeItemWidget(item, 1)
        self.editItem(item, 1)

    def _on_edit_closed(self, editor, hint):  # noqa: ARG002
        item, committed = self._editing_item, self._edit_committed
        self._editing_item = None
        if item is None or committed or self.params is None:
            # a committed edit is handled by the refresh() it triggers via
            # _on_item_changed, which replaces this item outright
            return
        key = item.data(0, Qt.ItemDataRole.UserRole)
        info = self.params.get(key) if key is not None else None
        if info is None:
            return
        value = info.get('value')
        text = '' if value is None else str(value)
        self._build_button_cell(item, text, 'edit', 'Edit',
                                 lambda _, i=item: self._start_inline_edit(i))

    def _build_param_item(self, parent, key, name, info):
        item = QTreeWidgetItem(parent, [name])
        item.setData(0, Qt.ItemDataRole.UserRole, key)
        readonly = info.get('readonly', False)
        tooltip = f"{info.get('datatype', '')}: {info.get('help', '')}"
        item.setToolTip(0, f'{tooltip} (read-only)' if readonly else tooltip)

        value = info.get('value')
        if isinstance(value, bool):
            checkbox = QCheckBox()
            checkbox.setChecked(value)
            checkbox.setEnabled(not readonly)
            checkbox.toggled.connect(lambda checked, k=key: self._send(k, checked))
            cell = QWidget()
            cell_layout = QHBoxLayout(cell)
            cell_layout.setContentsMargins(6, 0, 0, 0)
            cell_layout.addWidget(checkbox)
            self.setItemWidget(item, 1, cell)
        elif isinstance(value, (list, dict)):
            # editing happens exclusively via the dialog below, never via
            # the tree's own in-place editor
            item.setFlags(item.flags() & ~Qt.ItemFlag.ItemIsEditable)
            text = json.dumps(value)
            label = self._build_button_cell(
                item, text, 'view' if readonly else 'edit',
                'View in a larger dialog' if readonly else 'Edit in a larger dialog',
                lambda _, k=key, i=info: self._edit_dialog(k, i))
            label.setToolTip(json.dumps(value, indent=2))
        else:
            text = '' if value is None else str(value)
            if readonly:
                # no cell widget here, so the item's own text is the only
                # thing rendering it -- safe to set directly
                item.setText(1, text)
                item.setFlags(item.flags() & ~Qt.ItemFlag.ItemIsEditable)
            else:
                item.setFlags(item.flags() | Qt.ItemFlag.ItemIsEditable)
                self._build_button_cell(
                    item, text, 'edit', 'Edit',
                    lambda _, i=item: self._start_inline_edit(i))

    def refresh(self):
        params = self.client.get_params(full=True)
        if params is None:
            return
        self.params = params

        expanded = {
            self.topLevelItem(i).text(0): self.topLevelItem(i).isExpanded()
            for i in range(self.topLevelItemCount())
        }
        scroll_pos = self.verticalScrollBar().value()

        # _info entries are discovery metadata (see mesy_config.py /
        # aux_histo.py), not user-editable parameters -- keep them out of
        # the displayed leaves, but use them to annotate their module node.
        by_module = {}
        for key, info in params.items():
            module, sep, name = key.partition('.')
            if not sep or name == '_info':
                continue
            by_module.setdefault(module, {})[name] = (key, info)

        placeholder = self.palette().color(QPalette.ColorRole.PlaceholderText)
        self.blockSignals(True)
        self.clear()
        for module in sorted(by_module):
            mod_item = QTreeWidgetItem(self, [module])
            mod_item.setFlags(mod_item.flags() & ~Qt.ItemFlag.ItemIsEditable)
            info = params.get(f'{module}._info')
            if info:
                mod_item.setText(1, f"{info['kind']} ({info['type']})")
                mod_item.setForeground(1, placeholder)
            mod_item.setExpanded(expanded.get(module, True))

            for name, (key, pinfo) in sorted(by_module[module].items()):
                self._build_param_item(mod_item, key, name, pinfo)
        self.blockSignals(False)
        self.verticalScrollBar().setValue(scroll_pos)

    def _edit_dialog(self, key, info):
        dialog = ValueEditDialog(self, key, info)
        if dialog.exec() != QDialog.DialogCode.Accepted:
            return
        self._send(key, dialog.value())
        QTimer.singleShot(0, self.refresh)

    def _on_item_changed(self, item, column):
        if column != 1:
            return
        key = item.data(0, Qt.ItemDataRole.UserRole)
        if key is None:
            return
        self._send(key, self._parse(item.text(1)))
        QTimer.singleShot(0, self.refresh)

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
