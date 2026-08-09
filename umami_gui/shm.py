# Part of the Unified Mechanism for Acquisition of Measured Intensity
# (UMAMI), see README and LICENSE files for more info.

"""Read-only access to a UMAMI shared-memory histogram segment.

For the shared-memory segment layout, see Rust's `src/shm.rs`.
"""

import numpy as np

from umami_client import Shm


class ShmHistogram(Shm):
    """Read-only view of a UMAMI shared-memory histogram segment.

    We just add some convenience methods to the base Shm class for reading the
    histogram data in numpy format, and all counters.
    """

    def read_counters(self):
        """Read the events/neutrons/lifetime/tzero/monitors counters.

        Returns `(total_events, total_neutrons, lifetime_ns, tzero_count,
        monitor_counts)`, all accumulated since the last Clear.
        """
        # TODO: make this a dataclass or namedtuple for clarity
        return (self.total_events, self.total_neutrons, self.lifetime_ns,
                self.tzero_count, self.monitor_counts)

    def read_plane(self, t=0):
        offset = t * self.nx * self.ny * 4
        return np.frombuffer(self, '<u4', self.nx * self.ny, offset) \
                 .reshape((self.ny, self.nx))

    def read_time_projection(self, n=None):
        n = self.nt if n is None else min(n, self.nt)
        return np.frombuffer(self, '<u4', self.nx * self.ny * n) \
                 .reshape((n, self.ny, self.nx)) \
                 .sum(axis=(1, 2))
