# batch

`batch` is VMize's task execution engine.
It runs task directories in ephemeral VMs so each workload is isolated.
`batch` is a library crate; use the workspace `vmize` CLI to execute tasks.

Built on top of [`vm`](https://github.com/vigilo-project/vm), `batch` defines a task as:
- `task.json` — task definition with explicit `commands` list
- `input/` — scripts and assets to copy into the VM
- `output/` — collected output files and per-command logs

Shared curated tasks in this workspace live in `../tasks/`.

## Minimum Goal
`batch` is considered healthy when it can:
1. Load and run valid task directories.
2. Collect output artifacts back to host `output/`.
3. Exit non-zero on task/script failures.
4. Enforce `--concurrent` max of 4 tasks.

## Acceptance Checklist
- Single-task run works end-to-end.
- Multi-task run works sequentially.
- `--concurrent` accepts up to 4 tasks and rejects the 5th.
- Output files are present after successful runs.
- Per-command logs appear in `output/logs/`.

## Quick Start

```bash
cargo build --release

# Single task
./target/release/vmize task batch/example/task1

# Multiple tasks (sequential)
./target/release/vmize task batch/example/task1 batch/example/task2

# Concurrent (max 4)
./target/release/vmize task --concurrent \
  batch/example/split-task1 \
  batch/example/split-task2 \
  batch/example/split-task3 \
  batch/example/split-task4

# Shared tasks
./target/release/vmize task tasks/runc
./target/release/vmize task tasks/runc-llama
```

## Task Directory Structure

```text
<task-dir>/
├── task.json
├── input/          # Scripts and binary assets
└── output/
    └── logs/       # Per-command stdout/stderr (auto-created)
```

Example `task.json`:

```json
{
  "name": "my-task",
  "description": "Build and test in an ephemeral VM",
  "disk_size": "20G",
  "commands": ["00_setup.sh", "10_run.sh"],
  "artifacts": ["result.txt"]
}
```

`commands` — ordered list of files in `input/` to execute inside the VM.
`artifacts` — expected output files; if omitted, all of `/tmp/batch/out/` is copied back.

## Verification Commands

```bash
cargo test -p batch --lib     # unit tests only (no QEMU)
cargo test -p batch           # all tests (integration requires QEMU)

# Optional integration paths
BATCH_OLLAMA_IT=1 cargo test run_task_ollama_prompt_collects_answer --test integration -p batch -- --nocapture
```
