# batch

`batch` is VMize's task execution engine.
It runs task directories in ephemeral VMs so each workload is isolated.

Built on top of [`vm`](https://github.com/vigilo-project/vm), `batch` defines a task as:
- `task.json`
- `scripts/`
- `output/`

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

## Quick Start

```bash
cargo build --release

# Single task
./target/release/batch example/task1

# Multiple tasks (sequential)
./target/release/batch example/task1 example/task2

# Concurrent (max 4)
./target/release/batch --concurrent \
  example/split-task1 \
  example/split-task2 \
  example/split-task3 \
  example/split-task4

# Shared tasks
./target/release/batch ../tasks/runc
./target/release/batch ../tasks/runc-llama
```

## Task Directory Structure

```text
<task-dir>/
├── task.json
├── scripts/
└── output/
```

Example `task.json`:

```json
{
  "name": "my-task",
  "description": "Build and test in an ephemeral VM",
  "disk_size": "20G"
}
```

## Verification Commands

```bash
cargo test -p batch

# Optional integration path
BATCH_OLLAMA_IT=1 cargo test run_task_with_options_ollama_prompt_collects_answer --test integration -p batch -- --nocapture
```
