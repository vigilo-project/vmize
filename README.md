# VMize

> Turn tasks into VMs.
>
> Give every script its own machine.

VMize batches, isolates, and executes workloads inside ephemeral virtual machines so the host stays clean.

## Crates

### [`vm`](./vm)

VM lifecycle runtime: create, connect, copy files, list, and remove Ubuntu cloud-image VMs on QEMU.

### [`batch`](./batch)

Task runner on top of `vm`.
A **task** is a directory with `task.json`, `scripts/`, and `output/`.

### [`dashboard`](./dashboard)

Web control plane for queuing and running `batch` tasks with live progress.

### [`tasks`](./tasks)

Shared task directories (for example `runc`, `runc-llama`, `ollama`).

## Dependency Chain

`dashboard -> batch -> vm -> QEMU/KVM (Linux) or HVF (macOS)`

## Minimum Goals At A Glance

- `vm`: run a VM, execute commands with `ssh`, transfer files with `cp`, and clean up with `rm`.
- `batch`: run task directories in isolated VMs, collect outputs, and enforce the `--concurrent` max queue size.
- `dashboard`: queue tasks, run queued tasks in parallel, stream live state/log updates, and preserve state on reconnect.

## Quick Start: vm

```bash
cd vm && ./deps.sh
cargo build --release
./target/release/vm run
```

## Quick Start: batch

```bash
cargo build --release
./target/release/batch batch/example/task1
./target/release/batch tasks/runc-llama
```

## Quick Start: dashboard

```bash
cargo build --release
./target/release/dashboard --port 8080
```

## Verification

```bash
# Module gates
cargo test -p vm
cargo test -p batch
cargo test -p dashboard

# Optional extended paths
DASHBOARD_IT=1 cargo test --test api run_api_run_task_succeeds -p dashboard -- --nocapture
BATCH_OLLAMA_IT=1 cargo test run_task_with_options_ollama_prompt_collects_answer --test integration -p batch -- --nocapture

# Full workspace
cargo test
```

## Structure

```text
vmize/
├── Cargo.toml
├── vm/
├── batch/
├── dashboard/
└── tasks/
```
