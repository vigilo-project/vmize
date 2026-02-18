# dashboard

`dashboard` is VMize's web control plane for [`batch`](../batch).
It replaces terminal-only execution views with a browser UI for queueing, live progress, and operational visibility over SSH.

## Minimum Goal

The MVP is considered complete when all of the following pass end-to-end in a browser:

1. **Start server** — `dashboard --port 8080` starts without error.
2. **Add tasks** — entering a task directory path and clicking **Add** queues the task (validates `task.json` exists); shows name and description from `task.json`.
3. **Reject bad paths** — adding a non-existent directory shows an error; the queue is unchanged.
4. **Queue limit** — adding a 5th task is rejected with a clear error.
5. **Remove tasks** — clicking **✕** on a queued task removes it before any run starts.
6. **Run All** — clicking **Run All** starts all queued tasks in parallel and disables the button.
7. **Live progress** — each task card updates in real time: phase badge, script `N/M`, last 3 log lines, elapsed time.
8. **Completion** — succeeded tasks turn green with the output path; failed tasks turn red with the error.
9. **Reconnect** — refreshing the browser restores the current state via `/api/status` and replays the last 100 SSE events.

## Quick Start

```bash
# From workspace root
cargo build --release

./target/release/dashboard            # default port 8080
./target/release/dashboard --port 9090
```

Open `http://localhost:8080` in a browser.

## UX Flow

```
1. Enter task directory path → click Add (or press Enter)
   └─ server reads task.json → shows name + description on the card
2. Repeat for up to 4 tasks
3. Click Run All
   └─ all tasks start in parallel (each in its own OS thread)
4. Cards update in real time via Server-Sent Events
5. Each card turns green (success) or red (failure) when done
```

## Task Directory Structure

A task directory is the same format as `batch`:

```
<task-dir>/
├── task.json     # name, description, disk_size (all optional)
├── scripts/     # Shell scripts executed alphabetically inside the VM
└── output/      # Created automatically; results are collected here
```

Example `task.json`:

```json
{
  "name": "my-build",
  "description": "Compile and test inside an ephemeral VM",
  "disk_size": "20G"
}
```

## API Reference

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/` | Embedded HTML dashboard |
| `GET` | `/events` | Server-Sent Events stream |
| `GET` | `/api/status` | JSON snapshot of all tasks and `running` flag |
| `POST` | `/api/tasks` | Add task — body: `{"dir": "/path/to/task"}` |
| `DELETE` | `/api/tasks/{id}` | Remove a queued task by ID |
| `POST` | `/api/run` | Start all queued tasks |

### Status codes

| Code | Meaning |
|------|---------|
| 201 | Task added |
| 202 | Run started |
| 204 | Task removed |
| 400 | Bad request (invalid dir, empty queue, queue full) |
| 404 | Task ID not found |
| 409 | Conflict (already running) |

## SSE Event Format

All events are JSON objects on `data:` lines. New subscribers receive the last 100 events as a replay before live events begin.

```json
{"type":"loaded",   "id":0, "name":"my-build", "description":"..."}
{"type":"phase",    "id":0, "phase":"StartingVm"}
{"type":"script",   "id":0, "name":"10_build.sh", "index":1, "total":3, "done":false}
{"type":"script",   "id":0, "name":"10_build.sh", "index":1, "total":3, "done":true}
{"type":"log",      "id":0, "line":"Building..."}
{"type":"finished", "id":0, "success":true,  "elapsed_ms":42000, "output":"/path/to/output"}
{"type":"finished", "id":0, "success":false, "elapsed_ms":5000,  "error":"ScriptFailed: ..."}
```

## Testing

QEMU and SSH must be installed (see `vm` crate's `deps.sh`). `curl` must be on `PATH`.

```bash
# Unit tests only (no VM, no QEMU)
cargo test --lib -p dashboard

# All tests including HTTP API (no VM needed — only tests the HTTP layer)
cargo test --test api -p dashboard

# Full end-to-end (requires QEMU — actually runs a VM)
DASHBOARD_IT=1 cargo test --test api run_api_run_task_succeeds -p dashboard -- --nocapture
```
