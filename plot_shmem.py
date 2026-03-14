import os
import mmap
import cffi
import time
import numpy as np
import pyqtgraph as pg
from pyqtgraph.Qt import QtCore, QtWidgets

ffi = cffi.FFI()
ffi.cdef("""
int shm_open(const char *name, int flags, unsigned int mode);
int shm_unlink(const char *name);
""")

off_size = 128 + 4*4

lib = ffi.dlopen('rt')
fd = lib.shm_open(b"umami", os.O_RDONLY, 0o666)
if fd < 0:
    raise RuntimeError('Could not open shared memory')
mapp = mmap.mmap(fd, off_size, prot=mmap.PROT_READ)
header_values = np.frombuffer(mapp, '<u4')
nx = header_values[33]
ny = header_values[34]
del header_values
mapp.close()

mapp = mmap.mmap(fd, off_size + nx*ny*4, prot=mmap.PROT_READ)

prev = dict()

def update_buffer():
    # run id is encoded as a c string in the first 128 bytes
    run_id = np.frombuffer(mapp, 'S128', 1)[0].decode('ascii').rstrip('\x00')
    buf = np.frombuffer(mapp, '<u4', nx*ny, off_size).reshape((ny, nx))
    lbuf = np.log10(buf.astype(float) + 0.1)
    img.setImage(lbuf)
    total = buf.sum()
    now = time.monotonic()
    if prev and total >= prev['total']:
        rate = (total - prev['total']) / (now - prev['time'])
        plot.setTitle(f'Run {run_id}: {total} total counts ({rate:.1f}/sec)')
    prev['total'] = total
    prev['time'] = now

pg.setConfigOption('background', 'w')
pg.setConfigOption('foreground', 'k')
app = QtWidgets.QApplication([])
window = pg.GraphicsLayoutWidget()
window.resize(800, 800)
window.setWindowTitle('UMAMI histogram')
plot = window.addPlot()
plot.setTitle('starting...')

img = pg.ImageItem(border='w', axisOrder='row-major')
plot.addItem(img)
img.setColorMap(pg.colormap.get('viridis'))
plot.enableAutoRange('xy', True)
window.show()

timer = QtCore.QTimer()
timer.timeout.connect(update_buffer)
timer.start(250)

app.exec()
