# batch

`batch` is VMize's task execution engine.
It runs **tasks** inside ephemeral VMs so each task gets an isolated machine and host state remains untouched.

Built on top of [`vm`](https://github.com/vigilo-project/vm), `batch` adds the task abstraction: a self-contained directory with a `task.json` definition, a `scripts/` input directory, and an `output/` directory for results.

In the `vmize` workspace, shared curated tasks live in `../tasks/` from this crate directory.

## Quick Start

```bash
cargo build --release

# Run a single task (directory-based)
./target/release/batch example/task1

# Run multiple tasks sequentially
./target/release/batch example/task1 example/task2

# Run up to 4 tasks concurrently (no TTY required)
./target/release/batch --concurrent \
  example/split-task1 \
  example/split-task2 \
  example/split-task3 \
  example/split-task4

# Build/run baseline Ubuntu-minimal OCI bundle with runc
./target/release/batch ../tasks/runc

# Build/run Ubuntu-minimal OCI with runc + llama.cpp + mounted GGUF model,
# then export a replay bundle
./target/release/batch ../tasks/runc-llama
```

The `../tasks/runc-llama` workflow writes to `../tasks/runc-llama/output/`:
- `llama-answer.txt` (prompt output)
- `runc-llama-replay.tar.xz`
  (single replay bundle with `rootfs.tar.xz`, `config.template.json`, model, and `run-from-output.sh`)

### runc-llama local model flow

`../tasks/runc-llama/scripts/10_build_bundle.sh` resolves the model source in this order:
1. `LOCAL_MODEL_PATH` (if set and the file exists inside the VM)
2. First `*.gguf` under `../tasks/runc-llama/scripts/models/`
3. Download from `LLAMA_MODEL_URL` or built-in fallback URLs

Example (recommended to avoid network flakiness):

```bash
mkdir -p ../tasks/runc-llama/scripts/models
curl -L -C - \
  -o ../tasks/runc-llama/scripts/models/qwen2.5-0.5b-instruct-q2_k.gguf \
  https://huggingface.co/Qwen/Qwen2.5-0.5B-Instruct-GGUF/resolve/main/qwen2.5-0.5b-instruct-q2_k.gguf

./target/release/batch ../tasks/runc-llama
```

## Task Directory Structure

A task is a self-contained directory:

```
<task-dir>/
├── task.json     # name, description, disk_size
├── scripts/     # Shell scripts executed alphabetically (00_, 10_, ...)
└── output/      # Results collected here after VM run
```

### task.json Fields

```json
{
  "name": "my-batch",
  "description": "Build and test inside an ephemeral VM",
  "disk_size": "20G"
}
```

| Field | Required | Description |
|-------|----------|-------------|
| `name` | no | Human-readable label |
| `description` | no | What this task does (shown in logs) |
| `disk_size` | no | VM disk size override |

## How It Works

1. Spins up an ephemeral VM via the [vm](https://github.com/vigilo-project/vm) crate
2. Copies input scripts into the VM
3. Executes each script alphabetically, streaming output
4. Copies `/tmp/batch/out` from the VM back to the host output directory
5. Destroys the VM

Output collection and VM cleanup always run, even if a script fails.

## Testing

QEMU and SSH must be installed (see `vm` crate's `deps.sh`).

```bash
cargo test                     # All tests (requires QEMU)
cargo test --lib --bin batch  # Unit tests only (no VM needed)

# Ollama integration test (opt-in)
BATCH_OLLAMA_IT=1 cargo test run_in_out_with_ollama_prompt_collects_answer --test integration -- --nocapture
```
