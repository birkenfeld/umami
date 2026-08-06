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
"""

import mmap
import os

import cffi
import numpy as np

ffi = cffi.FFI()
ffi.cdef('''
int shm_open(const char *name, int flags, unsigned int mode);
int shm_unlink(const char *name);
''')

MAGIC = b'UMAMI01 '
HEADER_SIZE = 224

RUNNING_BIT = 1 << 1


class ShmHistogram:
    """Read-only view onto a UMAMI shared-memory histogram segment.

    Note: the `ni` header field is not implemented on the UMAMI/Rust side --
    the histogram offset math there never factors it in, and the shm segment
    is only ever sized for nx*ny*nt elements. It is read here only to be
    ignored; do not use it to size buffers or offsets.
    """

    def __init__(self, shm_name):
        lib = ffi.dlopen('rt')
        fd = lib.shm_open(shm_name.encode(), os.O_RDONLY, 0o666)
        if fd < 0:
            msg = f'Could not open shared memory {shm_name!r}: {os.strerror(-fd)}'
            raise RuntimeError(msg)
        self.fd = fd
        header_map = mmap.mmap(fd, HEADER_SIZE, prot=mmap.PROT_READ)
        magic = np.frombuffer(header_map, 'S8', count=1, offset=0)[0]
        if magic != MAGIC:
            header_map.close()
            os.close(fd)
            msg = (f'Shared memory {shm_name!r} has magic {magic!r}, '
                   f'expected {MAGIC!r} -- incompatible umami version?')
            raise RuntimeError(msg)
        header = np.frombuffer(header_map, '<u2', count=4, offset=140)
        self.nx = int(header[0])
        self.ny = int(header[1])
        self.nt = int(header[2])
        del header  # release the buffer export so header_map can be closed
        header_map.close()

        self.mapp = mmap.mmap(fd, HEADER_SIZE + self.nx * self.ny * self.nt * 4,
                               prot=mmap.PROT_READ)

    def close(self):
        self.mapp.close()
        os.close(self.fd)

    def read_run_id(self):
        return np.frombuffer(self.mapp, 'S128', 1, 8)[0].decode('ascii').rstrip('\x00')

    def read_run_start(self):
        """Unix timestamp of the last StartOfRun, or 0 if none yet.

        Re-read on every call, like read_run_id() -- both change whenever a
        new run starts, unlike nx/ny/nt which are cached once at construction
        since they're fixed for the lifetime of this shm segment.
        """
        return int(np.frombuffer(self.mapp, '<u4', 1, 148)[0])

    def read_running(self):
        """Whether a run is currently active (between StartOfRun and EndOfRun)."""
        global_state = int(np.frombuffer(self.mapp, '<u4', 1, 136)[0])
        return bool(global_state & RUNNING_BIT)

    def read_counters(self):
        """Read the events/neutrons/lifetime/tzero/monitors counters.

        Returns `(total_events, total_neutrons, lifetime_ns, tzero_count,
        monitor_counts)`, all accumulated since the last Clear.
        """
        v = np.frombuffer(self.mapp, '<u8', 9, 152)
        return int(v[0]), int(v[1]), int(v[2]), int(v[3]), v[4:9].tolist()

    def read_plane(self, t=0):
        offset = HEADER_SIZE + t * self.nx * self.ny * 4
        return np.frombuffer(self.mapp, '<u4', self.nx * self.ny, offset) \
                 .reshape((self.ny, self.nx))

    def read_time_projection(self, n=None):
        n = self.nt if n is None else min(n, self.nt)
        return np.frombuffer(self.mapp, '<u4', self.nx * self.ny * n, HEADER_SIZE) \
                 .reshape((n, self.ny, self.nx)) \
                 .sum(axis=(1, 2))
