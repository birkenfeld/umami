# Part of the Unified Mechanism for Acquisition of Measured Intensity
# (UMAMI), see README and LICENSE files for more info.

"""Scrolling, timestamped log widget for commands, replies, and errors."""

import time
from typing import ClassVar

from pyqtgraph.Qt import QtCore, QtGui, QtWidgets


class LogPanel(QtWidgets.QPlainTextEdit):
    """Scrolling, timestamped log of commands, replies, and errors."""

    # TODO dark mode
    COLORS: ClassVar = {'warning': QtGui.QColor('darkorange'),
                        'error': QtGui.QColor('red')}
    error_logged = QtCore.pyqtSignal()

    def __init__(self):
        super().__init__()
        self.setReadOnly(True)
        self.setMaximumBlockCount(2000)
        self.setFont(QtGui.QFont('monospace'))

    def _append(self, level, text):
        scrollbar = self.verticalScrollBar()
        at_bottom = scrollbar.value() >= scrollbar.maximum()
        cursor = self.textCursor()
        cursor.movePosition(QtGui.QTextCursor.MoveOperation.End)
        fmt = QtGui.QTextCharFormat()
        if level in self.COLORS:
            fmt.setForeground(self.COLORS[level])
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
