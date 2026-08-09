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

import os
import subprocess
import time

import numpy as np
import umami_client
from entangle import base
from entangle.core import BUSY, FAULT, OFF, INIT, UNKNOWN, ON, Attr, Cmd, \
    Prop, nonemptystring, uint16, uint64
from entangle.core.errors import ConfigurationError, InvalidOperation, \
    CommunicationFailure, HardwareFailure
from entangle.lib import toml
from entangle.lib.loggers import FdLogMixin

# Shared memory header length in bytes, i.e. the offset of the histogram
# array within the segment `umami_client.Shm` exports -- individual header
# fields aren't read directly here, `Shm`'s own properties cover those.
HEADER_SIZE = 224

# Measure modes (numbers for compatibility with older devices)
MODE_NAMES = {
    0: 'default',
    1: 'tof',
    2: 'ext_rt',
}


class ImageChannel(FdLogMixin, base.ImageChannel):
    """Provides histogram readout for any detector supported by UMAMI."""

    attributes = {
        'measureMode': Attr(uint16, 'Measure mode (normal/TOF/RT).',
                            writable=True),
        'tofRange':    Attr(uint64, 'Range of TOF channels.', dims=1,
                            unit='us', max_x=1025, writable=True),
        'ignoreGate':  Attr(bool, 'Whether to ignore the gate signal.',
                            writable=True),
    }

    commands = {
        'Command': Cmd('Communicate with UMAMI over the Unix socket.',
                       str, str,
                       'The command to send, in JSON serialized form.',
                       'The reply, in JSON serialized form.',
                       disallowed=(FAULT, OFF, INIT, UNKNOWN)),
    }

    properties = {
        'umami':      Prop(nonemptystring, 'Path to the umami executable.'),
        'config':     Prop(str, 'Path to the UMAMI TOML config file.'),
        'rawdatadir': Prop(str, 'Path to place raw event data if not empty.'),
    }

    _shm = None
    _proc = None
    _cmd = None

    def init(self):
        # check prerequisites
        if not os.path.isfile(self.umami):
            raise ConfigurationError('UMAMI executable does not exist')
        if self.rawdatadir and not os.path.isdir(self.rawdatadir):
            raise ConfigurationError('Raw data directory does not exist')
        if not os.path.isfile(self.config):
            raise ConfigurationError('Config file does not exist')

        # general initialization
        self._mode = 0
        self._tofbins = []
        self._ntofbins = 1
        self._ignoregate = False
        self._started = False

        # read initial config from UMAMI toml file
        with open(self.config) as f:
            content = f.read()
        config = toml.Parser(self.config, content).parse_doc()
        self._nmod = len(config['inputs'])

        # start the UMAMI subprocess
        self._ipc_name = 'umami-tango-' + self._worker_name.replace('/', '-')
        self.init_fd_log('umami')
        self._proc = subprocess.Popen(
            [self.umami, '--ipc', self._ipc_name, self.config],
            cwd=os.path.dirname(self.config),
            close_fds=True,
            stderr=self.get_log_fd(),
        )

        # connect to UMAMI's command socket, waiting for it to come up
        while True:
            time.sleep(0.1)
            try:
                self._cmd = umami_client.Client(self._ipc_name)
                self.hw_version = self._cmd.ping()
                break
            except umami_client.UmamiClientError:
                if self._proc.poll() is not None:
                    raise HardwareFailure('UMAMI exited during initialization')

        # set up histo readout via shared memory; nx/ny/nt come from the
        # segment's own header, not re-derived from the TOML config
        self._shm = umami_client.Shm(self._ipc_name)
        self._nx = self._shm.nx
        self._ny = self._shm.ny
        self._max_nt = self._shm.nt
        array_len = self._nx * self._ny * self._max_nt
        self._data = np.frombuffer(self._shm, '<u4', array_len, HEADER_SIZE)

        # enable raw data dumping
        if self.rawdatadir:
            self._send_cmd('set_raw_dump', enable=True, path=self.rawdatadir)

        # query measure modes and parameters
        self._measure_modes = self._send_cmd('get_modes')
        self._all_params = self._send_cmd('get_params')

    def delete(self):
        if self._proc and self._proc.poll() is None:
            self._proc.kill()
        self.delete_fd_log()
        self._data = None
        self._shm = None
        self._cmd = None

    def read_detectorSize(self):
        return [self._nx, self._ny, self._ntofbins]

    def read_roiOffset(self):
        return [0, 0, 0]

    def write_roiOffset(self, value):
        raise InvalidOperation('ROI not supported')

    def read_roiSize(self):
        return self.read_detectorSize()

    def write_roiSize(self, value):
        raise InvalidOperation('ROI not supported')

    def read_binning(self):
        return [1, 1, 1]

    def write_binning(self, value):
        raise InvalidOperation('binning not supported')

    def read_zeroPoint(self):
        return [0, 0, 0]

    def read_active(self):
        return False

    def write_active(self, value):
        if value:
            raise InvalidOperation('This channel does not support active mode')

    def read_measureMode(self):
        return self._mode

    def write_measureMode(self, value):
        mode_name = MODE_NAMES.get(value, 'unknown')
        if mode_name not in self._measure_modes:
            raise InvalidOperation(
                f'Measure mode {mode_name!r} not supported by UMAMI')
        self._send_cmd('set_mode', name=mode_name)
        self._mode = value

    def get_measureMode_unit(self):
        return ''

    def read_ignoreGate(self):
        return self._ignoregate

    def write_ignoreGate(self, value):
        send_params = {}
        for param in self._all_params:
            if param.endswith('.use_gate'):
                send_params[param] = not value
        self._send_cmd('set_params', params=send_params)
        self._ignoregate = value

    def get_ignoreGate_unit(self):
        return ''

    def read_tofRange(self):
        if self._mode == 0:
            return []
        return self._tofbins

    def write_tofRange(self, value):
        send_params = {}
        for param in self._all_params:
            if param.endswith('.time_bins'):
                send_params[param] = [int(t * 1e3) for t in value]
        self._send_cmd('set_params', params=send_params)
        # first and last bin are reserved for underflow and overflow
        self._ntofbins = len(value) + 2
        self._tofbins = value

    def get_tofRange_unit(self):
        return 'us'

    def read_preselection(self):
        return 0

    def write_preselection(self, value):
        pass

    def GetBlock(self, arg):
        offset, length = arg
        return self._data[offset:offset+length]

    def read_value(self):
        return self._data[:self._ntofbins * self._nx * self._ny]

    def state(self):
        if not (self._proc and self._proc.poll() is None):
            return FAULT, 'UMAMI process exited'
        state = self._send_cmd('get_state')['inputs']
        if any(st == 'running' for st in state.values()):
            return BUSY, 'counting'
        errors = [st['error'] for st in state.values()
                  if isinstance(st, dict) and 'error' in st]
        if errors:
            return FAULT, '; '.join(errors)
        return ON, ''

    def _send_cmd(self, cmd, **kwargs):
        try:
            return getattr(self._cmd, cmd)(**kwargs)
        except umami_client.UmamiError as e:
            module, message = e.args
            raise InvalidOperation(
                f'UMAMI error: {message} from {module or "unknown module"}')
        except Exception as e:
            raise CommunicationFailure(f'Error communicating with UMAMI: {e}')

    def Command(self, cmd):
        try:
            return self._cmd.send_json(cmd)
        except Exception as e:
            raise CommunicationFailure(f'Error communicating with UMAMI: {e}')

    def Clear(self):
        self._send_cmd('clear')

    def Prepare(self):
        self._started = False
        self.Clear()

    def Start(self):
        self._send_cmd('start', run_id=time.strftime('%Y-%m-%d_%H-%M-%S'))
        self._started = True

    def Stop(self):
        self._send_cmd('stop')

    def Resume(self):
        self.Start()
