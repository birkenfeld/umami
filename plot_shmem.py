import mmap
import time
import cffi
import numpy as np
import matplotlib.pyplot as plt

ffi = cffi.FFI()
ffi.cdef("""
int shm_open(const char *name, int flags, unsigned int mode);
int shm_unlink(const char *name);
#define O_RDWR  0002
""")

off_size = 128 + 4*4

lib = ffi.dlopen('rt')
fd = lib.shm_open(b"umami", lib.O_RDWR, 0o666)
if fd < 0:
    raise RuntimeError('Could not open shared memory')
mapp = mmap.mmap(fd, off_size)
header_values = np.frombuffer(mapp, '<u4')

nmod = header_values[32]
nx = header_values[33]
ny = header_values[34]
del header_values
mapp.close()

mapp = mmap.mmap(fd, off_size + nx*ny*4, prot=mmap.PROT_READ)

fig = plt.figure(f'UMAMI histogram, {nmod} modules')
plt.ion()
plt.show()

buf = np.frombuffer(mapp, '<u4', nx*ny, off_size).reshape((ny, nx))
axes_img = plt.imshow(buf, aspect='auto', origin='lower')

while True:
    time.sleep(0.25)
    buf = np.frombuffer(mapp, '<u4', nx*ny, off_size).reshape((ny, nx))
    fig.gca().set_title(f'Total events: {buf.sum()}')
    axes_img.set_data(buf)
    axes_img.set_clim(0, buf.max())
    fig.canvas.flush_events()
