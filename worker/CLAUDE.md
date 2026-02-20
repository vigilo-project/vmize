# CLAUDE.md

## Project

worker — VMize's Rust task runner that executes shell-script workloads inside ephemeral VMs via the [vm](https://github.com/vigilo-project/vm) crate.

## Commands

```bash
cargo build                    # Debug build
cargo build --release          # Release build
cargo test -p worker            # All tests (integration paths require QEMU)
cargo test -p worker --lib      # Library/unit tests only
cargo run -p vmize -- task --help
cargo clippy                   # Lint
cargo fmt                      # Format
```

## Task Directory Layout

```
example/my-task/
├── task.json          # Task definition
├── input/             # Scripts and assets (binary files allowed)
│   ├── 00_setup.sh
│   └── 10_run.sh
└── output/
    ├── result.txt     # Written by scripts to /tmp/vmize-worker/out/
    └── logs/          # Per-command stdout/stderr logs (00_setup.sh.log, …)
```

## task.json Schema

```json
{
  "name": "my-task",
  "description": "optional description",
  "disk_size": "20G",
  "commands": ["00_setup.sh", "10_run.sh"],
  "artifacts": ["result.txt"],
  "next_task_dir": "../next-task"
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `name` | string | no | Display name |
| `description` | string | no | Description |
| `disk_size` | string | no | VM disk size (e.g. `"20G"`) |
| `commands` | `string[]` | yes | Files to execute, relative to `input/` |
| `artifacts` | `string[]` | no | Expected output files in `output/`; if omitted, copies all of `out/` |
| `next_task_dir` | string | no | Relative path to the next task directory. If set, `artifacts` must be non-empty and are handed off into the next task's `input/` |

## Module Layout

- **`../task/src/lib.rs`** — `TaskDefinition` (serde), `LoadedTask` (resolved paths), `load_task()` (validates `input/` + command files, creates `output/logs/`)
- **`runner.rs`** — orchestration: `run_loaded_task` (async) / `run_loaded_task_blocking` (sync)
- **`error.rs`** — `Error` enum (`thiserror`), one variant per pipeline stage
- **`result.rs`** — `RunResult` (vm_id, output_dir, logs_dir, executed_commands, collected_artifacts, exit_code, elapsed_ms)
- **`vm_ops.rs`** — `VmOps` trait + `RealVmOps` (production) + `MockVmOps` (testing)
- **`../cli/src/main.rs`** — workspace CLI entry point (`vmize task` sequential and `--batch`)

## Core Flow (`runner.rs`)

1. `load_task()` — parse `task.json`, validate `input/` dir + all command files exist, create `output/logs/`
2. `vm::run()` → spin up VM
3. `prepare_vm()` — `mkdir /tmp/vmize-worker/out /tmp/vmize-worker/logs`, `scp input/ → /tmp/vmize-worker/work/`
4. `execute_commands()` — for each command: `cd /tmp/vmize-worker/work && bash {cmd} 2>&1 | tee /tmp/vmize-worker/logs/{cmd}.log`
5. Collect logs: `scp /tmp/vmize-worker/logs/* → output/logs/`
6. Collect output: if `artifacts` specified, copy each individually + verify; otherwise copy all of `/tmp/vmize-worker/out/`
7. `vm::rm()` → destroy VM

When `next_task_dir` is present, `vmize task` executes a linear chain (`task1 -> task2 -> ...`) and passes the current task's collected `artifacts` into the next task's `input/` via a temporary overlay directory.

Cleanup always runs, even on failure. Errors are combined, not swallowed. Log collection is best-effort (non-fatal).

## VM Directory Layout (inside VM)

```
/tmp/vmize-worker/
├── work/    # Contents of input/ copied here; scripts run from this directory
├── out/     # Scripts write output files here
└── logs/    # Runner writes per-command tee logs here
```

## Conventions

- **Error handling**: `thiserror` for `Error` enum; `vm` crate errors are stringified
- **Async**: tokio for VM ops; blocking variants create their own runtime
- **Commands**: executed in declaration order from `task.json` (`commands` array)
- **Log files**: named `{command_basename}.log` (e.g. `00_setup.sh` → `00_setup.sh.log`)
