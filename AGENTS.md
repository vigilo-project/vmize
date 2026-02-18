# Repository Guidelines

## Product Context
VMize turns tasks into VMs. Each workload runs inside an ephemeral machine so host state stays clean and reproducible.

## Project Structure & Module Organization
This repository is a Cargo workspace (`Cargo.toml`) with three crates:
- `vm/`: CLI for creating/managing Ubuntu cloud-image VMs via QEMU.
- `batch/`: runs job directories inside ephemeral VMs.
- `dashboard/`: axum-based web UI for running `batch` jobs.
- `jobs/`: shared job directories (`job.json`, `scripts/`, `output/`) used by `batch`.

Key paths:
- `vm/src/`, `batch/src/`, `dashboard/src/`: crate source code.
- `vm/tests/`, `batch/tests/`, `dashboard/tests/`: integration tests.
- `batch/example/` and `jobs/`: job fixtures and workflows.

Keep generated build artifacts out of review scope (for example `target/`).

## Dashboard MVP Priority
For any change in `dashboard/` (UI, API, job execution flow, SSE), treat `dashboard/AGENTS.md` as the release gate. A dashboard PR is not complete unless it satisfies the full MVP criteria there (browser UX + API behavior + SSE reconnect replay).

## Build, Test, and Development Commands
- `cargo build --release`: builds all crates.
- `cargo test`: runs all tests, including VM-dependent ones.
- `cargo test --lib --bin batch -p batch`: fast `batch` tests (no VM boot).
- `cargo test --test api -p dashboard`: dashboard HTTP/API integration tests (no QEMU).
- `cargo test --lib -p dashboard`: dashboard unit tests only.
- `DASHBOARD_IT=1 cargo test --test api run_api_run_job_succeeds -p dashboard -- --nocapture`: dashboard end-to-end VM test.
- `BATCH_OLLAMA_IT=1 cargo test run_in_out_with_ollama_prompt_collects_answer --test integration -p batch -- --nocapture`: opt-in Ollama integration.
- `(cd vm && ./deps.sh)`: install host deps and download Ubuntu image cache.

## Coding Style & Naming Conventions
Use Rust 2021 defaults and keep code `rustfmt`-formatted. Run `cargo fmt` and `cargo clippy` before opening a PR. Follow standard naming:
- `snake_case` for functions/modules/files.
- `PascalCase` for structs/enums/traits.
- `SCREAMING_SNAKE_CASE` for constants.

## Testing Guidelines
Primary framework is Rust’s built-in test harness (`#[test]`, `#[tokio::test]`). Add unit tests near logic changes and integration tests for CLI/API changes. Name tests by behavior (`run_*`, `test_vm_*`).

When dashboard behavior changes, run `cargo test --test api -p dashboard` at minimum, and run the `DASHBOARD_IT=1` case for run-path changes.

## Commit & Pull Request Guidelines
Use concise Conventional Commit subjects such as `feat: ...`, `fix(ci): ...`, `docs: ...`, `refactor: ...`. Keep each commit focused.

PRs should include:
- What changed and why.
- Commands run locally (for example `cargo fmt`, `cargo clippy`, relevant `cargo test ...`).
- Linked issue (if applicable) and relevant logs/screenshots.
