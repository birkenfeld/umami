# Part of the Unified Mechanism for Acquisition of Measured Intensity
# (UMAMI), see README and LICENSE files for more info.

"""Command-socket client for talking to a running UMAMI pipeline."""

import itertools
import json
import os
import socket

SOCKET_TIMEOUT = 0.5

_bind_counter = itertools.count()


class UmamiClient:
    """Talks to the UMAMI command socket.

    Never raises to callers; every failure is logged and the call returns
    None instead.
    """

    def __init__(self, ipc_name, log):
        self.ipc_name = ipc_name
        self.log = log
        self.connected = False
        self._busy = False
        self.sock = None
        self._new_socket()

    def _new_socket(self):
        # Bind to a fresh local address every time, recovers from a timeout.
        if self.sock is not None:
            self.sock.close()
        self.sock = socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM)
        self.sock.settimeout(SOCKET_TIMEOUT)
        self.sock.bind('\0plot-' + self.ipc_name + '-' + str(os.getpid()) +
                        '-' + str(next(_bind_counter)))

    def _ensure_connected(self):
        if self.connected:
            return True
        self._new_socket()
        try:
            self.sock.connect('\0' + self.ipc_name)
        except OSError as e:
            self.log.warning(f'Cannot reach {self.ipc_name!r}: {e}')
            return False
        self.connected = True
        return True

    def _call(self, cmd, quiet=False, **kwargs):
        if self._busy:
            return None  # a previous call is still (conceptually) in flight
        self._busy = True
        try:
            if not self._ensure_connected():
                return None
            msg = json.dumps({'command': cmd, **kwargs})
            if not quiet:
                self.log.info(f'-> {msg}')
            try:
                self.sock.sendall(msg.encode())
                raw = self.sock.recv(4096)
            except OSError as e:
                self.connected = False
                self.log.warning(f'Lost connection to {self.ipc_name!r}: {e}')
                return None
            reply = json.loads(raw.decode())
            if not quiet:
                self.log.info(f'<- {reply}')
            if reply['result'] == 'error':
                module = reply.get('module')
                prefix = f'[{module}] ' if module else ''
                self.log.error(f'{prefix}{reply["message"]}')
                return None
            return reply.get('value')
        finally:
            self._busy = False

    def ping(self):
        return self._call('ping')

    def clear(self):
        return self._call('clear')

    def start(self, run_id):
        return self._call('start', run_id=run_id)

    def stop(self):
        return self._call('stop')

    def reset(self):
        return self._call('reset')

    def get_state(self):
        return self._call('get_state', quiet=True)

    def set_raw_dump(self, enable, path):
        return self._call('set_raw_dump', enable=enable, path=path)

    def get_modes(self):
        return self._call('get_modes')

    def set_mode(self, name):
        return self._call('set_mode', name=name)

    def get_params(self):
        return self._call('get_params')

    def set_params(self, params):
        return self._call('set_params', params=params)

    def save_histo(self, path, max_nt):
        return self._call('save_histo', path=path, max_nt=max_nt)
