# AGENTS.md

## Project

UMAMI — Rust data acquisition backend for neutron detectors.
Modular pipeline: detector-specific inputs → recipes → sorter →
postprocessing → histogramming → outputs.

Produces two binaries (`umami`, `umami-ctl`), plus a standalone Python
debugging GUI (`umami-gui`, see "Python GUI" below).

## Build & Verify

Pure Cargo workspace:

```sh
cargo build                         # debug build
cargo build --release               # optimized
cargo test                          # unit + integration suite (74+ tests)
cargo clippy                        # lint
cargo fmt --check                   # format check
cargo check                         # type-check only
```

The `trace` feature gates per-event logging (`ltrace!` compiles to nothing without it):
```sh
cargo build --features trace
```

## System Dependencies

- **Linux required** (POSIX `shm_open`, abstract Unix sockets, nix crate)
- **HDF5 library** must be installed (`hdf5-metno` crate links it)
- **jemalloc** linked at compile time (global allocator in `src/bin/umami.rs`)
- Rust edition 2024, MSRV currently 1.88

## Architecture

Entry points:
- `src/bin/umami.rs` — main pipeline process
- `src/bin/umami-ctl.rs` — CLI control client

Pipeline flow (all threaded, connected via `flume` channels):
`Input` → `InputRecipe` → `Sorter` → `PostProcessor` (recipe + histogram) → `Output` chain

Key files:
- `src/pipeline.rs` — orchestration
- `src/sorter.rs` — multi-input event merger by timestamp
- `src/postproc.rs` — runs postprocess recipes, writes shared-memory histogram
- `src/shm.rs` — POSIX shared memory (mmap histogram, max 1 GB)
- `src/command.rs` — IPC protocol (JSON over Unix datagram sockets, abstract namespace)
- `src/config.rs` — TOML config deserialization
- `src/event.rs` — `Event` struct: **`#[repr(C)]`, exactly 48 bytes** (enforced by unit test)
- `src/derive/` — proc-macro sub-crate (`#[derive(HasParams)]`)

## Conventions

- **Event size is sacred.** `Event` must remain 48 bytes for `rkyv` zero-copy and shm layout.
- Thread names are ≤16 chars: `M: <input>`, `O: <output>`, `Sorter`, `Postprocessor`, `Command handler`.
- All module names (inputs, outputs, recipes, modes) are interned via `internment::Intern<String>` (`ModuleId`). Pipeline validates uniqueness at startup.
- Custom log macros: `ldebug!`, `ltrace!`, `lprintln!` — write to stderr with `jiff` timestamps. Format: `YYYY-MM-DD HH:MM:SS.ffffff : LEVEL : [module] message`.
- Outputs are daisy-chained: each output forwards events to the next. A `NullOutput` is auto-created if none configured.
- IPC commands use `#[serde(tag = "command")]` / `#[serde(tag = "result")]` for tagged JSON.

## Testing

Unit tests live alongside their modules (`#[cfg(test)] mod tests`) — recipes,
postprocessor, command handler, sorter, shm, outputs, config, event, etc.

Full pipeline integration tests (in `src/pipeline.rs`) replay real detector
dumps via `test/canon.conf`, `test/mesy.conf`, `test/ge.conf` and compare the
resulting histogram against a checked-in golden file (`test/*.golden.gz`).
The raw dump files themselves are **not** in the repo (see `.gitignore`) —
tests auto-download them into `test/data/` on first run from
https://forge.frm2.tum.de/public/umami-test/. To regenerate a golden file
after an intentional behavior change, rerun with `UMAMI_UPDATE_GOLDEN=1`.

A synthetic `type = "test"` input backend (`src/input/test.rs`,
`#[cfg(test)]`-gated) generates a fully known, deterministic event stream for
testing pipeline mechanics (wiring, sorting, histogramming) without real
detector data — see `test/synthetic.conf` / `test_pipeline_synthetic_input`.

Coverage: `cargo llvm-cov` (requires `cargo install cargo-llvm-cov` +
`rustup component add llvm-tools-preview`); currently ~78% line coverage.

## Config

Runtime config is TOML. Example configs in `test/*.conf`.

## Python GUI

`umami-gui` is a standalone, executable PyQtGraph script (no packaging, no
build step — `chmod +x` + shebang) that talks to the same command socket and
shared-memory histogram as `umami-ctl`, for interactive debugging: live
histogram + projection plot, per-input state, mode switching, live
param view/edit, raw-dump/save-histo controls, and a log of every command
sent and reply received.

- Lint: `ruff check umami-gui` (no other Python tooling/tests configured)
- To exercise it live (e.g. after a change), start a real `umami` process
  against one of the `test/*.conf` configs and drive the GUI under Xvfb:
  `Xvfb :99 -screen 0 1280x900x24 &`, then
  `DISPLAY=:99 python3 umami-gui <ipc_name>`. Screenshot with ImageMagick's
  `import -window root out.png`; drive clicks with `xdotool mousemove --sync
  X Y click 1`.
- `umami_det.py` is a second, Tango/Entangle-based client against the same
  wire protocol — useful as a reference for command usage patterns.
