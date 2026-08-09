# Part of the Unified Mechanism for Acquisition of Measured Intensity
# (UMAMI), see README and LICENSE files for more info.

"""Read-only access to a UMAMI shared-memory histogram segment.

Shared-memory segment layout:

    Offset  Size  Field
    ------  ----  -----
       0       8  magic (ASCII "UMAMI" + 2-digit version + a space, e.g. "UMAMI01 ")
       8     128  run_id (ASCII, NUL-padded)
     136       4  global_state (u32, bit 0: initialized, bit 1: run active)
     140       2  nx (u16)
     142       2  ny (u16)
     144       2  nt (u16)
     146       2  ni (u16, unused -- not implemented on the UMAMI side)
     148       4  run_start (u32, Unix timestamp of the last StartOfRun, 0 if none yet)
     152       8  total_events (u64, all events since the last Clear)
     160       8  total_neutrons (u64, neutrons landed in-bounds since last Clear)
     168       8  lifetime_ns (u64, last event time - first event time since last Clear)
     176       8  tzero_count (u64, Tzero events since the last Clear)
     184      40  monitor_counts (5 x u64, indexed by Monitor{num}, since last Clear)
     224    nx*ny*nt x 4  histogram bins (u32, LE)

`umami_client.Shm` does the actual `shm_open`/mmap/magic-check and exports
the segment via the buffer protocol; this module only adds the
numpy/pyqtgraph-facing readers on top.
"""

import numpy as np

from umami_client import Shm


class ShmHistogram(Shm):
    """Read-only view onto a UMAMI shared-memory histogram segment.

    Note: the `ni` header field is not implemented on the UMAMI/Rust side --
    the histogram offset math there never factors it in, and the shm segment
    is only ever sized for nx*ny*nt elements. It is read here only to be
    ignored; do not use it to size buffers or offsets.
    """

    HEADER_SIZE = 224

    def read_run_id(self):
        return self.run_id

    def read_run_start(self):
        """Unix timestamp of the last StartOfRun, or 0 if none yet.

        Re-read on every call, like read_run_id() -- both change whenever a
        new run starts, unlike nx/ny/nt which are fixed for the lifetime of
        this shm segment.
        """
        return self.run_start

    def read_running(self):
        """Whether a run is currently active (between StartOfRun and EndOfRun)."""
        return self.running

    def read_counters(self):
        """Read the events/neutrons/lifetime/tzero/monitors counters.

        Returns `(total_events, total_neutrons, lifetime_ns, tzero_count,
        monitor_counts)`, all accumulated since the last Clear.
        """
        return (self.total_events, self.total_neutrons, self.lifetime_ns,
                self.tzero_count, self.monitor_counts)

    def read_plane(self, t=0):
        offset = self.HEADER_SIZE + t * self.nx * self.ny * 4
        return np.frombuffer(self, '<u4', self.nx * self.ny, offset) \
                 .reshape((self.ny, self.nx))

    def read_time_projection(self, n=None):
        n = self.nt if n is None else min(n, self.nt)
        return np.frombuffer(self, '<u4', self.nx * self.ny * n, self.HEADER_SIZE) \
                 .reshape((n, self.ny, self.nx)) \
                 .sum(axis=(1, 2))
