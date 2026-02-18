use std::collections::VecDeque;
use std::convert::Infallible;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use std::time::Instant;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse, Response,
    },
    routing::{delete, get, post},
    Json, Router,
};
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

use batch::{
    load_task, run_task_blocking_with_progress, RunPhase, RunProgress, TaskRunOptions,
    MAX_CONCURRENT_TASKS,
};

const SSE_REPLAY_CAPACITY: usize = 100;

// ── State ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
enum TaskState {
    Queued,
    Running,
    Succeeded,
    Failed,
}

#[derive(Debug, Clone, Serialize)]
struct TaskEntry {
    id: usize,
    dir: String,
    name: Option<String>,
    description: Option<String>,
    state: TaskState,
    phase: Option<String>,
    script_index: usize,
    script_total: usize,
    current_script: Option<String>,
    recent_logs: VecDeque<String>,
    elapsed_ms: Option<u64>,
    error: Option<String>,
    output: Option<String>,
}

struct DashboardState {
    tasks: Vec<TaskEntry>,
    next_id: usize,
    running: bool,
    /// Last N SSE event strings replayed to new subscribers.
    replay: VecDeque<String>,
}

impl DashboardState {
    fn new() -> Self {
        Self {
            tasks: Vec::new(),
            next_id: 0,
            running: false,
            replay: VecDeque::with_capacity(SSE_REPLAY_CAPACITY),
        }
    }

    fn push_event(&mut self, sse_tx: &broadcast::Sender<String>, event: serde_json::Value) {
        let s = event.to_string();
        if self.replay.len() >= SSE_REPLAY_CAPACITY {
            self.replay.pop_front();
        }
        self.replay.push_back(s.clone());
        let _ = sse_tx.send(s);
    }
}

type SharedState = Arc<RwLock<DashboardState>>;

#[derive(Clone)]
struct AppState {
    state: SharedState,
    sse_tx: Arc<broadcast::Sender<String>>,
}

// ── Request bodies ────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct AddTaskRequest {
    dir: String,
}

// ── Entry point ───────────────────────────────────────────────────────────────

/// Start the dashboard web server on the given port.
///
/// This function runs until the server is shut down (e.g. the task is aborted).
pub async fn start(port: u16) {
    let (sse_tx, _) = broadcast::channel::<String>(256);
    let app_state = AppState {
        state: Arc::new(RwLock::new(DashboardState::new())),
        sse_tx: Arc::new(sse_tx),
    };

    let app = Router::new()
        .route("/", get(serve_dashboard))
        .route("/events", get(sse_handler))
        .route("/api/status", get(get_status))
        .route("/api/tasks", post(add_task))
        .route("/api/tasks/{id}", delete(remove_task))
        .route("/api/run", post(run_tasks))
        .with_state(app_state);

    let addr = format!("0.0.0.0:{port}");
    eprintln!("Dashboard: http://localhost:{port}");
    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

// ── Handlers ──────────────────────────────────────────────────────────────────

async fn serve_dashboard() -> impl IntoResponse {
    (
        [(axum::http::header::CONTENT_TYPE, "text/html; charset=utf-8")],
        include_str!("dashboard.html"),
    )
}

type SseStream = std::pin::Pin<Box<dyn futures::Stream<Item = Result<Event, Infallible>> + Send>>;

async fn sse_handler(State(app): State<AppState>) -> impl IntoResponse {
    let rx = app.sse_tx.subscribe();
    let replay: Vec<String> = {
        let s = app.state.read().unwrap();
        s.replay.iter().cloned().collect()
    };

    let replay_stream = futures::stream::iter(
        replay
            .into_iter()
            .map(|data| Ok::<Event, Infallible>(Event::default().data(data))),
    );

    let live_stream = futures::stream::unfold(rx, |mut rx| async move {
        loop {
            match rx.recv().await {
                Ok(data) => return Some((Ok(Event::default().data(data)), rx)),
                Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => return None,
            }
        }
    });

    let stream: SseStream = Box::pin(replay_stream.chain(live_stream));
    Sse::new(stream).keep_alive(KeepAlive::default())
}

async fn get_status(State(app): State<AppState>) -> impl IntoResponse {
    let s = app.state.read().unwrap();
    Json(serde_json::json!({
        "tasks": s.tasks,
        "running": s.running,
    }))
}

async fn add_task(State(app): State<AppState>, Json(req): Json<AddTaskRequest>) -> Response {
    let mut s = app.state.write().unwrap();

    if s.running {
        return (
            StatusCode::CONFLICT,
            Json(serde_json::json!({"error": "tasks are already running"})),
        )
            .into_response();
    }

    if s.tasks.len() >= MAX_CONCURRENT_TASKS {
        return (
            StatusCode::BAD_REQUEST,
            Json(
                serde_json::json!({"error": format!("queue is full (max {MAX_CONCURRENT_TASKS})")}),
            ),
        )
            .into_response();
    }

    let task_dir = PathBuf::from(&req.dir);
    let (name, description) = match load_task(&task_dir) {
        Ok(loaded) => (loaded.definition.name, loaded.definition.description),
        Err(err) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": err})),
            )
                .into_response();
        }
    };

    let id = s.next_id;
    s.next_id += 1;

    s.tasks.push(TaskEntry {
        id,
        dir: req.dir,
        name: name.clone(),
        description: description.clone(),
        state: TaskState::Queued,
        phase: None,
        script_index: 0,
        script_total: 0,
        current_script: None,
        recent_logs: VecDeque::new(),
        elapsed_ms: None,
        error: None,
        output: None,
    });

    s.push_event(
        &app.sse_tx,
        serde_json::json!({
            "type": "loaded",
            "id": id,
            "name": name,
            "description": description,
        }),
    );

    (StatusCode::CREATED, Json(serde_json::json!({"id": id}))).into_response()
}

