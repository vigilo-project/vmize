# Repository Guidelines

## Product Context
`vm` is VMize's infrastructure runtime for provisioning, booting, and connecting to ephemeral Ubuntu VMs.

## Project Structure
- `src/main.rs`: CLI entrypoint (`run`, `ssh`, `ps`, `rm`, `cp`, `version`)
- `src/platform.rs`, `src/config.rs`: host detection and runtime config
- `src/image/`, `src/cloud_init/`, `src/qemu/`, `src/ssh/`: core VM lifecycle internals
- `tests/integration_test.rs`: lifecycle integration test
- `deps.sh`: host dependency + Ubuntu image bootstrap

Runtime artifacts are stored under `~/.local/share/vm/` (`images/`, `instances/`, `keys/`).

## Minimum Goal (MVP)
A `vm` change is acceptable only if all of these remain true:
1. `run` creates a reachable VM and returns a usable VM ID.
2. `ssh` executes commands on a managed VM.
3. `cp` transfers files both local->VM and VM->local.
4. `ps` lists managed VMs.
5. `rm <id>` and `rm --all` clean up managed resources.

## Acceptance Checklist
- VM ID lifecycle remains stable (`run -> ssh/cp -> rm`).
- CLI subcommands and options match `src/main.rs`.
- Default runtime path remains `~/.local/share/vm` unless intentionally changed.

## Verification Commands
```bash
./deps.sh
cargo build --release -p vm
cargo test -p vm

# Focused lifecycle integration path
cargo test -p vm --test integration_test test_vm_run_ssh_apt -- --nocapture
```

## Development Commands
- `cargo run --release -p vm -- run --ssh-port 2222`
- `cargo run --release -p vm -- ssh <vm-id> "uname -a"`
- `cargo run --release -p vm -- cp ./file.txt <vm-id>:/tmp/file.txt`
- `cargo run --release -p vm -- ps`
- `cargo run --release -p vm -- rm <vm-id>`
- `cargo run --release -p vm -- rm --all`

## Conventions
- Use `rustfmt` defaults
- Keep platform-specific behavior inside `platform`/`qemu`
- Prefer explicit errors and behavior-driven tests
