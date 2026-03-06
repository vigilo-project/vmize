# Repository Guidelines

## Product Context
VMize turns tasks into VMs. Each workload runs in an ephemeral machine so host state stays reproducible.

## Project Structure
This repository is a Cargo workspace with five crates and shared task examples in the worker crate:
- `vm/`: VM lifecycle CLI using Ubuntu cloud images + QEMU.
- `task/`: task definition loading (`task.json`) and directory validation.
- `worker/`: task-directory runner built on top of `vm`.
- `dashboard/`: web UI/API library for queueing and running `worker` tasks.
- `cli/`: workspace CLI (`vmize`) for running tasks and starting the dashboard.
- `worker/example/`: shared task fixtures (`task.json`, `input/`, `output/`).

Key paths:
- `vm/src/`, `task/src/`, `worker/src/`, `dashboard/src/`, `cli/src/`
- `vm/tests/`, `worker/tests/`, `dashboard/tests/`
- `worker/example/`

## Workspace Release Gate
A change is release-ready only when module minimum goals are still true:
- `vm`: lifecycle path works (`run -> ssh/cp -> ps -> rm`).
- `worker`: task execution and output collection work; batch mode limit is enforced.
- `dashboard`: queue/add/remove/run, SSE progress, and reconnect replay all work.

Required verification commands:
- `cargo test -p vm`
- `cargo test -p task`
- `cargo test -p worker`
- `cargo test -p dashboard`

Optional extended checks:
- `DASHBOARD_IT=1 cargo test --test api run_api_run_task_succeeds -p dashboard -- --nocapture`
- `BATCH_OLLAMA_IT=1 cargo test run_task_ollama_prompt_collects_answer --test integration -p worker -- --nocapture`

## Dashboard MVP Priority
For any change in `dashboard/` (UI, API, task execution flow, SSE), treat `dashboard/AGENTS.md` as the release gate.

## Build, Test, and Development Commands
- `cargo build --release`
- `cargo test`
- `cargo test -p worker --lib`
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

## Execution Policy (Fail Closed)
- For workflows that require host directory mounts, execution must be **fail-closed**.
- If mount setup is unavailable (for example missing `virtiofsd`), stop immediately and report failure.
- Do **not** use copy/sync-based workarounds (such as `scp`, `rsync`, archive transfer) to bypass mount failures.