async fn remove_task(State(app): State<AppState>, Path(id): Path<usize>) -> Response {
    let mut s = app.state.write().unwrap();

    if s.running {
        return (
            StatusCode::CONFLICT,
            Json(serde_json::json!({"error": "cannot remove tasks while running"})),
        )
            .into_response();
    }

    if let Some(pos) = s.tasks.iter().position(|j| j.id == id) {
        s.tasks.remove(pos);
        StatusCode::NO_CONTENT.into_response()
    } else {
        (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "task not found"})),
        )
            .into_response()
    }
}

async fn run_tasks(State(app): State<AppState>) -> Response {
    let tasks_to_run: Vec<(usize, PathBuf)> = {
        let mut s = app.state.write().unwrap();

        if s.running {
            return (
                StatusCode::CONFLICT,
                Json(serde_json::json!({"error": "already running"})),
            )
                .into_response();
        }

        if s.tasks.is_empty() {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "no tasks in queue"})),
            )
                .into_response();
        }

        s.running = true;
        s.tasks
            .iter()
            .filter(|j| j.state == TaskState::Queued)
            .map(|j| (j.id, PathBuf::from(&j.dir)))
            .collect()
    };

    for (id, task_path) in tasks_to_run {
        let app_clone = app.clone();
        std::thread::Builder::new()
            .name(format!("dashboard-task-{id}"))
            .spawn(move || run_task_worker(id, task_path, app_clone))
            .expect("failed to spawn task worker thread");
    }

    (
        StatusCode::ACCEPTED,
        Json(serde_json::json!({"status": "started"})),
    )
        .into_response()
}

// ── Task worker (runs in a plain OS thread to avoid blocking the async runtime) ─

