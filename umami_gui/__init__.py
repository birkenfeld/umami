# Part of the Unified Mechanism for Acquisition of Measured Intensity
# (UMAMI), see README and LICENSE files for more info.

"""Live histogram viewer and debugging tool for UMAMI shared-memory output.

Reads a 3-D histogram (x * y * t) from a POSIX shared-memory segment (see
`umami_gui.shm` for the wire layout) and displays the selected t-plane as a
real-time, log-scale colour image using PyQtGraph. Two optional 1-D
projections can be opened in separate windows: a diffractogram (counts vs.
x channel, summed over y, for the current t-plane) and a TOF spectrum
(counts vs. t bin, summed over the whole x/y plane at each t).

A control panel sends JSON commands to the UMAMI pipeline via an abstract
Unix datagram socket: Reset, Clear, Start/Stop a run, switch processing
mode, toggle raw-event dumping, save the current histogram to a file, and
view/edit live recipe and output parameters. A status panel shows the
connection state and the state of each configured input. A log of every
command sent and reply received -- replacing the previous transient error
popups -- is available via the "Show Log" toggle (hidden by default) to
make this useful for debugging a running pipeline, not just watching it.

A separate "Aux Histograms" window discovers and displays any configured
`aux_histo` output(s): each is a user-defined 1-D or 2-D diagnostic
histogram (e.g. an amplitude spectrum) evaluated by a small expression
language, each backed by its own shared-memory segment named
"<ipc_name>_<output_name>_<histo_name>".

Usage: umami-gui [ipc_name]  (defaults to "umami"; this name is used for
both the shared-memory segment and the command socket.)
"""

import importlib.metadata

try:
    __version__ = importlib.metadata.version(__package__)
except importlib.metadata.PackageNotFoundError:
    try:
        from setuptools_scm import get_version  # pylint: disable=import-error
        __version__ = get_version(root='..', relative_to=__file__)
        del get_version
    except Exception:  # noqa: BLE001
        __version__ = 'unknown'
