import mmap
import cffi
import numpy as np
import pyqtgraph as pg
from pyqtgraph.Qt import QtCore, QtWidgets

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

def update_buffer():
    buf = np.frombuffer(mapp, '<u4', nx*ny, off_size).reshape((ny, nx))
    img.setImage(buf)
    plot.setTitle(f'{buf.sum()} total counts')

pg.setConfigOption('background', 'w')
pg.setConfigOption('foreground', 'k')
app = QtWidgets.QApplication([])
window = pg.GraphicsLayoutWidget()
window.setWindowTitle(f'UMAMI histogram, {nmod} modules')
plot = window.addPlot()

img = pg.ImageItem(border='w')
plot.addItem(img)
img.setColorMap(pg.colormap.get('viridis'))
plot.enableAutoRange('xy', True)
window.show()

timer = QtCore.QTimer()
timer.timeout.connect(update_buffer)
timer.start(250)

app.exec()
