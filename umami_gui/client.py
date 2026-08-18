# Part of the Unified Mechanism for Acquisition of Measured Intensity
# (UMAMI), see README and LICENSE files for more info.

"""Command-socket client for talking to a running UMAMI pipeline."""

import umami_client

SOCKET_TIMEOUT = 0.5


class UmamiClient(umami_client.Client):
    """Talks to the UMAMI command socket.

    Never raises to callers; every failure is logged and the call returns
    None instead.
    """

    def __new__(cls, ipc_name, log):  # noqa: ARG004
        # PyO3's constructor is `__new__` not `__init__` so we need to override
        # it here to not pass our `log` argument
        return super().__new__(cls, ipc_name, timeout=SOCKET_TIMEOUT)

    def __init__(self, ipc_name, log):
        self.ipc_name = ipc_name
        self.log = log
        self._busy = False

    def _call(self, cmd_name, quiet=False, **kwargs):
        if self._busy:
            return None  # a previous call is still (conceptually) in flight
        self._busy = True
        try:
            msg = {'command': cmd_name, **kwargs}
            if not quiet:
                self.log.info(f'-> {msg}')
            try:
                result = getattr(super(), cmd_name)(**kwargs)
            except umami_client.UmamiError as e:
                module, message = e.args
                if quiet:
                    self.log.warning(f'-> {msg}')
                prefix = f'[{module}] ' if module else ''
                self.log.error(f'{prefix}{message}')
                return None
            except umami_client.UmamiClientError as e:
                self.log.warning(f'Lost connection to {self.ipc_name!r}: {e}')
                return None
            if not quiet:
                self.log.info(f'<- {result}')
            return result
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
        return self._call('get_modes', quiet=True)

    def set_mode(self, name):
        return self._call('set_mode', name=name)

    def get_params(self, full=False):
        return self._call('get_params', full=full, quiet=True)

    def set_params(self, params):
        return self._call('set_params', params=params)

    def set_replay_files(self, mapping):
        """Point each named input's `replay_file` param at a new dump file.

        `mapping` is `{input_name: path}`.
        """
        return self.set_params({f'{name}.replay_file': path
                                 for name, path in mapping.items()})

    def save_histo(self, path, max_nt):
        return self._call('save_histo', path=path, max_nt=max_nt)

    def save_config(self, path=None):
        return self._call('save_config', path=path)
