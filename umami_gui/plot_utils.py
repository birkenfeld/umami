# Part of the Unified Mechanism for Acquisition of Measured Intensity
# (UMAMI), see README and LICENSE files for more info.

"""Small helpers shared by UMAMI's various step-histogram plots.

Used by `projection_windows.py` and `aux_histo.py` -- consistent curve
styling and a cursor-following hover tooltip, factored out since both were
previously copy-pasted at each plot with only the bin lookup/text differing.
"""

from pyqtgraph.Qt import QtGui, QtWidgets


def step_histogram_curve(plot_item_or_widget):
    """Add a step-mode histogram curve, styled consistently across UMAMI's plots."""
    return plot_item_or_widget.plot(
        stepMode='center', fillLevel=0, brush=(0, 0, 255, 80))


def connect_hover_tooltip(plot_widget, text_for_x):
    """Wire a `QToolTip`-based hover readout onto `plot_widget`.

    `text_for_x(view_x)` is called with the mouse's x position in the
    plot's view (data) coordinates on every move; return the tooltip text
    to show, or `None` to hide it (e.g. the mouse is out of data range).
    """
    def on_mouse_moved(scene_pos):
        plot_item = plot_widget.getPlotItem()
        if not plot_item.sceneBoundingRect().contains(scene_pos):
            QtWidgets.QToolTip.hideText()
            return
        view_pos = plot_item.vb.mapSceneToView(scene_pos)
        text = text_for_x(view_pos.x())
        if text is None:
            QtWidgets.QToolTip.hideText()
            return
        QtWidgets.QToolTip.showText(QtGui.QCursor.pos(), text)
    plot_widget.scene().sigMouseMoved.connect(on_mouse_moved)
