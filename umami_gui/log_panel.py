# Part of the Unified Mechanism for Acquisition of Measured Intensity
# (UMAMI), see README and LICENSE files for more info.

"""Scrolling, timestamped log widget for commands, replies, and errors."""

import time
from typing import ClassVar

from pyqtgraph.Qt.QtCore import pyqtSignal
from pyqtgraph.Qt.QtGui import QColor, QFont, QTextCharFormat, QTextCursor
from pyqtgraph.Qt.QtWidgets import QPlainTextEdit

from .icons import is_dark_mode


class LogPanel(QPlainTextEdit):
    """Scrolling, timestamped log of commands, replies, and errors."""

    COLORS: ClassVar = [
        {'warning': QColor('darkorange'), 'error': QColor('red')},
        {'warning': QColor('darkorange'), 'error': QColor('#ef5350')},
    ]
    error_logged = pyqtSignal()

    def __init__(self):
        super().__init__()
        self.colors = self.COLORS[is_dark_mode()]
        self.setReadOnly(True)
        self.setMaximumBlockCount(2000)
        self.setFont(QFont('monospace'))

    def _append(self, level, text):
        scrollbar = self.verticalScrollBar()
        at_bottom = scrollbar.value() >= scrollbar.maximum()
        cursor = self.textCursor()
        cursor.movePosition(QTextCursor.MoveOperation.End)
        fmt = QTextCharFormat()
        if level in self.colors:
            fmt.setForeground(self.colors[level])
        cursor.setCharFormat(fmt)
        cursor.insertText(f'[{time.strftime("%H:%M:%S")}] {level.upper():7} {text}\n')
        self.setTextCursor(cursor)
        if at_bottom:
            scrollbar.setValue(scrollbar.maximum())

    def info(self, text):
        self._append('info', text)

    def warning(self, text):
        self._append('warning', text)

    def error(self, text):
        self._append('error', text)
        self.error_logged.emit()