fn run_task_worker(id: usize, task_path: PathBuf, app: AppState) {
    let started_at = Instant::now();

    let loaded = match load_task(&task_path) {
        Ok(t) => t,
        Err(err) => {
            let mut s = app.state.write().unwrap();
            if let Some(task) = s.tasks.iter_mut().find(|j| j.id == id) {
                task.state = TaskState::Failed;
                task.error = Some(err.clone());
            }
            s.push_event(
                &app.sse_tx,
                serde_json::json!({
                    "type": "finished",
                    "id": id,
                    "success": false,
                    "error": err,
                }),
            );
            maybe_clear_running(&mut s);
            return;
        }
    };

    // Mark running
    {
        let mut s = app.state.write().unwrap();
        if let Some(task) = s.tasks.iter_mut().find(|j| j.id == id) {
            task.state = TaskState::Running;
        }
        s.push_event(
            &app.sse_tx,
            serde_json::json!({"type": "phase", "id": id, "phase": "StartingVm"}),
        );
    }

    // Progress forwarding thread
    let (progress_tx, progress_rx) = std::sync::mpsc::channel::<RunProgress>();
    let app_for_progress = app.clone();
    let progress_thread = std::thread::spawn(move || {
        while let Ok(progress) = progress_rx.recv() {
            forward_progress(id, &progress, &app_for_progress);
        }
    });

    let options = TaskRunOptions {
        disk_size: loaded.definition.disk_size.clone(),
        show_progress: false,
    };
    let result = run_task_blocking_with_progress(
        &loaded.input_dir,
        &loaded.output_dir,
        options,
        Some(progress_tx),
    );
    let _ = progress_thread.join();

    let elapsed_ms = started_at.elapsed().as_millis() as u64;

    let mut s = app.state.write().unwrap();
    match result {
        Ok(_) => {
            if let Some(task) = s.tasks.iter_mut().find(|j| j.id == id) {
                task.state = TaskState::Succeeded;
                task.elapsed_ms = Some(elapsed_ms);
                task.output = Some(loaded.output_dir.display().to_string());
            }
            s.push_event(
                &app.sse_tx,
                serde_json::json!({
                    "type": "finished",
                    "id": id,
                    "success": true,
                    "elapsed_ms": elapsed_ms,
                    "output": loaded.output_dir.display().to_string(),
                }),
            );
        }
        Err(err) => {
            let err_str = err.to_string();
            if let Some(task) = s.tasks.iter_mut().find(|j| j.id == id) {
                task.state = TaskState::Failed;
                task.elapsed_ms = Some(elapsed_ms);
                task.error = Some(err_str.clone());
            }
            s.push_event(
                &app.sse_tx,
                serde_json::json!({
                    "type": "finished",
                    "id": id,
                    "success": false,
                    "elapsed_ms": elapsed_ms,
                    "error": err_str,
                }),
            );
        }
    }
    maybe_clear_running(&mut s);
}

fn forward_progress(id: usize, progress: &RunProgress, app: &AppState) {
    let mut s = app.state.write().unwrap();

    let event = match progress {
        RunProgress::Phase(phase) => {
            let label = match phase {
                RunPhase::ValidatingPaths => "ValidatingPaths",
                RunPhase::StartingVm => "StartingVm",
                RunPhase::PreparingVm => "PreparingVm",
                RunPhase::RunningScripts => "RunningScripts",
                RunPhase::CollectingOutput => "CollectingOutput",
                RunPhase::CleaningUp => "CleaningUp",
            };
            if let Some(task) = s.tasks.iter_mut().find(|j| j.id == id) {
                task.phase = Some(label.to_string());
            }
            serde_json::json!({"type": "phase", "id": id, "phase": label})
        }

        RunProgress::ScriptStarted {
            script,
            index,
            total,
        } => {
            if let Some(task) = s.tasks.iter_mut().find(|j| j.id == id) {
                task.current_script = Some(script.clone());
                task.script_index = *index;
                task.script_total = *total;
                task.recent_logs.clear();
            }
            serde_json::json!({
                "type": "script",
                "id": id,
                "name": script,
                "index": index,
                "total": total,
                "done": false,
            })
        }

        RunProgress::ScriptFinished {
            script,
            index,
            total,
        } => {
            if let Some(task) = s.tasks.iter_mut().find(|j| j.id == id) {
                task.script_index = *index;
                task.script_total = *total;
            }
            serde_json::json!({
                "type": "script",
                "id": id,
                "name": script,
                "index": index,
                "total": total,
                "done": true,
            })
        }

        RunProgress::LogLine { line } => {
            if let Some(task) = s.tasks.iter_mut().find(|j| j.id == id) {
                if task.recent_logs.len() >= 10 {
                    task.recent_logs.pop_front();
                }
                task.recent_logs.push_back(line.clone());
            }
            serde_json::json!({"type": "log", "id": id, "line": line})
        }
    };

    s.push_event(&app.sse_tx, event);
}

