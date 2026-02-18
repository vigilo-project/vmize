# Repository Guidelines

## Project Structure & Module Organization
This repository is a Cargo workspace (`Cargo.toml`) with three crates:
- `vm/` (git submodule): CLI for creating/managing Ubuntu cloud-image VMs via QEMU.
- `vm-batch/` (git submodule): runs job directories inside ephemeral VMs.
- `vm-dashboard/`: axum-based web UI for running `vm-batch` jobs.

Key paths:
- `vm/src/`, `vm-batch/src/`, `vm-dashboard/src/`: crate source code.
- `vm/tests/`, `vm-batch/tests/`, `vm-dashboard/tests/`: integration tests.
- `vm-batch/example/` and `vm-batch/jobs/`: job fixtures.

Keep generated build artifacts out of review scope (for example `target/`).

## Dashboard MVP Priority
For any change in `vm-dashboard/` (UI, API, job execution flow, SSE), treat `vm-dashboard/AGENTS.md` as the release gate. A dashboard PR is not complete unless it satisfies the full MVP criteria there (browser UX + API behavior + SSE reconnect replay).

## Build, Test, and Development Commands
- `cargo build --release`: builds all crates.
- `cargo test`: runs all tests, including VM-dependent ones.
- `cargo test --lib --bin vm-batch -p vm-batch`: fast `vm-batch` tests (no VM boot).
- `cargo test --test api -p vm-dashboard`: dashboard HTTP/API integration tests (no QEMU).
- `cargo test --lib -p vm-dashboard`: dashboard unit tests only.
- `VM_DASHBOARD_IT=1 cargo test --test api run_api_run_job_succeeds -p vm-dashboard -- --nocapture`: dashboard end-to-end VM test.
- `VM_BATCH_OLLAMA_IT=1 cargo test run_in_out_with_ollama_prompt_collects_answer --test integration -p vm-batch -- --nocapture`: opt-in Ollama integration.
- `(cd vm && ./deps.sh)`: install host deps and download Ubuntu image cache.

## Coding Style & Naming Conventions
Use Rust 2021 defaults and keep code `rustfmt`-formatted. Run `cargo fmt` and `cargo clippy` before opening a PR. Follow standard naming:
- `snake_case` for functions/modules/files.
- `PascalCase` for structs/enums/traits.
- `SCREAMING_SNAKE_CASE` for constants.

## Testing Guidelines
Primary framework is Rust’s built-in test harness (`#[test]`, `#[tokio::test]`). Add unit tests near logic changes and integration tests for CLI/API changes. Name tests by behavior (`run_*`, `test_vm_*`).

When dashboard behavior changes, run `cargo test --test api -p vm-dashboard` at minimum, and run the `VM_DASHBOARD_IT=1` case for run-path changes.

## Commit & Pull Request Guidelines
Use concise Conventional Commit subjects such as `feat: ...`, `fix(ci): ...`, `docs: ...`, `refactor: ...`. Keep each commit focused.

PRs should include:
- What changed and why.
- Commands run locally (for example `cargo fmt`, `cargo clippy`, relevant `cargo test ...`).
- Linked issue (if applicable) and relevant logs/screenshots.

If you modify `vm/` or `vm-batch/`, commit inside the submodule first, then commit the updated submodule pointer in this workspace.
