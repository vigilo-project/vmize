# Repository Guidelines

## Product Context
`vm` is VMize's infrastructure layer for provisioning, booting, and connecting to ephemeral VMs used by higher-level task runners.

## Project Structure & Module Organization
`vm` is a Rust CLI for creating and managing Ubuntu cloud-image VMs.

- `src/main.rs`: CLI entrypoint (`run`, `ssh`, `ps`, `rm`).
- `src/platform.rs`, `src/config.rs`: host detection and runtime config.
- `src/image/`, `src/cloud_init/`, `src/qemu/`, `src/ssh/`: image download, seed ISO generation, QEMU launch, and SSH helpers.
- `tests/integration_test.rs`: end-to-end VM lifecycle test.
- `AGENTS.md`: contributor guidance.
- `deps.sh`: host dependency + Ubuntu image bootstrap.
- `vms/`: runtime artifacts (`images/`, `instances/`, `keys/`).

## Build, Test, and Development Commands
- `./deps.sh`: install platform dependencies and cache the Ubuntu 24.04 image.
- `cargo build --release`: build optimized binary.
- `cargo run --release -- run --ssh-port 2222`: create/start a VM and print VM ID.
- `cargo run --release -- ssh <vm-id> "uname -a"`: execute a command over SSH.
- `cargo run --release -- stop <vm-id>`: stop a managed VM.
- `cargo run --release -- clear`: remove all managed VM records and keys.
- `cargo run --release -- list`: list managed VMs.
- `cargo test`: run all tests.

## Coding Style & Naming Conventions
- Follow standard Rust formatting: 4-space indentation and `rustfmt` defaults.
- Run `cargo fmt` before submitting changes.
- Prefer `snake_case` for functions/modules, `PascalCase` for types, and keep public names aligned to responsibilities.
- Keep platform-specific behavior inside `platform`/`qemu` configuration boundaries.

## Testing Guidelines
- Primary framework: Rust test harness with async tests via `#[tokio::test]`.
- Integration tests should validate VM lifecycle using ID-based commands and clean up by `stop <vm-id>`.
- Name tests by behavior (example: `test_vm_run_ssh_apt`).
- Add coverage for record lookup/lookup-failure paths when lifecycle API evolves.

## Commit & Pull Request Guidelines
- Current history uses concise summary prefixes.
- Keep commit subjects imperative and specific.
- Use Conventional Commits style for consistency (no extra spaces around scope/type separators).
- Preferred commit types:
  - `feat`: new feature
  - `fix`: bug fix
  - `refactor`: restructuring without behavior change
  - `docs`: documentation updates
  - `test`: test additions/maintenance
  - `chore`: maintenance tasks (cleanup, formatting, dependency/tooling updates)
- Recommended format: `type(scope): summary` (for example `feat(qemu): add vnc support`)
- PRs should include:
  - Clear problem/solution summary.
  - Commands run locally (for example: `cargo fmt`, `cargo test`).
  - Linked issue when applicable and relevant runtime logs for VM/QEMU failures.

## Security & Configuration Tips
- Never commit `vms/` artifacts, private keys, or machine-specific runtime files.
- Use non-default SSH ports in local parallel runs to avoid collisions (for example `4445`, `4450`).
