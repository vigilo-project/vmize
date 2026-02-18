# CLAUDE.md

## Project

vm-dashboard — axum-based web server that provides a browser UI for running `vm-batch` jobs in parallel with real-time progress via Server-Sent Events.

## Commands

```bash
cargo build                    # Debug build
cargo build --release          # Release build
cargo test --lib               # Unit tests only (no server, no QEMU)
cargo test --test api          # HTTP API integration tests (no QEMU)
cargo clippy                   # Lint
cargo fmt                      # Format
```

## Architecture

Everything lives in `src/main.rs` (single binary, no lib crate).

### Entry point

`main()` parses CLI (`--port`, default 8080), initialises `AppState`, registers six axum routes, and calls `axum::serve`.

### Shared state

```
Arc<RwLock<DashboardState>>   ← read/write from HTTP handlers and job threads
Arc<broadcast::Sender<String>> ← SSE fan-out; also held by AppState (Clone)
```

`AppState` is `Clone` (both fields are `Arc`) and is registered as axum `State`.

`DashboardState` holds:
- `jobs: Vec<JobEntry>` — ordered by insertion
- `next_id: usize` — monotonic counter
- `running: bool` — true while any job thread is alive
- `replay: VecDeque<String>` — last 100 SSE JSON strings for late subscribers

`push_event(&mut self, tx, value)` serialises the JSON value, appends to `replay` (evicts oldest when full), and calls `tx.send()`.

### SSE design

`GET /events` subscribes to the broadcast channel, then:
1. Drains `replay` into a `futures::stream::iter` (snapshot taken under read lock, released before streaming)
2. Chains with a `futures::stream::unfold` live stream from the broadcast receiver
3. Handles `RecvError::Lagged` by continuing (skips burst); closes on `Closed`

The combined stream is boxed (`Pin<Box<dyn Stream<...>>>`) to unify the two concrete types.

### Worker threading model

`POST /api/run` spawns one `std::thread` per queued job (NOT `tokio::task::spawn_blocking`, to avoid `BlockingInAsyncContext` from `run_in_out_blocking_with_progress`).

Each worker thread:
1. Loads job definition via `vm_batch::job::load_job()`
2. Spawns a second thread to forward `mpsc::Receiver<RunProgress>` → `broadcast::Sender<String>`
3. Calls `run_in_out_blocking_with_progress()` (creates its own tokio runtime)
4. Joins the forwarder thread
5. Acquires write lock, updates `JobEntry` state, calls `push_event`, calls `maybe_clear_running`

No deadlock is possible: the forwarder thread and main worker thread never hold the write lock simultaneously (worker joins forwarder before acquiring the final lock).

### Routes

| Handler | Route | Notes |
|---------|-------|-------|
| `serve_dashboard` | `GET /` | `include_str!("dashboard.html")` |
| `sse_handler` | `GET /events` | replay + live stream |
| `get_status` | `GET /api/status` | read lock only |
| `add_job` | `POST /api/jobs` | validates dir, creates JobEntry, push_event "loaded" |
| `remove_job` | `DELETE /api/jobs/{id}` | axum 0.8 path syntax |
| `run_jobs` | `POST /api/run` | sets running=true, spawns threads |

## Key types

| Type | Description |
|------|-------------|
| `JobState` | `Queued \| Running \| Succeeded \| Failed` |
| `JobEntry` | Per-job state including recent_logs (VecDeque, max 10) |
| `DashboardState` | Global state behind RwLock |
| `AppState` | Cloneable axum state (SharedState + broadcast Sender) |

## Conventions

- **Locking**: Never hold `RwLock` across `.await`. All handlers acquire, operate, drop before returning.
- **Path syntax**: axum 0.8 uses `{id}` (not `:id`) for path parameters.
- **OS threads for workers**: prevents `BlockingInAsyncContext`; broadcast::Sender::send() is sync-safe.
- **SSE stream type**: boxed trait object avoids naming the `Chain<Iter<...>, Unfold<...>>` type.
- **Error responses**: always `(StatusCode, Json(json!({"error": "..."})))`.
