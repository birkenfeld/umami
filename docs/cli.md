# Running and controlling UMAMI

## `umami`

```console
umami [OPTIONS] [CONFIG]
```

* `CONFIG` (default `umami.conf`): path to the TOML config file
* `--start`: start acquisition immediately after initialization, using the
  run ID `auto`
* `--ipc <NAME>`: be reachable for IPC under that name, overrides `ipc_name`
  from the config file
* `--raw <PATH>`: enable raw dump to this path, overrides `raw_dir` from the
  config file
* `--debug`: enable debug logging
* `--trace`: enable per-event trace logging (if compiled in!)

If `--start` is omitted, UMAMI initializes everything, then waits for a `start`
command.  The process keeps running until it receives a terminating signal.

## `umami-ctl`

Talks to a running UMAMI instance through the IPC name selected by `ipc_name` or
`umami --ipc`.

```console
umami-ctl [--ipc umami] <command>
```

| Command | Arguments | Meaning |
|---|---|---|
| `start` | `RUN_ID` | Start a new run. If a run is already active, inputs are restarted with the new run ID. Does not clear the histogram. |
| `stop` | | Stop the current run. |
| `clear` | | Clear the histogram. Can be called while running. |
| `reset` | | Reset input modules (e.g. out of an error state). Does not clear the histogram. |
| `save-histo` | `PATH MAX_NT` | Save the current histogram (with desired number of time slices) to `PATH`, can be called while running. |
| `raw` | `PATH` | Enable raw dumping below `PATH` (one file per input, named after the input). |
| `no-raw` | | Disable raw dumping. |
| `get-modes` | | List available postprocessing mode names as JSON. |
| `set-mode` | `NAME` | Switch the active postprocessing mode. |
| `get-params` | `[--full]` | Get all recipe/input/output parameters and their current values, as JSON: `{"module.param": {"value": ...}}`. With `--full`, each entry also carries metadata, and every module gets a `"module._info" entry. |
| `set-params` | `'{"KEY": VALUE, ...}'` | Set one or more parameters, addressed by the `"module.param"` keys from `get-params`. |
| `save-config` | `[PATH]` | Save current settable parameter values into the config file the instance was started from, or `PATH` if given. |
| `state` | | Print a JSON object with the instance's display name (if set), current mode, and per-input states. |
| `ping` | | Print the UMAMI version. |

Examples:

```console
umami-ctl --ipc umami start run_0001
umami-ctl --ipc umami state
umami-ctl --ipc umami set-mode tof
umami-ctl --ipc umami set-params '{"tof.use_gate": true, "tof.aux_mode": 1}'
umami-ctl --ipc umami raw /data/umami-raw
umami-ctl --ipc umami clear
umami-ctl --ipc umami stop
umami-ctl --ipc umami no-raw
```

Note that switching modes and setting that mode's parameters are two separate
commands -- `set-mode` does not take parameters directly.

Input states, as reported by `state` and `get-state`, are one of:

* `"idle"`: ready to start
* `"running"`: actively acquiring
* `"ended"`: file-based replay input reached end of file (will rewind on start)
* `{"error": "<message>"}`: input hit an error and requires operator attention
  (`reset` to clear it)

## Inspecting the histogram

UMAMI writes histogram data to a POSIX shared-memory block whose dimensions are
defined by `[histogram]`; the run ID is stored in the shared-memory header too.

The repository includes `umami-gui`, a PyQtGraph-based viewer and debugging tool
that attaches to a shared-memory segment and displays the histogram as a live
image, alongside controls for starting/stopping runs, switching processing
modes, viewing/editing live parameters, viewing auxiliary diagnostic histograms
(see [outputs.md](outputs.md)), and a log of commands sent and replies received.
