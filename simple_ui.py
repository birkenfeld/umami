"""Simple live histogram viewer for UMAMI shared-memory output.

Reads a 3-D histogram (x × y × t) from a POSIX shared-memory segment
and displays the t=0 plane as a real-time, log-scale colour image using
PyQtGraph.  Shared-memory segment layout:

    Offset  Size  Field
    ------  ----  -----
       0     128  run_id (ASCII, NUL-padded)
     128       4  global_state (u32)
     132       2  nx (u16)
     134       2  ny (u16)
     136       2  nt (u16)
     138       2  ni (u16)
     140    nx*ny*nt*ni × 4  histogram bins (u32, LE)

Control buttons (Reset, Clear, Start, Stop) send JSON commands to the
UMAMI pipeline via an abstract Unix datagram socket; a Quit button closes
the viewer.

Features:
- Automatic refresh at 4 fps (250 ms timer).
- Log10 colour-mapped display (viridis).
- Live count-rate calculation shown in the window title.
- Accepts an optional command-line argument to select the shared-memory
  segment name (defaults to ``umami``).
"""

import os
import sys
import mmap
import cffi
import json
import time
import socket
import numpy as np
import pyqtgraph as pg
from pyqtgraph.Qt import QtCore, QtWidgets

ffi = cffi.FFI()
ffi.cdef("""
int shm_open(const char *name, int flags, unsigned int mode);
int shm_unlink(const char *name);
""")

try:
    shm_name = sys.argv[1]
except IndexError:
    shm_name = 'umami'

off_size = 128 + 4 + 2*4  # run_id(128) + global_state(u32) + nx,ny,nt,ni (4 × u16)

lib = ffi.dlopen('rt')
fd = lib.shm_open(shm_name.encode(), os.O_RDONLY, 0o666)
if fd < 0:
    raise RuntimeError('Could not open shared memory')
mapp = mmap.mmap(fd, off_size, prot=mmap.PROT_READ)
header_values = np.frombuffer(mapp, '<u2', count=4, offset=132)
nx = header_values[0]
ny = header_values[1]
del header_values
mapp.close()

mapp = mmap.mmap(fd, off_size + nx*ny*4, prot=mmap.PROT_READ)
sock = socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM)
sock.bind('\0plot-' + shm_name + '-' + str(os.getpid()))

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
        plot.setTitle(f'Run {run_id}: {total} total counts ({rate:,.1f}/sec)')
    prev['total'] = total
    prev['time'] = now

pg.setConfigOption('background', 'w')
pg.setConfigOption('foreground', 'k')
app = QtWidgets.QApplication(['umami-histogram'])

window = QtWidgets.QWidget()
window.resize(800, 800)
window.setWindowTitle('UMAMI histogram')
window.setLayout(QtWidgets.QVBoxLayout())
window.layout().setContentsMargins(0, 0, 0, 0)

graphics = pg.GraphicsLayoutWidget()
plot = graphics.addPlot()
plot.setTitle('starting...')

img = pg.ImageItem(border='w', axisOrder='row-major')
plot.addItem(img)
img.setColorMap(pg.colormap.get('viridis'))
plot.enableAutoRange('xy', True)

def send_cmd(cmd, **kwds):
    sock.connect('\0' + shm_name)
    try:
        sock.sendall(json.dumps({'command': cmd} | kwds).encode())
        reply = json.loads(sock.recv(2048).decode())
    except Exception as e:
        QtWidgets.QMessageBox.critical(window, 'Error',
                                       f'Error communicating with server: {e}')
    else:
        if reply['result'] == 'error':
            QtWidgets.QMessageBox.critical(window, 'Error',
                                           f'Error: {reply["message"]}')

buttons = QtWidgets.QFrame()
buttons.setLayout(QtWidgets.QHBoxLayout())
buttons.layout().setContentsMargins(0, 5, 0, 5)
buttons.layout().addStretch()

btn = QtWidgets.QPushButton('Reset')
btn.clicked.connect(lambda: send_cmd('reset'))
buttons.layout().addWidget(btn)

btn = QtWidgets.QPushButton('Clear')
btn.clicked.connect(lambda: send_cmd('clear'))
buttons.layout().addWidget(btn)

btn = QtWidgets.QPushButton('Start')
btn.clicked.connect(
    lambda: send_cmd('start', run_id=time.strftime('%Y-%m-%d_%H:%M:%S')))
buttons.layout().addWidget(btn)

btn = QtWidgets.QPushButton('Stop')
btn.clicked.connect(lambda: send_cmd('stop'))
buttons.layout().addWidget(btn)

btn = QtWidgets.QPushButton('Quit')
btn.clicked.connect(app.quit)
buttons.layout().addWidget(btn)

buttons.layout().addStretch()
window.layout().addWidget(buttons)
window.layout().addWidget(graphics)
window.show()

timer = QtCore.QTimer()
timer.timeout.connect(update_buffer)
timer.start(250)

app.exec()
