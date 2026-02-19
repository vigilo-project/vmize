# VMize

> Turn tasks into VMs.
>
> Give every script its own machine.

VMize batches, isolates, and executes workloads inside ephemeral virtual machines so the host stays clean.

## Workspace Components

### [`vm`](./vm)

VM lifecycle runtime: create, connect, copy files, list, and remove Ubuntu cloud-image VMs on QEMU.

### [`task`](./task)

Task definition crate for parsing `task.json` and validating task directories.

### [`worker`](./worker)

Task runner on top of `vm`.
A **task** is a directory with `task.json`, `input/`, and `output/`.

### [`dashboard`](./dashboard)

Web control plane for queuing and running `worker` tasks with live progress.

### [`cli` (`vmize`)](./cli)

Workspace CLI that exposes `task` and `dashboard` commands.

### [`tasks`](./tasks)

Shared task directories (for example `runc`, `runc-llama`, `ollama`).

## Dependency Chain

`vmize -> (dashboard | worker) -> vm -> QEMU/KVM (Linux) or HVF (macOS)`

## Minimum Goals At A Glance

- `vm`: run a VM, execute commands with `ssh`, transfer files with `cp`, and clean up with `rm`.
- `worker`: run task directories in isolated VMs, collect outputs, and enforce the `--batch` max queue size.
- `dashboard`: queue tasks, run queued tasks in parallel, stream live state/log updates, and preserve state on reconnect.

## Quick Start: vm

```bash
cd vm && ./deps.sh
cargo build --release
./target/release/vm run
```

## Quick Start: worker

```bash
cargo build --release
./target/release/vmize task worker/example/task1
./target/release/vmize task tasks/runc-llama
```

## Quick Start: dashboard

```bash
cargo build --release
./target/release/vmize dashboard --port 8080
```

## Verification

```bash
# Module gates
cargo test -p vm
cargo test -p task
cargo test -p worker
cargo test -p dashboard

# Optional extended paths
DASHBOARD_IT=1 cargo test --test api run_api_run_task_succeeds -p dashboard -- --nocapture
BATCH_OLLAMA_IT=1 cargo test run_task_ollama_prompt_collects_answer --test integration -p worker -- --nocapture

# Optional browser E2E (separate from `cargo test`)
(cd dashboard && npm install && npx playwright install chromium && npm run e2e)

# Full workspace
cargo test
```

## Structure

```text
vmize/
├── Cargo.toml
├── vm/
├── task/
├── worker/
├── dashboard/
├── cli/
└── tasks/
```
