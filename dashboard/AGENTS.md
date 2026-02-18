# Repository Guidelines

## Product Context
`dashboard` is VMize's browser interface for orchestrating and observing task execution across isolated VMs.

## Project Structure
`dashboard` is a library crate.
- `src/lib.rs`: HTTP routes, shared state, worker threading, SSE broadcast
- `src/dashboard.html`: embedded frontend served at `GET /`
- `tests/api.rs`: HTTP/API integration tests and optional VM end-to-end path
- `../cli/src/main.rs`: workspace CLI entrypoint (`vmize dashboard --port ...`)

## Minimum Goal (MVP) — Required Release Gate
A dashboard change is complete only when all of these remain true:
1. `vmize dashboard --port 8080` starts successfully.
2. Valid task path can be added; queue shows `name`/`description` from `task.json`.
3. Invalid path is rejected and queue remains unchanged.
4. A 5th queued task is rejected (queue max 4).
5. Queued tasks can be removed before run.
6. `Run All` starts queued tasks in parallel and blocks duplicate starts.
7. Live updates show phase, script progress (`N/M`), recent logs, and elapsed time.
8. Completion state is explicit: success includes output path; failure includes error.
9. Browser refresh restores state via `/api/status` and replays recent SSE events.

## Acceptance Checklist
- API paths stay aligned: `/api/tasks`, `/api/tasks/{id}`, `/api/run`, `/api/status`.
- SSE replay remains enabled and bounded.
- Locking model remains safe (`RwLock` not held across `.await`).

## Verification Commands
```bash
cargo build --release -p dashboard
cargo test -p dashboard --lib
cargo test -p dashboard --test api
cargo run -p vmize -- dashboard --help

# Optional VM-required path
DASHBOARD_IT=1 cargo test --test api run_api_run_task_succeeds -p dashboard -- --nocapture
```

## Coding & Concurrency Rules
- Rust 2021 style (`snake_case`, `PascalCase`, `Result`-first error handling)
- Do not hold `RwLock` guards across `.await`
- Preserve axum route syntax (`/api/tasks/{id}`)
- Preserve SSE replay behavior (recent-event replay + live stream)

## PR Expectations
- Specify which MVP items were affected
- Include exact commands run locally
- For UI/API changes, attach response snippets or screenshots
