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

import json
import os
import socket
import subprocess
import time

from entangle import base
from entangle.core import BUSY, FAULT, ON, Prop, nonemptystring
from entangle.core.errors import ConfigurationError, InvalidOperation, \
    CommunicationFailure, HardwareFailure
from entangle.lib import toml
from entangle.lib.loggers import FdLogMixin
from entangle.lib.shm import SharedMemory

# Define constants used to access the shared memory area.
# Unit: 32-bit
EL_SIZE = 4

# Shared memory header length in bytes
SHM_HEAD_LEN = 128 + 128 + 4*4

# Input states
STATE_UNKNOWN = 0
STATE_INIT    = 1
STATE_RUNNING = 2
STATE_STOPPED = 3
STATE_ERRORED = 4
STATE_ENDED   = 5


class ImageChannel(FdLogMixin, base.ImageChannel):
    """Provides histogram readout for any detector supported by UMAMI."""

    attributes = {
        # 'measureMode': Attr(uint16, 'Measure mode (normal/TOF/RT).',
        #                     writable=True),
        # 'tofRange':    Attr(uint64, 'Range of TOF channels.', dims=1,
        #                     unit='us', max_x=1025, writable=True),
        # 'ignoreGate':  Attr(bool, 'Whether to ignore the gate signal.',
        #                     writable=True),
    }

    properties = {
        'umami':      Prop(nonemptystring, 'Path to the umami executable.'),
        'config':     Prop(str, 'Path to the TOML config file.'),
        'rawdatadir': Prop(str, 'Path to place raw event data.'),
    }

    _shm = None
    _proc = None
    _cmdsock = None

    def init(self):
        self._proc = None
        if not os.path.isfile(self.umami):
            raise ConfigurationError('UMAMI executable does not exist')
        if not os.path.isdir(self.rawdatadir):
            raise ConfigurationError('Raw data directory does not exist')
        if not os.path.isfile(self.config):
            raise ConfigurationError('Config file does not exist')
        with open(self.config) as f:
            content = f.read()
        config = toml.Parser(self.config, content).parse_doc()
        self._nmod = len(config['modules'])
        shm_name = config.get('ipc_name', 'umami')
        self._nx = config['histogram']['nx']
        self._ny = config['histogram']['ny']
        self._max_nt = config['histogram']['max_nt']
        array_len = self._nx * self._ny * self._max_nt
        shm_size = SHM_HEAD_LEN + array_len * EL_SIZE
        self._shm = SharedMemory(shm_name, shm_size)
        self._state = self._shm.get_array('u1', 128, 128)
        self._state[0] = 0
        self._init = self._shm.get_array('u2', 1, 256)
        self._data = self._shm.get_array('u4', array_len, SHM_HEAD_LEN)
        self._ntofbins = 1
        self._started = False
        self.init_fd_log('umami')
        self._proc = subprocess.Popen(
            [self.umami, self.config],
            cwd=os.path.dirname(self.config),
            close_fds=True,
            stderr=self.get_log_fd(),
        )
        while not (self._init[0] and
                   all(st >= STATE_INIT for st in self._state[:self._nmod])):
            if not (self._proc and self._proc.poll() is None):
                raise HardwareFailure('UMAMI process failed to start')
            time.sleep(0.1)
        self._cmdsock = socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM)
        self._cmdsock.settimeout(2)
        self._cmdsock.bind('\0tango-umami-cmd-' + self._worker_name)
        self._cmdsock.connect('\0' + shm_name)
        self._send_cmd('set_raw_dump', enable=True, path=self.rawdatadir)

    def delete(self):
        if self._cmdsock:
            self._cmdsock.close()
        if self._proc and self._proc.poll() is None:
            self._proc.kill()
        self.delete_fd_log()
        if self._shm:
            self._state = self._init = self._data = None
            self._shm.close()

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

    # def read_measureMode(self):
    #     return 0

    # def write_measureMode(self, value):
    #     pass

    # def get_measureMode_unit(self):
    #     return ''

    # def read_ignoreGate(self):
    #     return False

    # def write_ignoreGate(self, value):
    #     pass

    # def get_ignoreGate_unit(self):
    #     return ''

    # def read_tofRange(self):
    #     return self._ntofbins

    # def write_tofRange(self, value):
    #     pass

    # def get_tofRange_unit(self):
    #     return 'us'

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
        if any(st == STATE_RUNNING for st in self._state[:self._nmod]):
            return BUSY, 'counting'
        elif any(st == STATE_ERRORED for st in self._state[:self._nmod]):
            return FAULT, 'input error'
        return ON, ''

    def _send_cmd(self, cmd, **kwargs):
        # send command to UMAMI via a Unix socket
        cmd = {'command': cmd}
        cmd.update(kwargs)
        msg = json.dumps(cmd).encode()
        try:
            self._cmdsock.sendall(msg)
            ret = self._cmdsock.recv(2048)
            reply = json.loads(ret.decode())
        except Exception as e:
            raise CommunicationFailure(f'Error communicating with UMAMI: {e}')
        if reply['result'] == 'error':
            raise InvalidOperation(
                f'UMAMI error: {reply.get("message", "unknown error")} from '
                f'{reply.get("module", "unknown module")}')
        elif reply['result'] == 'data':
            return reply['value']

    def Clear(self):
        self._send_cmd('clear')

    def Prepare(self):
        self._started = False
        self.Clear()

    def Start(self):
        self._send_cmd('start', run_id=time.strftime('%Y-%m-%d_%H:%M:%S'))
        self._started = True

    def Stop(self):
        self._send_cmd('stop')

    def Resume(self):
        self.Start()
