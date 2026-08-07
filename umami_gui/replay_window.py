# Part of the Unified Mechanism for Acquisition of Measured Intensity
# (UMAMI), see README and LICENSE files for more info.

"""Replay-directory picker: point file-backed inputs at a captured run.

Mirrors the layout raw-dump output writes (`<raw_dir>/<run_id>/<input_name>`),
so a "run" here is one of those `<run_id>` subdirectories.
"""

from pathlib import Path

from pyqtgraph.Qt.QtCore import QSettings, pyqtSignal
from pyqtgraph.Qt.QtGui import QPalette
from pyqtgraph.Qt.QtWidgets import (
    QApplication,
    QFileDialog,
    QHBoxLayout,
    QLineEdit,
    QListWidget,
    QListWidgetItem,
    QMessageBox,
    QPushButton,
    QVBoxLayout,
    QWidget,
)

from .icons import ICON_SIZE, icon_button, load_icon


class ReplayWindow(QWidget):
    """Pick a run subdirectory and load its files into matching inputs."""

    applied = pyqtSignal()

    def __init__(self, client):
        super().__init__()
        self.client = client
        self.setWindowTitle('UMAMI replay')
        self.resize(400, 500)
        self.settings = QSettings()

        self.root_path = QLineEdit()
        self.root_path.setPlaceholderText('/path/to/raw/dump/root')
        self.root_path.setText(self.settings.value('replay_root', ''))
        self.root_path.editingFinished.connect(self._refresh_list)
        browse_btn = QPushButton('...')
        browse_btn.setMaximumWidth(30)
        browse_btn.clicked.connect(self._browse_root)

        root_row = QHBoxLayout()
        root_row.addWidget(self.root_path)
        root_row.addWidget(browse_btn)

        self.run_list = QListWidget()
        self.run_list.setIconSize(ICON_SIZE)
        self.run_list.itemDoubleClicked.connect(lambda: self._load(start=True))

        load_btn = icon_button('file_open', 'Prepare replay')
        load_btn.setToolTip('Set replay_file on every input with a matching '
                             'file in the selected run directory.')
        load_btn.clicked.connect(lambda: self._load(start=False))
        load_start_btn = icon_button('start', 'Clear and replay', tint=False)
        load_start_btn.setStyleSheet(
            'background-color: rgb(140, 205, 140); color: black;')
        load_start_btn.clicked.connect(lambda: self._load(start=True))

        close_btn = QPushButton('Close')
        close_btn.clicked.connect(self.close)

        btn_row = QHBoxLayout()
        btn_row.addWidget(close_btn)
        btn_row.addStretch()
        btn_row.addWidget(load_btn)
        btn_row.addWidget(load_start_btn)

        layout = QVBoxLayout(self)
        layout.addLayout(root_row)
        layout.addWidget(self.run_list, 1)
        layout.addLayout(btn_row)

    def showEvent(self, event):  # noqa: N802
        super().showEvent(event)
        self._refresh_list()

    def closeEvent(self, event):  # noqa: N802
        self.settings.setValue('replay_root', self.root_path.text())
        super().closeEvent(event)

    def _browse_root(self):
        path = QFileDialog.getExistingDirectory(
            self, 'Replay Root Directory', self.root_path.text())
        if path:
            self.root_path.setText(path)
            self._refresh_list()

    def _refresh_list(self):
        self.run_list.clear()
        root = Path(self.root_path.text())
        if not root.is_dir():
            return
        input_names = self._current_input_names()
        color = QApplication.palette().color(QPalette.ColorRole.Text)
        for entry in sorted(p.name for p in root.iterdir() if p.is_dir()):
            matches = bool(self._matching_files(root / entry, input_names))
            icon = load_icon('folder_check' if matches else 'folder', color=color)
            self.run_list.addItem(QListWidgetItem(icon, entry))

    def _current_input_names(self):
        state = self.client.get_state()
        return list(state.get('inputs', {})) if state else []

    @staticmethod
    def _matching_files(run_dir, input_names):
        """`{input_name: path}` for every input with a file in `run_dir`."""
        return {name: run_dir / name for name in input_names
                if (run_dir / name).is_file()}

    def _load(self, start):
        items = self.run_list.selectedItems()
        if not items:
            return
        run_name = items[0].text()
        run_dir = Path(self.root_path.text()) / run_name

        matches = self._matching_files(run_dir, self._current_input_names())
        if not matches:
            QMessageBox.warning(
                self, 'Nothing to load',
                f'No files in {run_dir} match any currently configured input.')
            return
        mapping = {name: str(path) for name, path in matches.items()}

        self.client.set_replay_files(mapping)
        self.applied.emit()
        if start:
            self.client.clear()
            self.client.start(run_name)
