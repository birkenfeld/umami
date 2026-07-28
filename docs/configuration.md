# Configuration reference

UMAMI is configured from a single TOML file. At the top level:

| Key | Required | Meaning |
|---|---|---|
| `name` | no | display name for this instance |
| `inputs` | yes | detector input modules, keyed by a user-chosen name |
| `input_recipes` | yes | named event-processing recipes, referenced by inputs |
| `process_modes` | yes | named postprocessing recipes and the `default` mode |
| `histogram` | yes | shared-memory histogram dimensions |
| `outputs` | no | output modules, keyed by a user-chosen name -- see [outputs.md](outputs.md) |
| `ipc_name` | no | IPC name shared by `umami` and `umami-ctl` (default `"umami"`) |
| `raw_dir` | no | if set, raw event dumping to this directory starts automatically (see [cli.md](cli.md)) |
| `debug` | no | boolean, enables debug logging (also `--debug` on the command line) |
| `expr_aliases` | no | named expression aliases available to all `aux_histo` outputs -- see [outputs.md](outputs.md) |

## Inputs

`[inputs]` is a map from input name to a map that defines one detector input
module. Every input needs:

* `recipe`: name of a recipe from `[input_recipes]` applied to that input's events
* `type`: input backend type (see below)

Inputs are addressed by their name everywhere (raw dump filenames, parameter
names, status reporting).

### `type = "ge"`

Generic GE detector input.

* `source` (required): a file path (replay) or `"host:port"` (live network)
* `timestamper` (optional, default `false`): marks this input as the
  timestamper source

### `type = "canon"`

Canon detector input.

* `source` (required): file path or `"host:port"`
* `channel_offset` (required): added to the decoded PSD channel to form the
  final pixel ID -- runtime-settable via `set-params`
* `gatenet` (optional, default `false`)

### `type = "mesy"`

Mesytec MCPD input.

* `local` (required): local file path (replay) or local `"host:port"` for
  incoming data UDP socket (command socket is on port + 1)
* `remote` (required): remote MCPD command socket address
* `is_master` (required): whether this MCPD instance is the master
* `mcpd_id` (required): MCPD numeric ID
* `cells` (required): map of cell index -> `{ source, compare }`
* `modules` (required): map of module index -> module config (see below)

Both `cells` and `modules` are runtime-settable via `set-params` and are
pushed live to the hardware when changed.

Each module entry:

* `type = "mpsd"` or `type = "mstd"`
* `threshold`
* `gain`

### `type = "jumiom"` (requires building with `--features jumiom`)

Jumiom PSD input, using `libjumpsd.so`.

* `device` (required): device number, i.e. `/dev/jumpsd_d<device>`
* `mode` (required): `"tof1"`, `"raw"`, or `"ramp"` -- runtime-settable
  before acquisition
* `calibration` (optional): hardware calibration block:
  * `thresholds`: 3 ADC threshold levels
  * `pileup`: pileup rejection count
  * `poti`: 4 gain potentiometer settings
  * `dac1`, `dac2`: 4 DAC offsets each
  * `monitor_delay`, `chopper_delay` (optional, default `0`): timer reset
    delays in microseconds

If a source string (for `ge`/`canon`/`mesy`'s `local`) contains `:`, UMAMI
treats it as a network endpoint; otherwise it's opened as a replay file.

## Input recipes and processing modes

`[input_recipes]` is a pool of named recipe configurations referenced by inputs
via their `recipe` setting. `[process_modes]` configures recipes used as
postprocess modes selected at runtime (`default` sets which is active at
startup); example:

```toml
[process_modes]
default = "std"
std = { type = "histo_std", bin_x = 2 }
tof = { type = "histo_tof", bin_x = 2, bin_y = 2, use_gate = true }
```

Available recipe types:

| Type | Used as | Config keys |
|---|---|---|
| `none` | input or process mode | (none) |
| `histo_std` | process mode | `bin_x`, `bin_y` (default 1), `use_gate` (default false) |
| `histo_tof` | process mode | as `histo_std`, plus `aux_mode` (aux-signal number to use as T0, or omit to use explicit T0 events), `time_bins` (array of nanosecond bin-end times, runtime-settable) |
| `mesy_mdll` | input | (none) |
| `mesy_mpsd` | input | (none) |
| `canon` | input | (none) |
| `kws_gedet` | input | `reso_1024`, `rebin_8x8`, `invert_ts` (all bool, default false) |
| `jumiom` | input (requires `--features jumiom`) | `mode` (`fpga`\|`linear`\|`distortion`\|`formula0`\|`formula2`, default `fpga`), `offset_x`/`offset_y`, `factor_x`/`factor_y`, `a`/`b`/`c` (distortion-mode coefficients), `cutoff` (distortion radius cutoff, no cutoff if <= 0), `limits_file` (optional per-pixel accept-window file path), `use_fpga_for_limit_index` |

Recipes with runtime-changeable parameters can be inspected/changed live via
`umami-ctl get-params`/`set-params` -- see [cli.md](cli.md).

## Histogram section

`[histogram]` defines the shared-memory histogram dimensions:

* `nx`: X dimension
* `ny`: Y dimension
* `max_nt`: time dimension depth
* `max_ni`: reserved for a future 4th dimension; not yet used by the
  histogramming code

Storage is allocated as `nx * ny * max_nt` bins of `u32`.
