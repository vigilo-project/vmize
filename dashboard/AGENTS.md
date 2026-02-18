# Repository Guidelines

## Product Context
`dashboard` is VMize's browser interface for orchestrating and observing task execution across isolated VMs.

## Project Structure & Module Organization
`dashboard` is a single-binary Rust crate that provides a browser UI for `batch`.

- `src/main.rs`: CLI entrypoint, HTTP routes, shared state, worker threading, SSE broadcast.
- `src/dashboard.html`: embedded frontend served at `GET /`.
- `tests/api.rs`: integration tests for HTTP/API behavior and optional VM end-to-end path.

Keep behavior changes in `src/main.rs` and UI adjustments in `src/dashboard.html` clearly separated in commits.

## Minimum Goal (MVP) — Required Release Gate
Any dashboard change is complete only when all of these are true:

1. `dashboard --port 8080` starts successfully.
2. Valid task path can be added; queue shows `name`/`description` from `task.json`.
3. Invalid path is rejected with a clear error and no queue mutation.
4. A 5th queued task is rejected (queue max is 4).
5. Queued tasks can be removed before run.
6. `Run All` starts queued tasks in parallel and prevents duplicate starts.
7. Live card updates show phase, script progress (`N/M`), recent logs, elapsed time.
8. Completion state is explicit: success includes output path; failure includes error.
9. Browser refresh restores state via `/api/status` and replays recent SSE events.

## Build, Test, and Development Commands
- `cargo build --release -p dashboard`: build release binary.
- `cargo test --lib -p dashboard`: unit tests only.
- `cargo test --test api -p dashboard`: HTTP/API integration tests (no QEMU).
- `DASHBOARD_IT=1 cargo test --test api run_api_run_task_succeeds -p dashboard -- --nocapture`: VM-required end-to-end run path.
- `cargo fmt -p dashboard && cargo clippy -p dashboard --all-targets --all-features`: format and lint before PR.

## Coding Style & Concurrency Rules
Use Rust 2021 defaults (`snake_case`, `PascalCase`, `Result`-first error handling). Do not hold `RwLock` guards across `.await`. Preserve axum route syntax (`/api/tasks/{id}`) and SSE replay behavior (recent-event replay + live stream).

## Pull Request Guidelines
Use focused Conventional Commit subjects (`feat:`, `fix:`, `docs:`, `refactor:`). PRs must include:
- Which MVP items were affected and how they were verified.
- Exact commands run locally.
- For UI/API changes: screenshots or response snippets for success and error paths.
