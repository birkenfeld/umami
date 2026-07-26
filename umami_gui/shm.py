"""Read-only access to a UMAMI shared-memory histogram segment.

Shared-memory segment layout:

    Offset  Size  Field
    ------  ----  -----
       0     128  run_id (ASCII, NUL-padded)
     128       4  global_state (u32)
     132       2  nx (u16)
     134       2  ny (u16)
     136       2  nt (u16)
     138       2  ni (u16, unused -- not implemented on the UMAMI side)
     140    nx*ny*nt x 4  histogram bins (u32, LE)
"""

import mmap
import os

import cffi
import numpy as np

ffi = cffi.FFI()
ffi.cdef("""
int shm_open(const char *name, int flags, unsigned int mode);
int shm_unlink(const char *name);
""")

# run_id(128) + global_state(u32) + nx,ny,nt,ni(u16 x4) + reserved(u32) padding
HEADER_SIZE = 128 + 4 * 4


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
            raise RuntimeError(f'Could not open shared memory {shm_name!r}: '
                                f'{os.strerror(-fd)}')
        self.fd = fd
        header_map = mmap.mmap(fd, HEADER_SIZE, prot=mmap.PROT_READ)
        header = np.frombuffer(header_map, '<u2', count=4, offset=132)
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
        return np.frombuffer(self.mapp, 'S128', 1)[0].decode('ascii').rstrip('\x00')

    def read_plane(self, t=0):
        offset = HEADER_SIZE + t * self.nx * self.ny * 4
        return np.frombuffer(self.mapp, '<u4', self.nx * self.ny, offset) \
                 .reshape((self.ny, self.nx))

    def read_time_projection(self, n=None):
        n = self.nt if n is None else min(n, self.nt)
        return np.frombuffer(self.mapp, '<u4', self.nx * self.ny * n, HEADER_SIZE) \
                 .reshape((n, self.ny, self.nx)) \
                 .sum(axis=(1, 2))
