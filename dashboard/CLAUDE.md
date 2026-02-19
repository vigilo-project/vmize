# CLAUDE.md

## Project

dashboard — VMize's axum-based web server for running `worker` tasks in parallel with real-time progress via Server-Sent Events.

## Commands

```bash
cargo build                    # Debug build
cargo build --release          # Release build
cargo test -p dashboard --lib       # Unit tests only (no QEMU)
cargo test -p dashboard --test api  # HTTP API integration tests
cargo run -p vmize -- dashboard --help
cargo clippy                   # Lint
cargo fmt                      # Format
```

## Architecture

Core server logic lives in `src/lib.rs` (library crate). The workspace CLI entrypoint that launches it is `../cli/src/main.rs`.

### Entry point

`dashboard::start(port)` initialises `AppState`, registers six axum routes, and calls `axum::serve`. CLI parsing (`--port`, default 8080) is handled by `vmize dashboard`.

### Shared state

```
Arc<RwLock<DashboardState>>   ← read/write from HTTP handlers and task threads
Arc<broadcast::Sender<String>> ← SSE fan-out; also held by AppState (Clone)
```

`AppState` is `Clone` (both fields are `Arc`) and is registered as axum `State`.

`DashboardState` holds:
- `tasks: Vec<TaskEntry>` — ordered by insertion
- `next_id: usize` — monotonic counter
- `running: bool` — true while any task thread is alive
- `replay: VecDeque<String>` — last 100 SSE JSON strings for late subscribers

`push_event(&mut self, tx, value)` serialises the JSON value, appends to `replay` (evicts oldest when full), and calls `tx.send()`.

### SSE design

`GET /events` subscribes to the broadcast channel, then:
1. Drains `replay` into a `futures::stream::iter` (snapshot taken under read lock, released before streaming)
2. Chains with a `futures::stream::unfold` live stream from the broadcast receiver
3. Handles `RecvError::Lagged` by continuing (skips burst); closes on `Closed`

The combined stream is boxed (`Pin<Box<dyn Stream<...>>>`) to unify the two concrete types.

### Worker threading model

`POST /api/run` spawns one `std::thread` per queued task (NOT `tokio::task::spawn_blocking`, to avoid `BlockingInAsyncContext` from `run_task_blocking_with_progress`).

Each worker thread:
1. Loads task definition via `task::load_task()`
2. Spawns a second thread to forward `mpsc::Receiver<RunProgress>` → `broadcast::Sender<String>`
3. Calls `run_task_blocking_with_progress()` (creates its own tokio runtime)
4. Joins the forwarder thread
5. Acquires write lock, updates `TaskEntry` state, calls `push_event`, calls `maybe_clear_running`

No deadlock is possible: the forwarder thread and main worker thread never hold the write lock simultaneously (worker joins forwarder before acquiring the final lock).

### Routes

| Handler | Route | Notes |
|---------|-------|-------|
| `serve_dashboard` | `GET /` | `include_str!("dashboard.html")` |
| `sse_handler` | `GET /events` | replay + live stream |
| `get_status` | `GET /api/status` | read lock only |
| `add_task` | `POST /api/tasks` | validates dir, creates TaskEntry, push_event "loaded" |
| `remove_task` | `DELETE /api/tasks/{id}` | axum 0.8 path syntax |
| `run_tasks` | `POST /api/run` | sets running=true, spawns threads |

## Key types

| Type | Description |
|------|-------------|
| `TaskState` | `Queued \| Running \| Succeeded \| Failed` |
| `TaskEntry` | Per-task state including recent_logs (VecDeque, max 10) |
| `DashboardState` | Global state behind RwLock |
| `AppState` | Cloneable axum state (SharedState + broadcast Sender) |

## Conventions

- **Locking**: Never hold `RwLock` across `.await`. All handlers acquire, operate, drop before returning.
- **Path syntax**: axum 0.8 uses `{id}` (not `:id`) for path parameters.
- **OS threads for workers**: prevents `BlockingInAsyncContext`; broadcast::Sender::send() is sync-safe.
- **SSE stream type**: boxed trait object avoids naming the `Chain<Iter<...>, Unfold<...>>` type.
- **Error responses**: always `(StatusCode, Json(json!({"error": "..."})))`.
