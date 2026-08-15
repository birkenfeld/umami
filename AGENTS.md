# AGENTS.md

## Project

This is a neutron detector data acquisition backend with a modular pipeline:
detector-specific inputs → recipes → sorter → postprocessing → histogramming →
outputs.

Contains two binaries (`umami`, `umami-ctl`), a native Python extension module
(`umami-client`), and a standalone Python debugging GUI (`umami-gui`).

## Build & Verify

The Cargo workspace has two members: the `umami` package (root) and
`umami_client` (a PyO3 extension, depending on `umami`).  Plain `cargo
build`/`test`/`clippy` from the root only cover the `umami` package
itself; add `--workspace` to include `umami_client` too.

```sh
cargo build                                       # debug build
cargo build --release                             # optimized
cargo test                                        # unit + integration suite
cargo clippy --all-targets                        # lint
cargo check                                       # type-check only
cargo check --workspace                           # + type-check umami_client
cargo clippy --workspace --all-targets            # + lint umami_client
cargo test -p umami-client --no-default-features  # umami_client's own tests
```

## System Dependencies

- **Linux required** (POSIX `shm_open`, abstract Unix sockets, nix crate)
- **HDF5 library** must be installed (`hdf5-metno` crate links it); the
  `hdf5` output is a default-on feature, disable with `--no-default-features`
- **libjumpsd.so** (from FZJ `DriverJumiom`) is required for the `jumiom` input
  feature (off by default, `--features jumiom`); if it isn't on a standard
  linker search path, point `JUMPSD_LIB_DIR` at its directory
- Rust edition 2024, MSRV currently 1.88

## Architecture

Entry points:
- `src/bin/umami.rs` — main pipeline process
- `src/bin/umami-ctl.rs` — CLI control client

Pipeline flow (all threaded, connected via `flume` channels): `Input` →
`InputRecipe` → `Sorter` → `PostProcessor` (recipe + histogram) → `Output` chain

Key files:
- `src/pipeline.rs` — orchestration
- `src/sorter.rs` — multi-input event merger by timestamp
- `src/postproc.rs` — runs postprocess recipes, writes shared-memory histogram
- `src/shm.rs` — POSIX shared memory (mmap histogram, max 4 GB)
- `src/command.rs` — IPC protocol (JSON over abstract Unix datagram sockets)
- `src/config.rs` — TOML config deserialization
- `src/event.rs` — `Event` struct and its members
- `src/client.rs` — `Client`, the command-socket client, wrapped for Python
  by `umami_client/`
- `src/derive/` — proc-macro sub-crate for `#[derive(HasParams)]`
- `umami_client/` — PyO3 extension crate wrapping `Client` and a read-only
  `ShmReader`; see "Native Python client" below

## Conventions

- `Event` layout must not change lightly, for `rkyv` zero-copy format.
- Thread names are ≤16 chars: `M: <input>`, `O: <output>`, `Sorter`,
  `Postprocessor`, `Command handler`.
- All module names (inputs, outputs, recipes, modes) are interned via
  `internment::Intern<String>` (`ModuleId`). Pipeline validates uniqueness at
  startup.
- Custom log macros: `ldebug!`, `ltrace!`, `lprintln!` — write to stderr with
  `jiff` timestamps. Format: `YYYY-MM-DD HH:MM:SS.ffffff : LEVEL : [module]
  message`.
- Outputs are daisy-chained: each output forwards events to the next. A
  `NullOutput` is auto-created if none configured.
- IPC commands use `#[serde(tag = "command")]` / `#[serde(tag = "result")]` for
  tagged JSON.

## Testing

Unit tests live alongside their modules (`#[cfg(test)] mod tests`) — recipes,
postprocessor, command handler, sorter, shm, outputs, config, event, etc.

Full pipeline integration tests in `src/pipeline.rs` replay real detector dumps
via `test/canon.conf`, `test/mesy.conf`, `test/ge.conf` and compare the
resulting histogram against a checked-in golden file (`test/*.golden.gz`).  The
raw dump files themselves are **not** in the repo (see `.gitignore`) — tests
auto-download them into `test/data/` on first run from
https://forge.frm2.tum.de/public/umami-test/.  To regenerate a golden file after
an intentional behavior change, rerun with `UMAMI_UPDATE_GOLDEN=1`.

A synthetic `type = "test"` input backend in `src/input/test.rs` generates a
fully known, deterministic event stream for testing pipeline mechanics (wiring,
sorting, histogramming) without real detector data — see `test/synthetic.conf`.

