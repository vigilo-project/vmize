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

Direct kernel boot (raw/qcow2 rootfs):
- `--kernel <path_to_bzImage>`
- `--rootfs <path_to_disk_image>`

For direct boot, `--kernel` and `--rootfs` must be used together, and `--image-url`/`--disk-size` are not used.

Recommended image layout for custom kernel flow:

```bash
mkdir -p ~/.local/share/vm/images/custom
```

Example:

```bash
# kernel: built from https://github.com/torvalds/linux
cargo run -p vm -- run \
  --kernel ~/.local/share/vm/images/custom/bzImage \
  --rootfs ~/.local/share/vm/images/custom/rootfs.qcow2 \
  --verbose
```

Note:
- `worker/example/runc-llama` produces OCI-style `rootfs` output (`rootfs/` and `rootfs/rootfs.tar`), which is **not** a QEMU disk image.
  For `--rootfs`, provide a disk image (`.img`/`.qcow2`/`raw`) that QEMU can attach directly.

### Build Kernel/Rootfs With Worker Tasks

```bash
cd /Users/sangwan/dev/vmize
mkdir -p image

# 1) build kernel from torvalds/linux
cargo run -p vmize --release -- task worker/example/kernel-build
cp worker/example/kernel-build/output/kernel image/bzImage

# 2) build rootfs.qcow2 from Ubuntu minimal cloud rootfs tarball
cargo run -p vmize --release -- task worker/example/rootfs-build
cp worker/example/rootfs-build/output/rootfs.qcow2 image/rootfs.qcow2

# 3) run custom boot
cargo run -p vm --release -- run \
  --kernel /Users/sangwan/dev/vmize/image/bzImage \
  --rootfs /Users/sangwan/dev/vmize/image/rootfs.qcow2
```

`ubuntu-24.04-minimal-cloudimg-*-root.tar.xz` is the source tarball, not the final disk image.
The final rootfs for `vm run --rootfs` is `rootfs.qcow2` (or raw disk image).

### Custom VM Build Workspace Script

Use `vm/scripts/setup_custom_kernel_boot.sh` to create a reusable workspace under
`~/.local/share/vm/images/custom`.

It can:
- clone Linux and build a bootable kernel image
- download Ubuntu minimal rootfs (`.tar.xz`) and convert it to raw/qcow2
- print the exact `vm run` command for your generated paths

Quick path:

```bash
cd /Users/sangwan/dev/vmize/vm
./scripts/setup_custom_kernel_boot.sh \
  --rootfs-size 20G
```

Then run the printed command (example path shown):

```bash
# arm64 host example (x86_64 uses arch/x86/boot/bzImage)
cargo run -p vm -- run \
  --kernel ~/.local/share/vm/images/custom/linux/arch/arm64/boot/Image \
  --rootfs ~/.local/share/vm/images/custom/rootfs.qcow2 \
  --verbose
```

Converting `runc-llama` rootfs artifacts:

```bash
cd /Users/sangwan/dev/vmize/vm
./scripts/setup_custom_kernel_boot.sh \
  --rootfs-source /Users/sangwan/dev/vmize/worker/example/runc-llama-build/output/rootfs/rootfs.tar \
  --rootfs-size 30G
```

Use the printed `--kernel` and `--rootfs` values with `vm run`.

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