fn maybe_clear_running(s: &mut DashboardState) {
    let all_done = s
        .tasks
        .iter()
        .all(|j| matches!(j.state, TaskState::Succeeded | TaskState::Failed));
    if all_done {
        s.running = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── helpers ───────────────────────────────────────────────────────────────

    fn make_app() -> (AppState, broadcast::Receiver<String>) {
        let (tx, rx) = broadcast::channel(32);
        let app = AppState {
            state: Arc::new(RwLock::new(DashboardState::new())),
            sse_tx: Arc::new(tx),
        };
        (app, rx)
    }

    fn make_task(id: usize, state: TaskState) -> TaskEntry {
        TaskEntry {
            id,
            dir: format!("/tasks/{id}"),
            name: Some(format!("task-{id}")),
            description: None,
            state,
            phase: None,
            script_index: 0,
            script_total: 0,
            current_script: None,
            recent_logs: VecDeque::new(),
            elapsed_ms: None,
            error: None,
            output: None,
        }
    }

    // ── DashboardState::new ───────────────────────────────────────────────────

    #[test]
    fn new_state_is_empty_and_not_running() {
        let s = DashboardState::new();
        assert!(s.tasks.is_empty());
        assert!(!s.running);
        assert_eq!(s.next_id, 0);
        assert!(s.replay.is_empty());
    }

    // ── push_event ────────────────────────────────────────────────────────────

    #[test]
    fn push_event_records_to_replay_and_broadcasts() {
        let (app, mut rx) = make_app();
        let mut s = app.state.write().unwrap();
        s.push_event(&app.sse_tx, serde_json::json!({"type": "test", "v": 1}));

        assert_eq!(s.replay.len(), 1);
        assert!(s.replay[0].contains("\"type\":\"test\""));

        let received = rx.try_recv().expect("broadcast should have one message");
        assert!(received.contains("\"type\":\"test\""));
    }

    #[test]
    fn push_event_evicts_oldest_when_at_capacity() {
        let (app, _rx) = make_app();
        let mut s = app.state.write().unwrap();

        for i in 0..SSE_REPLAY_CAPACITY {
            s.push_event(&app.sse_tx, serde_json::json!({"seq": i}));
        }
        assert_eq!(s.replay.len(), SSE_REPLAY_CAPACITY);

        s.push_event(&app.sse_tx, serde_json::json!({"seq": "newest"}));

        assert_eq!(
            s.replay.len(),
            SSE_REPLAY_CAPACITY,
            "capacity must not grow"
        );
        // seq:0 was the oldest and should have been evicted
        assert!(
            !s.replay[0].contains("\"seq\":0"),
            "oldest event should be evicted, front = {}",
            s.replay[0]
        );
        assert!(
            s.replay.back().unwrap().contains("newest"),
            "newest event should be at the back"
        );
    }

    // ── maybe_clear_running ───────────────────────────────────────────────────

    #[test]
    fn maybe_clear_running_clears_when_all_tasks_terminal() {
        let mut s = DashboardState::new();
        s.running = true;
        s.tasks.push(make_task(0, TaskState::Succeeded));
        s.tasks.push(make_task(1, TaskState::Failed));

        maybe_clear_running(&mut s);

        assert!(!s.running);
    }

    #[test]
    fn maybe_clear_running_keeps_true_while_any_running() {
        let mut s = DashboardState::new();
        s.running = true;
        s.tasks.push(make_task(0, TaskState::Succeeded));
        s.tasks.push(make_task(1, TaskState::Running));

        maybe_clear_running(&mut s);

        assert!(s.running);
    }

    #[test]
    fn maybe_clear_running_keeps_true_while_any_queued() {
        let mut s = DashboardState::new();
        s.running = true;
        s.tasks.push(make_task(0, TaskState::Succeeded));
        s.tasks.push(make_task(1, TaskState::Queued));

        maybe_clear_running(&mut s);

        assert!(s.running);
    }

    #[test]
    fn maybe_clear_running_does_not_set_false_when_no_tasks() {
        // Empty task list: all() on empty iterator returns true,
        // so running is cleared. This is the correct behaviour — it
        // can only be reached in practice when a run completes with 0 tasks,
        // which the API prevents (run_tasks rejects empty queues).
        let mut s = DashboardState::new();
        s.running = true;
        maybe_clear_running(&mut s);
        // Document the current (vacuously-true) behaviour explicitly.
        assert!(!s.running);
    }

    // ── forward_progress ─────────────────────────────────────────────────────

    #[test]
    fn forward_progress_phase_updates_task_and_broadcasts_event() {
        let (app, mut rx) = make_app();
        {
            let mut s = app.state.write().unwrap();
            s.tasks.push(make_task(0, TaskState::Running));
        }

        forward_progress(0, &RunProgress::Phase(RunPhase::RunningScripts), &app);

        let s = app.state.read().unwrap();
        assert_eq!(s.tasks[0].phase, Some("RunningScripts".to_string()));

        let evt: serde_json::Value = serde_json::from_str(&rx.try_recv().unwrap()).unwrap();
        assert_eq!(evt["type"], "phase");
        assert_eq!(evt["id"], 0);
        assert_eq!(evt["phase"], "RunningScripts");
    }

    #[test]
    fn forward_progress_script_started_clears_logs_and_sets_current() {
        let (app, _rx) = make_app();
        {
            let mut s = app.state.write().unwrap();
            let mut task = make_task(0, TaskState::Running);
            task.recent_logs.push_back("old log".to_string());
            s.tasks.push(task);
        }

        forward_progress(
            0,
            &RunProgress::ScriptStarted {
                script: "10_build.sh".to_string(),
                index: 1,
                total: 3,
            },
            &app,
        );

        let s = app.state.read().unwrap();
        assert!(
            s.tasks[0].recent_logs.is_empty(),
            "logs must be cleared on ScriptStarted"
        );
        assert_eq!(s.tasks[0].current_script, Some("10_build.sh".to_string()));
        assert_eq!(s.tasks[0].script_index, 1);
        assert_eq!(s.tasks[0].script_total, 3);
    }

    #[test]
    fn forward_progress_script_finished_updates_counters() {
        let (app, mut rx) = make_app();
        {
            let mut s = app.state.write().unwrap();
            s.tasks.push(make_task(0, TaskState::Running));
        }

        forward_progress(
            0,
            &RunProgress::ScriptFinished {
                script: "10_build.sh".to_string(),
                index: 1,
                total: 3,
            },
            &app,
        );

        let s = app.state.read().unwrap();
        assert_eq!(s.tasks[0].script_index, 1);
        assert_eq!(s.tasks[0].script_total, 3);

        let evt: serde_json::Value = serde_json::from_str(&rx.try_recv().unwrap()).unwrap();
        assert_eq!(evt["done"], true);
    }

    #[test]
    fn forward_progress_log_line_appends_and_caps_at_ten() {
        let (app, _rx) = make_app();
        {
            let mut s = app.state.write().unwrap();
            s.tasks.push(make_task(0, TaskState::Running));
        }

        for i in 0..12usize {
            forward_progress(
                0,
                &RunProgress::LogLine {
                    line: format!("line {i}"),
                },
                &app,
            );
        }

        let s = app.state.read().unwrap();
        assert_eq!(s.tasks[0].recent_logs.len(), 10, "must cap at 10 entries");
        assert_eq!(
            s.tasks[0].recent_logs.front().unwrap(),
            "line 2",
            "oldest surviving entry should be line 2"
        );
        assert_eq!(
            s.tasks[0].recent_logs.back().unwrap(),
            "line 11",
            "newest entry should be line 11"
        );
    }

    #[test]
    fn forward_progress_ignores_unknown_task_id_silently() {
        let (app, _rx) = make_app();
        // No tasks in state — should not panic.
        forward_progress(99, &RunProgress::Phase(RunPhase::StartingVm), &app);
    }
}
