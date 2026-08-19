# Outputs reference

The optional `[outputs]` section configures zero or more output modules,
each consuming the full sorted event stream. Each entry needs a `type`:

```toml
[outputs.raw]
type = "file"
dir = "/data/umami-events"
```

Available types:

## `type = "none"`

No-op output, discards everything. No config keys.

## `type = "diag"`

Debug/diagnostic printer: prints selected events, tracks out-of-order
arrivals, and can print a rolling inter-arrival-time histogram.

* `event_mask` (optional, default empty = nothing printed): event-type flags
  to print, combined with `|`, e.g. `"NEUTRON|EDGE"`. Available flags:
  `NEUTRON`, `MONITOR`, `EDGE`, `GATE`, `TZERO`, `AUX`, `HEARTBEAT`, `VOID`,
  and the combined flags `SIGNAL`, `EDGES`, `OTHER`, `NOTNEUTRON`, `ALL`.
* `check_order` (optional, default false): count/report events arriving out
  of timestamp order
* `print_every` (optional, default never): print a running summary every N
  events
* `ts_histogram` (optional, default false): track and print (to stdout)
  relative-time histogram for each run, good to determine initial spread of
  timestamp values

## `type = "hdf5"` (requires building with `--features hdf5`)

Writes events to an HDF5 file with NeXus conventions.

* `dir` (required): directory to write files to
* `filename` (optional, default `<run id>.h5`): filename within `dir`

## `type = "file"`

Writes raw events to a plain file in the binary format.

* `dir` (required): directory to write files to
* `filename` (optional, default derived from the run ID): filename within
  `dir`

## `type = "aux_histo"`

User-defined 1-D/2-D diagnostic histograms (e.g. amplitude spectra,
ADC-pair correlations), each evaluated per-event via a small expression
language and backed by its own shared-memory segment (named
`<ipc_name>_<output_name>_<histo_name>`), viewable in `umami-gui`'s
"Aux Histograms" window.

```toml
[outputs.aux]
type = "aux_histo"

[[outputs.aux.histos]]
name = "amp_vs_x"
filter = "evtype == neutron"                 # optional, default "true"
x = { expr = "x", bins = 256, min = 0, max = 255 }
y = { expr = "ampl", bins = 256, min = 0, max = 255 }  # optional -> 1-D if omitted

[[outputs.aux.histos]]
name = "adc0_spectrum"
x = { expr = "raw_0[0..12:signed]", bins = 4096, min = -2048, max = 2047 }
```

* `enabled` (optional, default true): global on/off switch; runtime-settable
* `histos` (optional, default empty): list of histogram definitions,
  runtime-settable via `set-params` -- setting a new list unlinks all
  current shm segments and recreates fresh ones from the new definitions
* `available_aliases` (read-only): the current alias table (from recipes
  and `expr_aliases`), reported so a client can show what's available

Each histogram definition (`HistoSpec`):

* `name` (required)
* `filter` (optional, default `"true"`): expression; nonzero/true keeps the
  event
* `x` (required), `y` (optional, adds a second dimension): axis
  specifications
* `group` (optional): arbitrary string; the GUI puts every 1-D histogram
  sharing the same group name in the same plot

Each axis (`AxisSpec`):

* `expr` (required): the expression to bin
* `bins` (required): number of bins
* `min`, `max` (required, both inclusive): values outside `[min, max]` are
  silently dropped, not clamped

### Expression language

```text
expr    := or
or      := and ( "||" and )*
and     := cmp ( "&&" cmp )*
cmp     := add ( ("=="|"!="|"<"|"<="|">"|">=") add )?
add     := mul ( ("+"|"-") mul )*
mul     := unary ( ("*"|"/"|"&"|">>"|"<<") unary )*
unary   := ("-"|"!") unary | postfix
postfix := primary ( "[" int ".." int (":signed")? "]" )*
primary := int | ident | "(" expr ")"
```

Fields: `time`, `rel_time` (nanoseconds), `raw_0`, `raw_1` (the
event's raw data fields), `channel`, `ampl`, `x`, `y`, `t`, `i` (the event's
computed histogram coordinates), `flags`, `evtype`, `auxnum`, `monnum`,
`gateup`.

Named constants (for `evtype` comparisons): `neutron`, `monitor`, `edge`,
`gate`, `tzero`, `auxsignal`, `heartbeat`, `void`.

Integer literals: decimal (`100`), hex (`0xFF`), or binary (`0b1010`).

Bit-slice: `<expr>[offset..end]` (unsigned) or `[offset..end:signed]`
(sign-extended), e.g. `raw_0[0..12:signed]` extracts the low 12 bits of
`raw_0` as a signed integer.

An identifier that isn't a known field or named constant is looked up in an
alias table: named expressions contributed by the active input recipe (e.g.
`adc0` for `raw_0[0..12:signed]`) plus any `[expr_aliases]` defined in the
config file:

```toml
[expr_aliases]
adc0 = "raw_0[0..12:signed]"
adc1 = { expr = "raw_0[16..28:signed]", help = "Second ADC channel" }
```

## `type = "ext_process"`

Forwards the full sorted event stream to a single external consumer, over an
abstract-namespace Unix stream socket named `<ipc_name>_<output_name>`, for
processing outside UMAMI.

* The `histos` parameter declares the external consumer's shm histogram(s)
  for GUI discovery -- `name`, `x` (required), `y`/`t` (optional, so a
  histogram can be 1-D/2-D/3-D). Each axis has `name`, `bins`, and `min`/`max`
  (the value of the first/last bin). The consumer needs to publish each
  histogram at `<ipc_name>_<output_name>_<histo_name>`.

```toml
[[outputs.live.histos]]
name = "qx_qy"
x = { name = "qx", bins = 200, min = -2.0, max = 2.0 }
y = { name = "qy", bins = 100, min = -1.0, max = 1.0 }
```

Only one connection is accepted at a time; a second connection replaces the
first.

Sending never blocks the pipeline. If the consumer can't keep up, queued
event batches are dropped; start/end-of-run and clear frames are always
delivered.

One frame is sent per event batch and per run-boundary event:

| Bytes | Meaning |
|---|---|
| 1 | tag: `0` = events, `1` = start of run, `2` = end of run, `3` = clear |
| 4 | payload length, u32 little-endian |
| N | payload |

Payload by tag:

* Events (`0`): an array of binary `Event` structs.
* Start of run (`1`): UTF-8 run ID bytes.
* End of run (`2`) / clear (`3`): empty (length `0`).

A Python consumer doesn't need to implement this framing itself:
`umami_client.EventReceiver(ipc_name, output_name, callback)` runs the
connect-with-retry and frame-parsing loop on a background thread, calling
`callback.on_events(bytes)` (undecoded -- pick `decode_events` or
`decode_events_xy` yourself), `on_start_of_run(run_id)`, `on_end_of_run()`,
and `on_clear()`.
