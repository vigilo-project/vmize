# Repository Guidelines

## Product Context
VMize turns tasks into VMs. Each workload runs in an ephemeral machine so host state stays reproducible.

## Project Structure
This repository is a Cargo workspace with four crates and one shared tasks directory:
- `vm/`: VM lifecycle CLI using Ubuntu cloud images + QEMU.
- `batch/`: task-directory runner built on top of `vm`.
- `dashboard/`: web UI/API library for queueing and running `batch` tasks.
- `cli/`: workspace CLI (`vmize`) for running tasks and starting the dashboard.
- `tasks/`: shared task fixtures (`task.json`, `input/`, `output/`).

Key paths:
- `vm/src/`, `batch/src/`, `dashboard/src/`, `cli/src/`
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
- `BATCH_OLLAMA_IT=1 cargo test run_task_ollama_prompt_collects_answer --test integration -p batch -- --nocapture`

## Dashboard MVP Priority
For any change in `dashboard/` (UI, API, task execution flow, SSE), treat `dashboard/AGENTS.md` as the release gate.

## Build, Test, and Development Commands
- `cargo build --release`
- `cargo test`
- `cargo test -p batch --lib`
- `cargo test --test api -p dashboard`
- `cargo run -p vmize -- --help`
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
