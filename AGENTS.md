# AGENTS.md

## Project

UMAMI — Rust data acquisition backend for neutron detectors.
Modular pipeline: detector-specific inputs → recipes → sorter →
postprocessing → histogramming → outputs.

Produces two binaries (`umami`, `umamictl`).

## Build & Verify

Pure Cargo workspace:

```sh
cargo build                         # debug build
cargo build --release               # optimized
cargo test                          # runs the single unit test (event size)
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
- `src/bin/umamictl.rs` — CLI control client

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

- **Event size is sacred.** `Event` must remain 48 bytes for `rkyv` zero-copy and shm layout. The single unit test enforces this — don't remove it.
- Thread names are ≤16 chars: `M: <input>`, `O: <output>`, `Sorter`, `Postprocessor`, `Command handler`.
- All module names (inputs, outputs, recipes, modes) are interned via `internment::Intern<String>` (`ModuleId`). Pipeline validates uniqueness at startup.
- Custom log macros: `ldebug!`, `ltrace!`, `lprintln!` — write to stderr with `jiff` timestamps. Format: `YYYY-MM-DD HH:MM:SS.ffffff : LEVEL : [module] message`.
- Outputs are daisy-chained: each output forwards events to the next. A `NullOutput` is auto-created if none configured.
- IPC commands use `#[serde(tag = "command")]` / `#[serde(tag = "result")]` for tagged JSON.

## Testing

Unit tests are in progress.

Full pipeline testing requires pre-recorded detector data input — use the TOML configs in
`test/` (e.g., `mesyfile.conf` replays from `test/data/00678408.mdat`). These tests are TBW.

## Config

Runtime config is TOML. Example configs in `test/*.conf`.