Coverage: `cargo llvm-cov` (requires `cargo install cargo-llvm-cov` + `rustup
component add llvm-tools-preview`); currently ~81% line coverage.  The input
backends (`input.rs`, `input/canon.rs`, `input/mesy.rs`, `input/mesy/cmd.rs`)
are the weakest spots, since covering them properly would need mocking their
TCP/UDP interfaces — not planned for now.

## Manual Testing Harness

`test/harness.sh` wraps the manual test loop — building, starting a real `umami`
instance, driving it via `umami-ctl`, and exercising `umami-gui` under a
dedicated Xvfb display — into single-script invocations instead of a fresh set
of ad-hoc shell commands for every step. Run `test/harness.sh help` for the full
command list; the common flow:

```sh
test/harness.sh start canon.conf   # or mesy.conf / ge.conf; builds + starts, waits for ping
test/harness.sh ctl canon start my-run-id
test/harness.sh ctl canon state
test/harness.sh gui canon          # launches umami-gui under Xvfb :99
test/harness.sh screenshot out.png
test/harness.sh stop canon         # kills the process, drops its shm segment
```

Instances are tracked by name (default: the config's basename) under
`test/.harness/`, so several can run concurrently.  `test/synthetic.conf` (the
`type = "test"` input) can't be used here — it's `#[cfg(test)]`-gated and only
exists inside `cargo test` builds; use `canon`/`mesy`/`ge` instead.

Put scratch configs under test/ (untracked, e.g. a "_scratch_*.conf" name) if
they need to reach the checked-in test/data/* files, since the data files are
resolved relative to the config file's directory.

## Config

Runtime config is TOML.  Example configs in `test/*.conf`.

## Native Python client

`umami_client/` is a PyO3 extension crate (pure Rust) wrapping `src/client.rs`'s
`Client` and a read-only SHM reader for Python.  `cargo test`/`clippy` need
`--workspace` to reach it.

## Python GUI

`umami-gui` is a PyQtGraph-based package (`umami_gui/`) that uses
`umami_client`, for commissioning and interactive debugging: live histogram +
projection plot, per-input state, mode switching, live param view/edit,
raw-dump/save-histo controls, and a log of every command sent and reply
received.

Dependencies (`numpy`, `pyqtgraph`, `pyqt6`, `umami-client`) are managed via
`pyproject.toml` + `uv`:
```sh
uv sync                             # create/update .venv, builds umami-client too
uv run umami-gui [ipc_name]         # run it
uv run ruff check umami_gui         # lint (dev dependency group)
uv build                            # build an installable wheel
```

Prefer `uv run ruff` over an ambient system `ruff` — versions/default rule sets
can differ and surface different findings. `uv sync` needs a Rust toolchain on
`PATH` to build `umami-client` from source.

The package version is derived from git tags via `setuptools_scm` (see
`[tool.setuptools_scm]` in `pyproject.toml` and the fallback logic in
`umami_gui/__init__.py`).

- `umami_det.py` is a second client to integrate into the Tango/Entangle
  framework, also built on `umami_client`.  It is meant for production data
  acquisition, no commissioning/testing features needed.  Keep it up to date if
  the client API changes.
- To exercise the GUI live (e.g. after a change), use `test/harness.sh` (see
  "Manual Testing Harness" above), not manual `Xvfb`/`uv run`/`import`/
  `xdotool` invocations.

## Documentation

User-facing docs are `README.md` (short overview/quickstart) plus
`docs/configuration.md`, `docs/outputs.md`, `docs/cli.md` (full reference).
Keep them in sync with the code you're changing:

- Adding/renaming/removing an input type, recipe type, or output type -> update
  the relevant table in `docs/configuration.md` or `docs/outputs.md`.
- Adding/renaming/removing a config field (global, or per-input/recipe/output)
  -> update the same file, including whether it's required, optional (with
  default), or runtime-settable via `set-params`.
- Adding/renaming/removing a `umami-ctl` subcommand, or a CLI flag on
  `umami`/`umami-ctl` -> update `docs/cli.md`'s command table.
- Changing the expression language (`src/expr.rs`) -> update the grammar/field
  list in `docs/outputs.md`.

When in doubt about current behavior, verify against the actual source
(`src/config.rs`'s structs, the type-dispatch `match` in `output.rs`/
`recipe.rs`, or `umami-ctl --help`) rather than assuming an existing doc is
still accurate — this file and the docs it describes have drifted from the code
before.
