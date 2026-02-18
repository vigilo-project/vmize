# VMize

> Turn tasks into VMs.
>
> Give every script its own machine.

VMize batches, isolates, and executes your workloads inside ephemeral virtual machines, leaving your host untouched.

## Crates

### [`vm`](./vm)

A CLI tool for creating and managing Ubuntu Cloud Image VMs via QEMU.
Handles the full VM lifecycle: image download, cloud-init setup, QEMU process management, and SSH access.

### [`batch`](./batch)

A CLI task runner built on top of `vm`.
A **task** is a named bundle of shell scripts declared in JSON, executed inside a fresh VM, with outputs copied back to the host.

### [`dashboard`](./dashboard)

A web control plane for running and observing `batch` tasks.
It provides queueing, live progress, and API-driven orchestration without requiring a TTY.

### [`tasks`](./tasks)

Shared task directories consumed by `batch` (for example `runc`, `runc-llama`, `ollama`).

## Dependency chain

`dashboard → batch → vm → QEMU/KVM (Linux) / HVF (macOS)`

## Quick start: vm

```bash
cd vm && ./deps.sh          # Install dependencies and download Ubuntu image
cargo build --release
./vm/target/release/vm run
```

## Quick start: batch

```bash
cargo build --release
./target/release/batch batch/example/task1  # Run one example task
./target/release/batch tasks/runc-llama     # Run a shared runc-llama task
```

## Quick start: dashboard

```bash
cargo build --release
./target/release/dashboard --port 8080
```

## Structure

```
vmize/
├── Cargo.toml  # workspace root
├── vm/         # workspace crate
├── batch/      # workspace crate
├── dashboard/  # workspace crate
└── tasks/       # shared task directories
```
