# CLAUDE.md

## Project

batch — VMize's Rust task runner that executes shell-script workloads inside ephemeral VMs via the [vm](https://github.com/vigilo-project/vm) crate.

## Commands

```bash
cargo build                    # Debug build
cargo build --release          # Release build
cargo test                     # All tests (requires QEMU)
cargo test --lib --bin batch  # Unit tests only
cargo clippy                   # Lint
cargo fmt                      # Format
```

## Module Layout

- **`runner.rs`** — orchestration: `run_in_out` (async) / `run_in_out_blocking` (sync), helper functions `prepare_vm`, `execute_scripts`
- **`error.rs`** — `Error` enum (`thiserror`), one variant per pipeline stage
- **`result.rs`** — `RunResult` (vm_id, output_dir, executed_scripts, exit_code, elapsed_ms)
- **`bin/batch.rs`** — CLI entry point, sequential and `--split-live` concurrent modes

## Core Flow (`runner.rs`)

1. Validate paths, discover scripts (sorted alphabetically)
2. `vm::api::run()` → spin up VM
3. `prepare_vm()` → mkdir + copy scripts into VM
4. `execute_scripts()` → run each script via SSH, stream output
5. `vm::api::cp_from()` → collect `/tmp/batch/out` back to host
6. `vm::api::rm()` → destroy VM

Cleanup always runs, even on failure. Errors are combined, not swallowed.

## Conventions

- **Error handling**: `thiserror` for `Error` enum; `vm` crate errors are stringified
- **Async**: tokio for VM ops; blocking variants create their own runtime
- **Scripts**: executed in alphabetical order (`00_`, `10_`, `20_` naming)
