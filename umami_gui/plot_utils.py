# Part of the Unified Mechanism for Acquisition of Measured Intensity
# (UMAMI), see README and LICENSE files for more info.

"""Small helpers shared by UMAMI's plots."""

import numpy as np

# display name -> pyqtgraph colormap name
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

    `level_min`/`level_max` are in raw count units -- when `log` is set,
    they're transformed to log space in here, so the caller's UI doesn't have
    to show/enter log values directly.
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
    """Add a step-mode histogram curve."""
    return plot_item_or_widget.plot(
        stepMode='center', fillLevel=0, brush=(0, 0, 255, 80))
