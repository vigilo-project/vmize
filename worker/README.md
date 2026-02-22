# worker

`worker` is VMize's task execution engine.
It runs task directories in ephemeral VMs so each workload is isolated.
`worker` is a library crate; use the workspace `vmize` CLI to execute tasks.

Built on top of [`vm`](https://github.com/vigilo-project/vm), `worker` defines a task as:
- `task.json` — task definition with explicit `commands` list
- `input/` — scripts and assets to copy into the VM
- `output/` — collected output files and per-command logs

Shared curated tasks in this workspace live in `./example/`.
For chain-oriented development notes and change history, see [`TASK_CHAIN_TUTORIAL.md`](./TASK_CHAIN_TUTORIAL.md).

## Minimum Goal
`worker` is considered healthy when it can:
1. Load and run valid task directories.
2. Collect output artifacts back to host `output/`.
3. Exit non-zero on task/script failures.
4. Enforce `--batch` max of 4 tasks.

## Acceptance Checklist
- Single-task run works end-to-end.
- Multi-task run works sequentially.
- `--batch` accepts up to 4 tasks and rejects the 5th.
- Output files are present after successful runs.
- Per-command logs appear in `output/logs/`.

## Quick Start

```bash
cargo build --release

# Single task
./target/release/vmize task worker/example/task1

# Multiple tasks (sequential)
./target/release/vmize task worker/example/task1 worker/example/task2

# Concurrent (max 4)
./target/release/vmize task --batch \
  worker/example/split-task1 \
  worker/example/split-task2 \
  worker/example/split-task3 \
  worker/example/split-task4

# Shared tasks
./target/release/vmize task example/runc
./target/release/vmize task example/runc-llama-build
./target/release/vmize task worker/example/ima-sign

# runc-llama Task Chain:
#   step1: runc-llama-build (HTTP prompt flow)
#   step2: runc-llama-hardened (hardened config for UDS-oriented stage)
#   step3: runc-llama-verity-pack (squashfs + dm-verity artifact packaging)
#   step4: runc-llama-verity-run (runtime smoke/UDS check + IMA-sign + xattr tar packaging)
#   step5: runc-llama-ima-verify-run (verify tar+cert with IMA, then run UDS inference)
./target/release/vmize task worker/example/runc-llama-build
```

`worker/example/runc-llama-hardened` expects `rootfs`, `config.json`, and `model.gguf`
from `runc-llama-build`, hands off to `runc-llama-verity-pack`, and then into
`runc-llama-verity-run`, and finally to `runc-llama-ima-verify-run` for IMA-verified replay.

`worker/example/ima-sign` is an independent debug-verify task that checks
`ima_sign` + `ima_verify`, plus tar+HTTP roundtrip preservation of `security.ima`
without enabling kernel appraise policy.

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
`artifacts` — expected output files; if omitted, all of `/tmp/vmize-worker/out/` is copied back.

## Verification Commands

```bash
cargo test -p worker --lib     # unit tests only (no QEMU)
cargo test -p worker           # all tests (integration requires QEMU)

# Optional integration paths
BATCH_OLLAMA_IT=1 cargo test run_task_ollama_prompt_collects_answer --test integration -p worker -- --nocapture
```
