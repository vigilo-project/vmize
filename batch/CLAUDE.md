# CLAUDE.md

## Project

batch — VMize's Rust task runner that executes shell-script workloads inside ephemeral VMs via the [vm](https://github.com/vigilo-project/vm) crate.

## Commands

```bash
cargo build                    # Debug build
cargo build --release          # Release build
cargo test -p batch            # All tests (integration paths require QEMU)
cargo test -p batch --lib      # Library/unit tests only
cargo run -p vmize -- task --help
cargo clippy                   # Lint
cargo fmt                      # Format
```

## Module Layout

- **`runner.rs`** — orchestration: `run_task` (async) / `run_task_blocking` (sync), helper functions `prepare_vm`, `execute_scripts`
- **`error.rs`** — `Error` enum (`thiserror`), one variant per pipeline stage
- **`result.rs`** — `RunResult` (vm_id, output_dir, executed_scripts, exit_code, elapsed_ms)
- **`../cli/src/main.rs`** — workspace CLI entry point (`vmize task` sequential and `--concurrent`)

## Core Flow (`runner.rs`)

1. Validate paths, discover scripts (sorted alphabetically)
2. `vm::run()` → spin up VM
3. `prepare_vm()` → mkdir + copy scripts into VM
4. `execute_scripts()` → run each script via SSH, stream output
5. `vm::cp_from()` → collect `/tmp/batch/out` back to host
6. `vm::rm()` → destroy VM

Cleanup always runs, even on failure. Errors are combined, not swallowed.

## Conventions

- **Error handling**: `thiserror` for `Error` enum; `vm` crate errors are stringified
- **Async**: tokio for VM ops; blocking variants create their own runtime
- **Scripts**: executed in alphabetical order (`00_`, `10_`, `20_` naming)
