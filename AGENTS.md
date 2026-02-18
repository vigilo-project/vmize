# Repository Guidelines

## Product Context
VMize turns tasks into VMs. Each workload runs in an ephemeral machine so host state stays reproducible.

## Project Structure
This repository is a Cargo workspace with three crates:
- `vm/`: VM lifecycle CLI using Ubuntu cloud images + QEMU.
- `batch/`: task-directory runner built on top of `vm`.
- `dashboard/`: web UI/API for queueing and running `batch` tasks.
- `tasks/`: shared task fixtures (`task.json`, `scripts/`, `output/`).

Key paths:
- `vm/src/`, `batch/src/`, `dashboard/src/`
- `vm/tests/`, `batch/tests/`, `dashboard/tests/`
- `batch/example/`, `tasks/`

## Workspace Release Gate
A change is release-ready only when module minimum goals are still true:
- `vm`: lifecycle path works (`run -> ssh/cp -> ps -> rm`).
- `batch`: task execution and output collection work; concurrency limit is enforced.
- `dashboard`: queue/add/remove/run, SSE progress, and reconnect replay all work.

Required verification commands:
- `cargo test -p vm`
- `cargo test -p batch`
- `cargo test -p dashboard`

Optional extended checks:
- `DASHBOARD_IT=1 cargo test --test api run_api_run_task_succeeds -p dashboard -- --nocapture`
- `BATCH_OLLAMA_IT=1 cargo test run_in_out_with_ollama_prompt_collects_answer --test integration -p batch -- --nocapture`

## Dashboard MVP Priority
For any change in `dashboard/` (UI, API, task execution flow, SSE), treat `dashboard/AGENTS.md` as the release gate.

## Build, Test, and Development Commands
- `cargo build --release`
- `cargo test`
- `cargo test --lib --bin batch -p batch`
- `cargo test --test api -p dashboard`
- `(cd vm && ./deps.sh)`

## Coding Style
Use Rust 2021 defaults and keep code `rustfmt`-formatted.
- `snake_case` for functions/modules/files
- `PascalCase` for types
- `SCREAMING_SNAKE_CASE` for constants

## PR Expectations
- Focused scope
- Clear reason for change
- Exact local commands run
- Relevant logs/screenshots for behavior changes
