# Part of the Unified Mechanism for Acquisition of Measured Intensity
# (UMAMI), see README and LICENSE files for more info.

"""Read-only access to a UMAMI shared-memory histogram segment.

For the shared-memory segment layout, see Rust's `src/shm.rs`.
"""

from dataclasses import dataclass

import numpy as np

from umami_client import Shm


@dataclass
class Counters:
    """Events/neutrons/lifetime/tzero/monitors counters, since the last Clear."""

    total_events: int
    total_neutrons: int
    lifetime_ns: int
    tzero_count: int
    monitor_counts: list


class ShmHistogram(Shm):
    """Read-only view of a UMAMI shared-memory histogram segment.

    We just add some convenience methods to the base Shm class for reading the
    histogram data in numpy format, and all counters.
    """

    def read_counters(self):
        """Read the events/neutrons/lifetime/tzero/monitors counters."""
        return Counters(self.total_events, self.total_neutrons, self.lifetime_ns,
                         self.tzero_count, self.monitor_counts)

    def read_plane(self, t=0):
        return np.asarray(self)[t]

    def read_time_projection(self, n=None):
        n = self.nt if n is None else min(n, self.nt)
        return np.asarray(self)[:n].sum(axis=(1, 2))
