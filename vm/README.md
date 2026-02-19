# VM

`vm` is VMize's VM lifecycle runtime.
It is a Rust CLI for creating and managing Ubuntu cloud-image VMs on QEMU.

Supported hosts:
- Linux x86_64 (KVM)
- macOS arm64 (HVF)

## Minimum Goal
The minimum lifecycle goal is:
1. Create VM (`run`).
2. Connect/execute command (`ssh`).
3. Transfer files (`cp`) in both directions.
4. Inspect managed VMs (`ps`).
5. Clean up one/all VMs (`rm`, `rm --all`).

## Acceptance Checklist
- `run` prints a usable VM ID hint (`vm ssh <id>`).
- `ssh` works for both interactive and command modes.
- `cp` works local->VM and VM->local.
- `rm` removes the VM and related managed metadata.

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

`./deps.sh` installs dependencies and downloads Ubuntu 24.04 minimal image to `~/.local/share/vm/images`.

## Core Commands

```bash
# Create VM (auto-selects free SSH port from 2222)
./target/release/vm run

# SSH command execution
./target/release/vm ssh <vm-id> "uname -a"

# Copy files (scp-style remote path)
./target/release/vm cp ./file.txt <vm-id>:/tmp/file.txt
./target/release/vm cp <vm-id>:/tmp/file.txt ./file.txt

# List and remove
./target/release/vm ps
./target/release/vm rm <vm-id>
./target/release/vm rm --all
```

Common `run` options:
- `--memory <size>`
- `--cpus <n>`
- `--disk-size <size>`
- `--image-url <url>`
- `--verbose`

## Runtime Paths

`vm` stores managed state under `~/.local/share/vm/`:

```text
images/     cached Ubuntu images
instances/  per-VM data (disk, seed ISO, vm.json)
keys/       SSH key pairs
```

## Verification Commands

```bash
cargo test -p vm
cargo test -p vm --test integration_test test_vm_run_ssh_apt -- --nocapture
```
