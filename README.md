# vm-lab

Cargo workspace for VM automation tooling.

## Crates

### [`vm`](./vm)

A CLI tool for creating and managing Ubuntu Cloud Image VMs via QEMU.
Handles the full VM lifecycle: image download, cloud-init setup, QEMU process management, and SSH access.

### [`vm-batch`](./vm-batch)

A CLI tool that runs jobs inside ephemeral VMs, built on top of `vm`.
A **job** is a named bundle of shell scripts declared in a JSON file, executed sequentially inside a fresh VM, with output collected back to the host.

## Dependency chain

```
vm-batch → vm → QEMU/KVM (Linux) / HVF (macOS)
```

## Quick start: vm

```bash
cd vm && ./deps.sh          # Install dependencies and download Ubuntu image
cargo build --release
./vm/target/release/vm run
```

## Quick start: vm-batch

```bash
cargo build --release
./vm-batch/target/release/vm-batch example/job1.json  # Spins up a VM automatically
```

## Structure

```
vm-lab/
├── Cargo.toml     # workspace root
├── vm/            # submodule — vigilo-project/vm
└── vm-batch/      # submodule — vigilo-project/vm-batch
```

> `vm` and `vm-batch` are independent git repositories managed as git submodules.
