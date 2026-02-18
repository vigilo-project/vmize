# dashboard

`dashboard` is VMize's web control plane for [`batch`](../batch).
It provides queueing, live progress, and operational visibility for task runs.

## Minimum Goal
The MVP is complete only when all of these pass end-to-end in a browser:
1. Start server on `--port`.
2. Add valid task path (reads `task.json`).
3. Reject invalid path without queue mutation.
4. Reject 5th queued task (max queue: 4).
5. Remove queued task.
6. Run all queued tasks in parallel.
7. Show live phase/script/log updates.
8. Show explicit success/failure completion state.
9. Restore current state and replay recent events after refresh.

## Acceptance Checklist
- `GET /api/status` returns `tasks` + `running`.
- `POST /api/tasks` and `DELETE /api/tasks/{id}` behave consistently.
- `POST /api/run` enforces queue/running constraints.
- SSE (`/events`) replays and streams updates.

## Quick Start

```bash
cargo build --release
./target/release/dashboard --port 8080
```

Open `http://localhost:8080`.

## API

| Method | Path | Purpose |
|---|---|---|
| `GET` | `/` | Embedded HTML dashboard |
| `GET` | `/events` | SSE stream (replay + live) |
| `GET` | `/api/status` | Current tasks and running flag |
| `POST` | `/api/tasks` | Add queued task (`{"dir":"/path/to/task"}`) |
| `DELETE` | `/api/tasks/{id}` | Remove queued task |
| `POST` | `/api/run` | Start queued tasks |

## Verification Commands

```bash
cargo test -p dashboard --bin dashboard
cargo test -p dashboard --test api

# Optional VM-required path
DASHBOARD_IT=1 cargo test --test api run_api_run_task_succeeds -p dashboard -- --nocapture
```
