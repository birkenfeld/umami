# Part of the Unified Mechanism for Acquisition of Measured Intensity
# (UMAMI), see README and LICENSE files for more info.

"""Custom pyqtgraph plot items shared across umami-gui windows."""

import pyqtgraph as pg
from pyqtgraph.Qt.QtCore import QPointF, QRectF, Qt


class RotatedAxisItem(pg.AxisItem):
    """An AxisItem that draws its tick labels at an angle.

    For long labels (e.g. time-bin ranges) that would otherwise overlap.
    """

    def __init__(self, *args, angle=-40, **kwargs):
        super().__init__(*args, **kwargs)
        self.tick_angle = angle

    def drawPicture(self, p, axisSpec, tickSpecs, textSpecs):  # noqa: N802,N803
        p.setRenderHint(p.RenderHint.Antialiasing, False)
        p.setRenderHint(p.RenderHint.TextAntialiasing, True)

        pen, p1, p2 = axisSpec
        p.setPen(pen)
        p.drawLine(p1, p2)
        for pen, p1, p2 in tickSpecs:
            p.setPen(pen)
            p.drawLine(p1, p2)

        if self.style['tickFont'] is not None:
            p.setFont(self.style['tickFont'])
        p.setPen(self.textPen())
        for rect, flags, text in textSpecs:
            # pyqtgraph centers the (unrotated) label under its tick, so the
            # tick's true x-position is rect's horizontal center, not its
            # right edge; anchor there so the rotated label's top-right
            # corner lines up with the tick and swings down-left below it
            p.save()
            p.translate(QPointF(rect.center().x(), rect.top()))
            p.rotate(self.tick_angle)
            local_rect = QRectF(-rect.width(), 0, rect.width(), rect.height())
            p.drawText(local_rect, int(flags), text)
            p.restore()


class ZoomViewBox(pg.ViewBox):
    """A ViewBox with left-button drag-to-zoom and right-button drag-to-pan.

    The reverse of pyqtgraph's own default (left-button pans, right-button
    drag-to-scale). Right-click still opens the normal context menu (not
    overridden); a middle-click resets the view to fit all data instead.
    """

    def mouseDragEvent(self, ev, axis=None):  # noqa: N802
        if ev.button() == Qt.MouseButton.RightButton:
            ev.accept()
            p1 = self.mapToView(ev.lastPos())
            p2 = self.mapToView(ev.pos())
            self.translateBy(x=p1.x() - p2.x(), y=p1.y() - p2.y())
            return
        if ev.button() != Qt.MouseButton.LeftButton:
            super().mouseDragEvent(ev, axis=axis)
            return
        ev.accept()
        if ev.isFinish():
            self.rbScaleBox.hide()
            ax = QRectF(pg.Point(ev.buttonDownPos(ev.button())),
                        pg.Point(ev.pos()))
            ax = self.childGroup.mapRectFromParent(ax)
            self.showAxRect(ax)
            self.axHistoryPointer += 1
            self.axHistory = [*self.axHistory[:self.axHistoryPointer], ax]
        else:
            self.updateScaleBox(ev.buttonDownPos(), ev.pos())

    def mouseClickEvent(self, ev):  # noqa: N802
        if ev.button() != Qt.MouseButton.MiddleButton:
            super().mouseClickEvent(ev)
            return
        ev.accept()
        self.autoRange()
