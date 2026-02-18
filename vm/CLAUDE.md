# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project

VM — VMize's Rust CLI for automating Ubuntu Cloud Image VM creation and management on QEMU/KVM. Supports Linux x86_64 (KVM) and macOS arm64 (HVF).

## Build & Test Commands

```bash
cargo build                    # Debug build
cargo build --release          # Release build
cargo test                     # All tests (unit + integration)
cargo test <test_name>         # Single test
cargo clippy                   # Lint
cargo fmt                      # Format
./deps.sh                      # Install QEMU/dependencies and download Ubuntu image
```

The integration test (`tests/integration_test.rs`) creates a real VM with SSH, runs `apt-get install`, and takes 1-2 minutes. It uses `cargo run --release` internally, so a release build is required.

## Architecture

Five CLI subcommands in `src/main.rs` via clap derive: **run**, **ssh**, **ps**, **rm**, **cp**.

### VM lifecycle

VMs use sequential IDs (`vm0`, `vm1`, ...) derived from the highest existing ID in `~/.local/share/vm/instances/`. Each VM's state is persisted as `~/.local/share/vm/instances/<vm-id>/vm.json` (`VmRecord` struct).

Runtime status is determined by checking the recorded PID: `Running` (process alive), `Stopped` (explicit stop or no PID), or `Stale` (PID dead but record says running — hidden from `list`).

### The `run` pipeline (8 steps)

1. `platform::HostProfile::detect()` — selects QEMU binary, machine type, CPU, and image URL based on OS/arch
2. `Config::default()` — sets up `~/.local/share/vm/` directory layout
3. `image::ImageDownloader` — async streaming download with progress bar, skips if image cached
4. `ssh::SshKeyManager` — generates ED25519 key pairs per hostname, skips if key exists
5. `cloud_init::CloudInitSeed` + `IsoCreator` — writes cloud-init YAML, creates NOCLOUD seed ISO (tries genisoimage → xorriso → mkisofs)
6. `qemu-img create -b` — creates qcow2 backing file (falls back to `cp`)
7. `qemu::QemuConfig` (fluent builder) → `QemuRunner::start()` — spawns QEMU, reads PID from pidfile
8. SSH port probe + `SshClient::connect_with_retry()` — verifies VM is reachable, runs `hostname` check

### SSH port reservation

`reserve_ssh_port()` uses filesystem locks (`~/.local/share/vm/instances/.ssh-port-locks/<port>.lock`) with PID-based stale detection. If the requested port is occupied, it auto-increments. The lock is cleaned up via `Drop` on `SshPortReservation`. If QEMU fails to stay alive due to a port conflict, the run loop retries with the next available port (up to 20 attempts).

### Module layout

| Module | Key types | Purpose |
|--------|-----------|---------|
| `platform` | `HostProfile` | Runtime OS/arch detection, platform-specific QEMU defaults |
| `config` | `Config` | Base directory paths (`~/.local/share/vm/`), default memory/CPU |
| `image` | `ImageDownloader` | Async HTTP download with indicatif progress |
| `cloud_init` | `CloudInitSeed`, `IsoCreator` | Cloud-init metadata/userdata generation + ISO creation |
| `qemu` | `QemuConfig`, `QemuRunner` | QEMU arg building (fluent builder) + process lifecycle |
| `ssh` | `SshKeyManager`, `SshClient` | Key generation + async SSH via openssh with native mux |

### Runtime directories

```
~/.local/share/vm/
├── images/      # Cached Ubuntu cloud images
├── instances/   # Per-VM dirs (vm.json, disk.qcow2, seed.iso, meta-data, user-data)
│   └── .ssh-port-locks/  # Filesystem-based port reservation locks
└── keys/        # SSH key pairs ({hostname}.key, {hostname}.key.pub)
```

## Patterns & Conventions

- **Error handling**: `anyhow::Result<T>` everywhere, `context()` for wrapping, `bail!()` for early returns
- **Async**: tokio runtime for I/O (downloads, SSH); QEMU process spawning is sync (`std::process::Command`)
- **Builder pattern**: `QemuConfig` uses a hand-rolled fluent API (not `typed-builder` despite the dependency)
- **Idempotency**: image downloads and SSH key generation skip if artifacts already exist
- **Logging**: `tracing` crate; level controlled via `RUST_LOG` env var (default: info)
- **Process checks**: `is_process_alive()` combines `kill -0` with `ps` zombie detection
- **Key sharing**: `remove` and `clear` track which SSH keys are shared across VMs to avoid deleting keys still in use
