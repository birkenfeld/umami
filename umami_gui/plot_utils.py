# Part of the Unified Mechanism for Acquisition of Measured Intensity
# (UMAMI), see README and LICENSE files for more info.

"""Small helpers shared by UMAMI's plots.

Used by `projection_windows.py`, `aux_histo.py` and `aux_plot.py` --
consistent curve styling, a cursor-following hover tooltip, and the
log-scale/colormap/levels convention for 2-D images -- factored out since
these were previously copy-pasted at each plot with only minor details
differing.
"""

import numpy as np
from pyqtgraph.Qt import QtGui, QtWidgets

# display name -> pyqtgraph colormap name; pyqtgraph has no plain 'grey'/'gray'
# map, so this points at CET-L1, its linear (grayscale) perceptual map
COLORMAPS = {
    'viridis': 'viridis',
    'inferno': 'inferno',
    'plasma': 'plasma',
    'magma': 'magma',
    'turbo': 'turbo',
    'grey': 'CET-L1',
}

# avoids log10(0) = -inf; also the implicit "zero" level for manual limits
LOG_OFFSET = 0.1


def set_image_data(img, counts, *, log, auto_levels, level_min, level_max):
    """Apply `counts` to an `ImageItem`, handling log scale and z-limits.

    `level_min`/`level_max` are always in raw count units -- when `log` is
    set, they're transformed to log space at apply time, so the caller's
    UI never has to show/enter log values directly.
    """
    display = counts.astype(float)
    if log:
        display = np.log10(display + LOG_OFFSET)
    if auto_levels:
        # pyqtgraph's autoLevels defaults to subsampling (levelSamples=65536)
        # for speed, which can miss a small/sparse peak entirely
        img.setImage(display, autoLevels=True, levelSamples=display.size)
    else:
        lo, hi = level_min, level_max
        if log:
            lo, hi = np.log10(lo + LOG_OFFSET), np.log10(hi + LOG_OFFSET)
        img.setImage(display, autoLevels=False, levels=(lo, hi))


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
