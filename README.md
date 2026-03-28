Unified Mechanism for Acquisition of Measured Intensity (UMAMI)
===============================================================

UMAMI is a data acquisition backend for neutron detectors.  It implements a
"modular pipeline" approach which combines detector-specific backends with
configurable processing steps.


Building
--------

UMAMI is a Rust project and currently requires the Rust toolchain version
declared in `Cargo.toml`.

For a development build:

```console
cargo build
```

For a release build:

```console
cargo build --release
```

This produces two executables:

* `umami`: starts and runs the acquisition pipeline
* `umamictl`: sends control commands to a running `umami` instance


How it works
------------

One UMAMI process is configured by a config file that configures and starts
several components (many in separate threads) that act as a data pipeline:

* Inputs: connect to a detector/data source and reads events into the
  common internal event structure, dumping raw data to disk if wanted
* Input recipes: transform event data to assign logical meaning to event types
  and locations
* Sorters: merge events from all inputs into a single data stream, sorted by
  absolute timestamp
* Postprocess recipes: run transformations only possible looking at the sorted
  data stream, e.g. assigning a relative timestamp to events
* Histogram: automatic histogramming of events in 2- or 3-dimensional arrays
* Outputs: save the processed event stream in desired formats


Configuration
-------------

UMAMI is configured from a TOML file.

At the top level, the configuration contains these sections:

* `inputs`: detector input modules keyed by a user-chosen input name
* `input_recipes`: named event-processing recipes used by inputs
* `process_modes`: named postprocessing recipes; `default` must exist
* `histogram`: histogram dimensions
* `ipc_name`: optional IPC name shared by `umami` and `umamictl`
* `debug`: optional boolean enabling debug logging


Global settings
~~~~~~~~~~~~~~~

`ipc_name`
: Name of the control interface for this UMAMI instance.  If omitted, the
  default is `umami`.  When you run multiple instances on one host, give each
  one a unique name and pass the same name to `umamictl --ipc`.

`debug`
: Enables debug logging from the configuration file.  This can also be turned
  on from the command line with `--debug`.


Inputs
~~~~~~

Each entry in `[inputs]` defines one detector input module.  Every input needs:

* `id`: numeric input ID used internally and for raw dump file names
* `recipe`: name of a recipe from `[input_recipes]` applied to that input's events
* `type`: input backend type

The supported input types are:

`type = "ge"`
: Generic GE detector input.

  Required keys:

  * `source`: either a file path for replay data or an address string
    containing `:` such as `localhost:50001`

  Optional keys:

  * `timestamper = true`: marks the input as a timestamper source

`type = "canon"`
: Canon detector input.

  Required keys:

  * `source`: file path or network address

  Optional keys:

  * `gatenet = true`

`type = "mesy"`
:   Mesytec MCPD input.

  Required keys:

  * `local`: local file path or local address for incoming data
  * `remote`: remote MCPD control/data address
  * `is_master`: whether this MCPD instance is the master
  * `mcpd_id`: MCPD numeric ID
  * `cells`: map of cell definitions
  * `modules`: map of attached Mesytec modules

  Each cell entry must provide:

  * `source`
  * `compare`

  Each Mesytec module entry must provide:

  * `type = "mpsd"` or `type = "mstd"`
  * `threshold`
  * `gain`

If a source string contains `:`, UMAMI treats it as a network endpoint.
Otherwise it is opened as a replay file.


Recipes and processing modes
~~~~~~~~~~~~~~~~~~~~~~~~~~~~

`[input_recipes]` is a named pool of recipe configurations referenced by inputs.

Currently implemented recipe types are:

* `none`: no event transformation
* `kws_ge`: converts GE raw events into logical detector coordinates
* `histo_std`: standard histogramming postprocessing
* `tof_std`: time-of-flight postprocessing

Input recipes are selected through the `recipe` key in each input.

Postprocessing recipes are configured in `[process_modes]`.  Example:

```toml
[process_modes]
default = "std"
std = { type = "histo_std" }
tof = { type = "histo_tof", bin_x = 2, bin_y = 2 }
```

The `default` is mandatory and is active when UMAMI starts.

Postprocessing recipes can have runtime-changeable parameters depending on the
selected recipe.


Histogram section
~~~~~~~~~~~~~~~~~

The `[histogram]` section defines the shared-memory histogram dimensions:

* `nx`: X dimension
* `ny`: Y dimension
* `max_nt`: time dimension depth

The histogram storage is allocated as `nx * ny * max_nt` bins of `u32`.


Running UMAMI
-------------

Start UMAMI with a specific config file:

```console
umami path/to/umami.conf
```

Useful runtime flags:

* `--start`: start acquisition immediately after initialization using the
  synthetic run ID `auto`
* `--ipc NAME`: override `ipc_name` from the config file
* `--debug`: enable debug logging
* `--trace`: enable per-event trace logging

If `--start` is omitted, UMAMI initializes all modules, the shared-memory
histogram and the IPC control channel, then waits for a `start` command.

The process keeps running until it receives a terminating signal.


Controlling a running instance
------------------------------

`umamictl` talks to a running UMAMI instance through the IPC name selected by
`ipc_name` or `umami --ipc`.

General form:

```console
umamictl [--ipc umami] <command>
```

Available commands are:

`start RUN_ID`
: Starts a run with the given run ID.  If a run is already active, inputs are
  restarted with the new run ID.

`stop`
: Stops the current run.

`clear`
: Clears the histogram in shared memory.  This is allowed while acquisition is
  running.

`raw PATH`
: Enables raw data dumping below `PATH`.  For run `1234`, UMAMI creates the
  directory `PATH/1234/` and stores one file per module using two-digit input
  IDs such as `00`, `01`, `18`.

`no-raw`
: Disables raw dumping.

`mode NAME '{"KEY": VALUE, ...}'`
: Switches the active postprocessing mode and optionally updates its runtime
  parameters.  Parameters are passed as a JSON object.

`state`
: Prints a JSON object containing the current mode and the per-input states.

Examples:

```console
umamictl --ipc umami start run_0001
umamictl --ipc umami state
umamictl --ipc umami mode tof '{"use_gate": true, "aux_mode": 1}'
umamictl --ipc umami raw /data/umami-raw
umamictl --ipc umami clear
umamictl --ipc umami stop
umamictl --ipc umami no-raw
```

Input states are reported as one of:

* `init`: input created but not yet initialized for operation
* `idle`: ready to start
* `running`: actively acquiring
* `ended`: replay input reached end of file
* `error`: input hit an error and requires operator attention


Inspecting the histogram
------------------------

UMAMI writes histogram data to a POSIX shared-memory block whose dimensions are
defined by `[histogram]`.  The run ID is stored in the shared-memory header as
well.

The repository includes `plot_shmem.py`, a small Python viewer that attaches to
the default shared-memory object `umami` and displays the 2D histogram as a
live image.

If you use a non-default `ipc_name`, adjust the shared-memory name in the
script accordingly before running it.


Authors
-------

UMAMI is brought to you by

* Georg Brandl <g.brandl@fz-juelich.de>
* Alexander Zaft <a.zaft@fz-juelich.de>
