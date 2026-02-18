# VM

`vm` is VMize's VM lifecycle runtime.
It is a Rust CLI for creating and managing Ubuntu cloud-image VMs on QEMU.

Supported hosts:
- Linux x86_64 (KVM)
- macOS arm64 (HVF)

## Quick Start

```bash
./deps.sh
cargo build --release
./target/release/vm run
./target/release/vm ssh <vm-id>
./target/release/vm cp ./local.txt <vm-id>:/tmp/local.txt
./target/release/vm rm <vm-id>
```

## Prerequisites

- Rust 1.80+
- QEMU
- One ISO tool: `genisoimage`, `xorriso`, or `mkisofs`
- OpenSSH client and `ssh-keygen`

`./deps.sh` installs platform dependencies and downloads the Ubuntu 24.04 minimal image to `~/.local/share/vm/images`.

## Core Commands

```bash
# Create a VM (auto-selects a free SSH port starting from 2222)
./target/release/vm run

# Run a command over SSH
./target/release/vm ssh <vm-id> "uname -a"

# Copy files (scp-style remote path)
./target/release/vm cp ./file.txt <vm-id>:/tmp/file.txt
./target/release/vm cp <vm-id>:/tmp/file.txt ./file.txt

# List running VMs
./target/release/vm ps

# Remove one VM or everything managed by vm
./target/release/vm rm <vm-id>
./target/release/vm rm --all
```

Common `run` options:
- `--ssh-port <port>`
- `--memory <size>`
- `--cpus <n>`
- `--disk-size <size>`
- `--image-url <url>`
- `--verbose`

## Runtime Paths

`vm` stores state under `~/.local/share/vm/`:

```text
images/     cached Ubuntu images
instances/  per-VM data (disk, seed ISO, vm.json)
keys/       SSH key pairs
```
