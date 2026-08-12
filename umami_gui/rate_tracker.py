# Part of the Unified Mechanism for Acquisition of Measured Intensity
# (UMAMI), see README and LICENSE files for more info.

"""Sliding-window rate-of-change tracking for live counter displays."""


class RateTracker:
    """Tracks one or more monotonically-increasing counters over a window.

    Feed periodic snapshots via `update()`; it retains up to `window`
    samples and reports each value's rate of change against the oldest
    retained sample. A rate is `None` where there aren't enough samples yet,
    or the value decreased since the oldest sample (e.g. a Clear happened
    mid-window).
    """

    def __init__(self, window):
        self.window = window
        self._samples = []

    def clear(self):
        self._samples.clear()

    def update(self, now, values):
        """Record a snapshot of `values` at time `now`, return their rates."""
        self._samples.append((now, values))
        if len(self._samples) > self.window:
            self._samples.pop(0)
        rates = [None] * len(values)
        if len(self._samples) > 1:
            old_time, old_values = self._samples[0]
            for i, (cur, old) in enumerate(zip(values, old_values)):
                if cur >= old:
                    rates[i] = (cur - old) / (now - old_time)
        return rates

    def update_one(self, now, value):
        """Track a single value; see `update()`."""
        return self.update(now, (value,))[0]
