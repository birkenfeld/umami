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

import numpy as np
import umami_client as umami

from entangle import base
from entangle.core import FAULT, ON, Prop, nonemptystring, uint16
from entangle.core.errors import ConfigurationError
from entangle.lib.loggers import FdLogMixin


class LiveViewChannel(FdLogMixin, base.ImageChannel):
    """Converts UMAMI's rel_time/x/y event stream into a physical-coordinate
    live view, published as its own shared-memory histogram.

    `umami_client.EventReceiver` owns the connect-with-retry and frame-parsing
    loop (on its own background thread) and calls back into this class's
    `on_events`/`on_start_of_run`/`on_end_of_run`/`on_clear` methods as frames
    arrive -- undecoded, so this class decides `decode_events` vs
    `decode_events_xy`.
    """

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
    _receiver = None

    def init(self):
        if not self.ipc_name:
            raise ConfigurationError('ipc_name must be set')
        self.init_fd_log('umami_live')

        # matches aux_histo's shm-naming convention, so umami-gui's "Other
        # Histograms" window (which discovers this output's declared
        # `histos`) can find the segment
        shm_name = f'{self.ipc_name}_{self.output_name}_{self.histo_name}'
        self._writer = umami.ShmWriter(shm_name, self.nx, self.ny, self.nt)
        self._data = np.asarray(self._writer)

        self._receiver = umami.EventReceiver(self.ipc_name, self.output_name, self)

    def delete(self):
        if self._receiver is not None:
            self._receiver.stop()
        self.delete_fd_log()
        self._writer = None
        self._data = None

    # ---- EventReceiver callbacks (called from its background thread) ----

    def on_events(self, payload):
        """`payload` is the raw, undecoded frame bytes.

        `decode_events_xy` gives a numpy structured array with just
        `rel_time_ns`/`x`/`y` -- the common case for a live view. Switch to
        `umami.decode_events` instead if you need to filter by event type
        (e.g. neutrons only) or need other fields (channel, ampl, flags, ...).
        """
        events = umami.decode_events_xy(payload)
        # TODO: real physics-informed conversion goes here. Sketch:
        #
        #   phys_x, phys_y = your_conversion_library.convert(
        #       events['x'], events['y'])
        #   np.add.at(self._data, (phys_y, phys_x), 1)

    def on_start_of_run(self, run_id):
        self._writer.set_run_id(run_id)
        self._writer.set_running(True)
        self._writer.clear_histo()  # TODO: confirm this is wanted
        # TODO: reset any of your own per-run accumulator state here

    def on_end_of_run(self):
        self._writer.set_running(False)

    def on_clear(self):
        self._writer.clear_histo()
        # TODO: reset any of your own per-run accumulator state here

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
        if not self._receiver.connected:
            return FAULT, (self._receiver.last_error
                           or 'Not connected to UMAMI event socket')
        return ON, ''
