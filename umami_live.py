# *****************************************************************************
# MLZ library of Tango servers
# Copyright (c) 2015-present by the authors, see LICENSE
#
# This program is free software; you can redistribute it and/or modify it under
# the terms of the GNU General Public License as published by the Free Software
# Foundation; either version 2 of the License, or (at your option) any later
# version.
#
# This program is distributed in the hope that it will be useful, but WITHOUT
# ANY WARRANTY; without even the implied warranty of MERCHANTABILITY or FITNESS
# FOR A PARTICULAR PURPOSE.  See the GNU General Public License for more
# details.
#
# You should have received a copy of the GNU General Public License along with
# this program; if not, write to the Free Software Foundation, Inc.,
# 59 Temple Place, Suite 330, Boston, MA  02111-1307  USA
#
# Module authors:
#   Georg Brandl <g.brandl@fz-juelich.de>
#
# *****************************************************************************

"""Physics-based live view of a running UMAMI instance's event stream."""

import socket
import struct
import threading
import time

import numpy as np
import umami_client as umami

from entangle import base
from entangle.core import FAULT, ON, Prop, nonemptystring, uint16
from entangle.core.errors import ConfigurationError
from entangle.lib.loggers import FdLogMixin

# frame header: 1-byte tag, 4-byte little-endian payload length, see
# docs/outputs.md for the full wire format
FRAME_HEADER = struct.Struct('<BI')
TAG_EVENTS, TAG_START_OF_RUN, TAG_END_OF_RUN, TAG_CLEAR = range(4)

RECONNECT_DELAY_S = 1.0


class LiveViewChannel(FdLogMixin, base.ImageChannel):
    """Converts UMAMI's rel_time/x/y event stream into a physical-coordinate
    live view, published as its own shared-memory histogram."""

    attributes = {
        # TODO: whatever calibration/config parameters the conversion needs
        # exposed and settable
    }

    properties = {
        'ipc_name':    Prop(nonemptystring, 'IPC name of the running UMAMI instance.'),
        'output_name': Prop(nonemptystring, "Name of UMAMI's ext_process output."),
        'histo_name':  Prop(nonemptystring,
                            "Name matching one of that output's declared 'histos'."),
        'nx':          Prop(uint16, 'Output histogram width.'),
        'ny':          Prop(uint16, 'Output histogram height.'),
        'nt':          Prop(uint16, 'Output histogram time slices.'),
    }

    _writer = None
    _sock = None
    _thread = None

    def init(self):
        if not self.ipc_name:
            raise ConfigurationError('ipc_name must be set')
        self.init_fd_log('umami_live')

        self._stop = threading.Event()
        self._connected = False
        self._last_error = None

        # matches aux_histo's shm-naming convention, so umami-gui's "Other
        # Histograms" window (which discovers this output's declared
        # `histos`) can find the segment
        shm_name = f'{self.ipc_name}_{self.output_name}_{self.histo_name}'
        self._writer = umami.ShmWriter(shm_name, self.nx, self.ny, self.nt)
        self._data = np.asarray(self._writer)

        self._sock_name = f'{self.ipc_name}_{self.output_name}'
        self._thread = threading.Thread(
            target=self._run, name='event-consumer', daemon=True)
        self._thread.start()

    def delete(self):
        self._stop.set()
        if self._sock is not None:
            self._sock.close()
        if self._thread is not None:
            self._thread.join(timeout=2.0)
        self.delete_fd_log()
        self._writer = None
        self._data = None

    # ---- background event-consumer thread ----

    def _run(self):
        while not self._stop.is_set():
            try:
                self._connect()
                self._connected = True
                self._read_loop()
            except (OSError, ConnectionError) as e:
                self._last_error = str(e)
            self._connected = False
            if self._sock is not None:
                self._sock.close()
                self._sock = None
            if not self._stop.is_set():
                time.sleep(RECONNECT_DELAY_S)

    def _connect(self):
        # abstract namespace: leading NUL byte, same convention UMAMI's own
        # Rust side uses via the `uds` crate
        self._sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self._sock.connect('\0' + self._sock_name)

    def _recv_exact(self, n):
        buf = bytearray()
        while len(buf) < n:
            chunk = self._sock.recv(n - len(buf))
            if not chunk:
                raise ConnectionError('ext_process socket closed')
            buf += chunk
        return bytes(buf)

    def _read_loop(self):
        while not self._stop.is_set():
            header = self._recv_exact(FRAME_HEADER.size)
            tag, length = FRAME_HEADER.unpack(header)
            payload = self._recv_exact(length) if length else b''
            if tag == TAG_EVENTS:
                events = umami.decode_events_xy(payload)
                self._process_events(events)
            elif tag == TAG_START_OF_RUN:
                run_id = payload.decode()
                self._writer.set_run_id(run_id)
                self._writer.set_running(True)
                self._writer.clear_histo()  # TODO: confirm this is wanted
                # TODO: reset any of your own per-run accumulator state here
            elif tag == TAG_END_OF_RUN:
                self._writer.set_running(False)
            elif tag == TAG_CLEAR:
                self._writer.clear_histo()
                # TODO: reset any of your own per-run accumulator state here

    def _process_events(self, events):
        """`events` is a numpy structured array with fields `rel_time_ns`,
        `x`, `y` -- the common case for a live view. If you need to filter
        by event type (e.g. neutrons only) or need other fields (channel,
        ampl, flags, ...), switch to `umami.decode_events` instead, which
        exposes the full record.

        Convert x/y (and rel_time_ns, if the physical transform is
        time-dependent) into physical coordinates, then accumulate into
        self._data.
        """
        # TODO: real physics-informed conversion goes here. Sketch:
        #
        #   phys_x, phys_y = your_conversion_library.convert(
        #       events['x'], events['y'])
        #   np.add.at(self._data, (phys_y, phys_x), 1)
        raise NotImplementedError

    # ---- Tango-facing attributes ----

    def read_detectorSize(self):
        return [self.nx, self.ny, self.nt]

    def read_roiOffset(self):
        return [0, 0, 0]

    def read_roiSize(self):
        return self.read_detectorSize()

    def read_binning(self):
        return [1, 1, 1]

    def read_zeroPoint(self):
        return [0, 0, 0]

    def read_active(self):
        return False

    def read_value(self):
        return self._data.reshape(-1)[:self.nt * self.ny * self.nx]

    def state(self):
        if not self._connected:
            return FAULT, self._last_error or 'Not connected to UMAMI event socket'
        return ON, ''
