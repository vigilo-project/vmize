use std::collections::VecDeque;
use std::convert::Infallible;
use std::fs;
use std::path::{Path as StdPath, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::Instant;

use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::{
        IntoResponse, Response,
        sse::{Event, KeepAlive, Sse},
    },
    routing::{delete, get, post},
};
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use task::load_task;
use tokio::sync::broadcast;

use worker::{
    ChainRunResult, ChainStepProgress, MAX_BATCH_TASKS, RunPhase, RunProgress, TaskRunOptions,
    run_task_chain_blocking_with_progress,
};

const SSE_REPLAY_CAPACITY: usize = 100;
const DASHBOARD_EVENT_PROTOCOL_VERSION: u8 = 1;

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum DashboardSseEvent {
    Loaded {
        id: usize,
        #[serde(skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        description: Option<String>,
    },
    Phase {
        id: usize,
        phase: String,
    },
    Script {
        id: usize,
        name: String,
        index: usize,
        total: usize,
        done: bool,
    },
    VmProgress {
        id: usize,
        line: String,
    },
    ScriptLog {
        id: usize,
        line: String,
    },
    ChainStep {
        id: usize,
        step_index: usize,
        total_steps: usize,
        #[serde(skip_serializing_if = "Option::is_none")]
        task_name: Option<String>,
        task_dir: String,
    },
    Finished {
        id: usize,
        success: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        elapsed_ms: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        output: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        chain_steps: Vec<ChainStepEntry>,
        #[serde(skip_serializing_if = "Option::is_none")]
        chain_failed_step_index: Option<usize>,
    },
}

fn worker_phase_label(phase: RunPhase) -> &'static str {
    match phase {
        RunPhase::StartingVm => "StartingVm",
        RunPhase::PreparingVm => "PreparingVm",
        RunPhase::RunningScripts => "RunningScripts",
        RunPhase::CollectingOutput => "CollectingOutput",
        RunPhase::CleaningUp => "CleaningUp",
    }
}

fn serialize_and_push_event<T: Serialize>(
    sse_tx: &broadcast::Sender<String>,
    replay: &mut VecDeque<String>,
    event: T,
) -> Option<String> {
    let mut payload = serde_json::to_value(event).ok()?;
    payload.as_object_mut()?.insert(
        "protocol_version".to_string(),
        serde_json::json!(DASHBOARD_EVENT_PROTOCOL_VERSION),
    );

    let data = serde_json::to_string(&payload).ok()?;
    if replay.len() >= SSE_REPLAY_CAPACITY {
        replay.pop_front();
    }
    replay.push_back(data.clone());
    let _ = sse_tx.send(data.clone());

    Some(data)
}

// ── State ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
enum TaskState {
    Queued,
    Running,
    Succeeded,
    Failed,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum ChainStepState {
    Queued,
    Running,
    Succeeded,
    Failed,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct ChainStepEntry {
    index: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    task_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    task_dir: Option<String>,
    state: ChainStepState,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
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
    chain_step_index: usize,
    chain_step_total: usize,
    chain_current_task: Option<String>,
    chain_steps: Vec<ChainStepEntry>,
    chain_failed_step_index: Option<usize>,
    vm_progress_lines: Vec<String>,
    script_output_lines: Vec<String>,
    elapsed_ms: Option<u64>,
    error: Option<String>,
    output: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct TaskCandidate {
    name: String,
    description: Option<String>,
    dir: String,
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

    fn push_event<T: Serialize>(&mut self, sse_tx: &broadcast::Sender<String>, event: T) {
        let _ = serialize_and_push_event(sse_tx, &mut self.replay, event);
    }
}

type SharedState = Arc<RwLock<DashboardState>>;

#[derive(Clone)]
struct AppState {
    state: SharedState,
    sse_tx: Arc<broadcast::Sender<String>>,
    workspace_root: PathBuf,
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
        workspace_root: detect_workspace_root(),
    };

    let app = Router::new()
        .route("/", get(serve_dashboard))
        .route("/events", get(sse_handler))
        .route("/api/status", get(get_status))
        .route("/api/task-candidates", get(get_task_candidates))
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

async fn get_task_candidates(State(app): State<AppState>) -> impl IntoResponse {
    let tasks = discover_task_candidates(&app.workspace_root);
    Json(serde_json::json!({ "tasks": tasks }))
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

    if s.tasks.len() >= MAX_BATCH_TASKS {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": format!("queue is full (max {MAX_BATCH_TASKS})")})),
        )
            .into_response();
    }

    let task_dir = PathBuf::from(&req.dir);
    let (name, description) = match load_task(&task_dir) {
        Ok(loaded) => (loaded.definition.name, loaded.definition.description),
        Err(err) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": err.to_string()})),
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
        chain_step_index: 0,
        chain_step_total: 0,
        chain_current_task: None,
        chain_steps: Vec::new(),
        chain_failed_step_index: None,
        vm_progress_lines: Vec::new(),
        script_output_lines: Vec::new(),
        elapsed_ms: None,
        error: None,
        output: None,
    });

    s.push_event(
        &app.sse_tx,
        DashboardSseEvent::Loaded {
            id,
            name,
            description,
        },
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

        let tasks_to_run: Vec<(usize, PathBuf)> = s
            .tasks
            .iter()
            .filter(|j| j.state == TaskState::Queued)
            .map(|j| (j.id, PathBuf::from(&j.dir)))
            .collect();

        if tasks_to_run.is_empty() {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "no queued tasks"})),
            )
                .into_response();
        }

        s.running = true;
        tasks_to_run
    };

    let mut started_any = false;
    for (id, task_path) in tasks_to_run {
        let app_clone = app.clone();
        let spawn_result = std::thread::Builder::new()
            .name(format!("dashboard-task-{id}"))
            .spawn(move || run_task_worker(id, task_path, app_clone));
        if let Err(err) = spawn_result {
            let mut s = app.state.write().unwrap();
            let message = format!("failed to spawn task worker thread: {err}");
            if let Some(task) = s.tasks.iter_mut().find(|j| j.id == id) {
                task.state = TaskState::Failed;
                task.error = Some(message.clone());
            }
            s.push_event(
                &app.sse_tx,
                DashboardSseEvent::Finished {
                    id,
                    success: false,
                    elapsed_ms: None,
                    output: None,
                    error: Some(message),
                    chain_steps: Vec::new(),
                    chain_failed_step_index: None,
                },
            );
            maybe_clear_running(&mut s);
        } else {
            started_any = true;
        }
    }

    if !started_any {
        let mut s = app.state.write().unwrap();
        maybe_clear_running(&mut s);
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({
                "error": "failed to start task worker threads"
            })),
        )
            .into_response();
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

    // Mark running
    {
        let mut s = app.state.write().unwrap();
        if let Some(task) = s.tasks.iter_mut().find(|j| j.id == id) {
            task.state = TaskState::Running;
        }
        s.push_event(
            &app.sse_tx,
            DashboardSseEvent::Phase {
                id,
                phase: "StartingVm".to_string(),
            },
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

    // Chain-step forwarding thread
    let (chain_step_tx, chain_step_rx) = std::sync::mpsc::channel::<ChainStepProgress>();
    let app_for_chain = app.clone();
    let chain_thread = std::thread::spawn(move || {
        while let Ok(step) = chain_step_rx.recv() {
            forward_chain_step(id, &step, &app_for_chain);
        }
    });

    let options = TaskRunOptions {
        disk_size: None,
        show_progress: false,
    };
    let result = run_task_chain_blocking_with_progress(
        &task_path,
        options,
        Some(progress_tx),
        Some(chain_step_tx),
    );
    let _ = progress_thread.join();
    let _ = chain_thread.join();

    let elapsed_ms = started_at.elapsed().as_millis() as u64;

    let mut s = app.state.write().unwrap();
    match result {
        Ok(chain_result) => {
            let output_path = chain_result
                .steps
                .last()
                .map(|step| step.run_result.output_dir.display().to_string())
                .unwrap_or_else(|| task_path.join("output").display().to_string());
            let mut chain_steps = Vec::new();
            let mut chain_failed_step_index = None;
            if let Some(task) = s.tasks.iter_mut().find(|j| j.id == id) {
                task.state = TaskState::Succeeded;
                task.elapsed_ms = Some(elapsed_ms);
                task.output = Some(output_path.clone());
                sync_chain_steps_from_result(task, &chain_result);
                chain_steps = task.chain_steps.clone();
                chain_failed_step_index = task.chain_failed_step_index;
            }
            s.push_event(
                &app.sse_tx,
                DashboardSseEvent::Finished {
                    id,
                    success: true,
                    elapsed_ms: Some(elapsed_ms),
                    output: Some(output_path),
                    error: None,
                    chain_steps,
                    chain_failed_step_index,
                },
            );
        }
        Err(err) => {
            let err_str = err.to_string();
            let mut chain_steps = Vec::new();
            let mut chain_failed_step_index = None;
            if let Some(task) = s.tasks.iter_mut().find(|j| j.id == id) {
                task.state = TaskState::Failed;
                task.elapsed_ms = Some(elapsed_ms);
                task.error = Some(err_str.clone());
                mark_chain_failure(task, &err_str);
                chain_steps = task.chain_steps.clone();
                chain_failed_step_index = task.chain_failed_step_index;
            }
            s.push_event(
                &app.sse_tx,
                DashboardSseEvent::Finished {
                    id,
                    success: false,
                    elapsed_ms: Some(elapsed_ms),
                    output: None,
                    error: Some(err_str),
                    chain_steps,
                    chain_failed_step_index,
                },
            );
        }
    }
    maybe_clear_running(&mut s);
}

fn forward_progress(id: usize, progress: &RunProgress, app: &AppState) {
    let mut s = app.state.write().unwrap();

    let event = match progress {
        RunProgress::Phase(phase) => {
            let label = worker_phase_label(*phase);
            if let Some(task) = s.tasks.iter_mut().find(|j| j.id == id) {
                task.phase = Some(label.to_string());
            }
            DashboardSseEvent::Phase {
                id,
                phase: label.to_string(),
            }
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
            }
            DashboardSseEvent::Script {
                id,
                name: script.clone(),
                index: *index,
                total: *total,
                done: false,
            }
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
            DashboardSseEvent::Script {
                id,
                name: script.clone(),
                index: *index,
                total: *total,
                done: true,
            }
        }

        RunProgress::VmProgressLine { line } => {
            if let Some(task) = s.tasks.iter_mut().find(|j| j.id == id) {
                task.vm_progress_lines.push(line.clone());
            }
            DashboardSseEvent::VmProgress {
                id,
                line: line.clone(),
            }
        }

        RunProgress::ScriptOutputLine { line } => {
            if let Some(task) = s.tasks.iter_mut().find(|j| j.id == id) {
                task.script_output_lines.push(line.clone());
            }
            DashboardSseEvent::ScriptLog {
                id,
                line: line.clone(),
            }
        }
    };

    s.push_event(&app.sse_tx, event);
}

fn ensure_chain_steps(task: &mut TaskEntry, total_steps: usize) {
    if total_steps <= task.chain_steps.len() {
        return;
    }
    let start_index = task.chain_steps.len() + 1;
    for index in start_index..=total_steps {
        task.chain_steps.push(ChainStepEntry {
            index,
            task_name: None,
            task_dir: None,
            state: ChainStepState::Queued,
            error: None,
        });
    }
}

fn mark_prior_running_steps_succeeded(task: &mut TaskEntry, next_step_index: usize) {
    for step in &mut task.chain_steps {
        if step.state == ChainStepState::Running && step.index != next_step_index {
            step.state = ChainStepState::Succeeded;
            step.error = None;
        }
    }
}

fn sync_chain_steps_from_result(task: &mut TaskEntry, chain_result: &ChainRunResult) {
    let total_steps = chain_result.steps.len();
    if total_steps == 0 {
        task.chain_step_index = 0;
        task.chain_step_total = 0;
        task.chain_current_task = None;
        task.chain_steps.clear();
        task.chain_failed_step_index = None;
        return;
    }

    ensure_chain_steps(task, total_steps);
    for (idx, step_result) in chain_result.steps.iter().enumerate() {
        if let Some(step) = task.chain_steps.get_mut(idx) {
            step.task_name = step_result.task_name.clone();
            step.task_dir = Some(step_result.task_dir.display().to_string());
            step.state = ChainStepState::Succeeded;
            step.error = None;
        }
    }
    task.chain_steps.truncate(total_steps);
    task.chain_step_index = total_steps;
    task.chain_step_total = total_steps;
    task.chain_current_task = chain_result
        .steps
        .last()
        .and_then(|step| step.task_name.clone());
    task.chain_failed_step_index = None;
}

fn mark_chain_failure(task: &mut TaskEntry, error: &str) {
    let mut failed_index = None;

    if let Some(step) = task
        .chain_steps
        .iter_mut()
        .find(|step| step.state == ChainStepState::Running)
    {
        step.state = ChainStepState::Failed;
        step.error = Some(error.to_string());
        failed_index = Some(step.index);
    } else if let Some(position) = task.chain_step_index.checked_sub(1)
        && let Some(step) = task.chain_steps.get_mut(position)
        && step.state != ChainStepState::Succeeded
    {
        step.state = ChainStepState::Failed;
        step.error = Some(error.to_string());
        failed_index = Some(step.index);
    }

    task.chain_failed_step_index = failed_index;
    if let Some(index) = failed_index {
        task.chain_step_index = index;
        if let Some(position) = index.checked_sub(1)
            && let Some(step) = task.chain_steps.get(position)
        {
            task.chain_current_task = step.task_name.clone();
        }
    }
}

fn forward_chain_step(id: usize, step: &ChainStepProgress, app: &AppState) {
    let mut s = app.state.write().unwrap();

    match step {
        ChainStepProgress::StepStarted {
            step_index,
            total_steps,
            task_dir,
            task_name,
        } => {
            let phase = format!("ChainStep {step_index}/{total_steps}");
            if let Some(task) = s.tasks.iter_mut().find(|j| j.id == id) {
                ensure_chain_steps(task, *total_steps);
                mark_prior_running_steps_succeeded(task, *step_index);
                if let Some(position) = step_index.checked_sub(1)
                    && let Some(chain_step) = task.chain_steps.get_mut(position)
                {
                    chain_step.task_name = task_name.clone();
                    chain_step.task_dir = Some(task_dir.display().to_string());
                    chain_step.state = ChainStepState::Running;
                    chain_step.error = None;
                }
                task.chain_step_index = *step_index;
                task.chain_step_total = *total_steps;
                task.chain_current_task = task_name.clone();
                task.chain_failed_step_index = None;
                task.phase = Some(phase);
            }
            s.push_event(
                &app.sse_tx,
                DashboardSseEvent::ChainStep {
                    id,
                    step_index: *step_index,
                    total_steps: *total_steps,
                    task_name: task_name.clone(),
                    task_dir: task_dir.display().to_string(),
                },
            );
        }
    }
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

fn detect_workspace_root() -> PathBuf {
    let dashboard_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    dashboard_root
        .parent()
        .map_or(dashboard_root.clone(), StdPath::to_path_buf)
}

fn discover_task_candidates(workspace_root: &StdPath) -> Vec<TaskCandidate> {
    let tasks_root = workspace_root.join("worker").join("example");
    let Ok(entries) = fs::read_dir(&tasks_root) else {
        return Vec::new();
    };

    let mut tasks = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() || !path.join("task.json").is_file() {
            continue;
        }

        let Ok(loaded) = load_task(&path) else {
            continue;
        };

        let fallback_name = path
            .file_name()
            .map_or_else(String::new, |name| name.to_string_lossy().to_string());
        let task_name = loaded.definition.name.unwrap_or(fallback_name);
        let task_dir = path.canonicalize().unwrap_or(path);

        tasks.push(TaskCandidate {
            name: task_name,
            description: loaded.definition.description,
            dir: task_dir.to_string_lossy().to_string(),
        });
    }

    tasks.sort_by_key(|task| (task.name.to_lowercase(), task.dir.clone()));
    tasks
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
            workspace_root: detect_workspace_root(),
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
            chain_step_index: 0,
            chain_step_total: 0,
            chain_current_task: None,
            chain_steps: Vec::new(),
            chain_failed_step_index: None,
            vm_progress_lines: Vec::new(),
            script_output_lines: Vec::new(),
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

    #[test]
    fn push_event_includes_protocol_version() {
        let (app, mut rx) = make_app();
        {
            let mut s = app.state.write().unwrap();
            s.push_event(
                &app.sse_tx,
                DashboardSseEvent::Loaded {
                    id: 3,
                    name: Some("task-x".into()),
                    description: Some("desc".into()),
                },
            );
        }

        let evt: serde_json::Value = serde_json::from_str(&rx.try_recv().unwrap()).unwrap();
        assert_eq!(evt["type"], "loaded");
        assert_eq!(evt["protocol_version"], 1);
    }

    #[test]
    fn forward_progress_phase_emits_protocol_version() {
        let (app, mut rx) = make_app();
        {
            let mut s = app.state.write().unwrap();
            s.tasks.push(make_task(0, TaskState::Running));
        }

        forward_progress(0, &RunProgress::Phase(RunPhase::StartingVm), &app);

        let evt: serde_json::Value = serde_json::from_str(&rx.try_recv().unwrap()).unwrap();
        assert_eq!(evt["type"], "phase");
        assert_eq!(evt["id"], 0);
        assert_eq!(evt["protocol_version"], 1);
    }

    #[test]
    fn forward_chain_step_updates_task_and_broadcasts_event() {
        let (app, mut rx) = make_app();
        {
            let mut s = app.state.write().unwrap();
            s.tasks.push(make_task(0, TaskState::Running));
        }

        forward_chain_step(
            0,
            &ChainStepProgress::StepStarted {
                step_index: 2,
                total_steps: 3,
                task_dir: PathBuf::from("/tmp/task-2"),
                task_name: Some("task-two".to_string()),
            },
            &app,
        );

        let s = app.state.read().unwrap();
        assert_eq!(s.tasks[0].chain_step_index, 2);
        assert_eq!(s.tasks[0].chain_step_total, 3);
        assert_eq!(s.tasks[0].chain_current_task, Some("task-two".to_string()));
        assert_eq!(s.tasks[0].phase, Some("ChainStep 2/3".to_string()));
        assert_eq!(s.tasks[0].chain_steps.len(), 3);
        assert_eq!(s.tasks[0].chain_steps[1].state, ChainStepState::Running);
        assert_eq!(
            s.tasks[0].chain_steps[1].task_name.as_deref(),
            Some("task-two")
        );

        let evt: serde_json::Value = serde_json::from_str(&rx.try_recv().unwrap()).unwrap();
        assert_eq!(evt["type"], "chain_step");
        assert_eq!(evt["id"], 0);
        assert_eq!(evt["step_index"], 2);
        assert_eq!(evt["total_steps"], 3);
        assert_eq!(evt["task_name"], "task-two");
        assert_eq!(evt["task_dir"], "/tmp/task-2");
    }

    #[test]
    fn forward_chain_step_marks_previous_running_step_succeeded() {
        let (app, _rx) = make_app();
        {
            let mut s = app.state.write().unwrap();
            let mut task = make_task(0, TaskState::Running);
            task.chain_steps = vec![
                ChainStepEntry {
                    index: 1,
                    task_name: Some("task-one".to_string()),
                    task_dir: Some("/tmp/task-1".to_string()),
                    state: ChainStepState::Running,
                    error: None,
                },
                ChainStepEntry {
                    index: 2,
                    task_name: None,
                    task_dir: None,
                    state: ChainStepState::Queued,
                    error: None,
                },
            ];
            task.chain_step_total = 2;
            task.chain_step_index = 1;
            s.tasks.push(task);
        }

        forward_chain_step(
            0,
            &ChainStepProgress::StepStarted {
                step_index: 2,
                total_steps: 2,
                task_dir: PathBuf::from("/tmp/task-2"),
                task_name: Some("task-two".to_string()),
            },
            &app,
        );

        let s = app.state.read().unwrap();
        assert_eq!(s.tasks[0].chain_steps[0].state, ChainStepState::Succeeded);
        assert_eq!(s.tasks[0].chain_steps[1].state, ChainStepState::Running);
    }

    #[test]
    fn mark_chain_failure_marks_running_step_as_failed() {
        let mut task = make_task(0, TaskState::Running);
        task.chain_steps = vec![
            ChainStepEntry {
                index: 1,
                task_name: Some("task-one".to_string()),
                task_dir: Some("/tmp/task-1".to_string()),
                state: ChainStepState::Succeeded,
                error: None,
            },
            ChainStepEntry {
                index: 2,
                task_name: Some("task-two".to_string()),
                task_dir: Some("/tmp/task-2".to_string()),
                state: ChainStepState::Running,
                error: None,
            },
        ];
        task.chain_step_total = 2;
        task.chain_step_index = 2;

        mark_chain_failure(&mut task, "boom");

        assert_eq!(task.chain_failed_step_index, Some(2));
        assert_eq!(task.chain_steps[1].state, ChainStepState::Failed);
        assert_eq!(task.chain_steps[1].error.as_deref(), Some("boom"));
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
    fn forward_progress_script_started_keeps_logs_and_sets_current() {
        let (app, _rx) = make_app();
        {
            let mut s = app.state.write().unwrap();
            let mut task = make_task(0, TaskState::Running);
            task.vm_progress_lines.push("vm progress".to_string());
            task.script_output_lines.push("script output".to_string());
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
        assert_eq!(
            s.tasks[0].vm_progress_lines,
            vec!["vm progress".to_string()]
        );
        assert_eq!(
            s.tasks[0].script_output_lines,
            vec!["script output".to_string()]
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
    fn forward_progress_vm_progress_line_appends_without_cap() {
        let (app, mut rx) = make_app();
        {
            let mut s = app.state.write().unwrap();
            s.tasks.push(make_task(0, TaskState::Running));
        }

        for i in 0..12usize {
            forward_progress(
                0,
                &RunProgress::VmProgressLine {
                    line: format!("line {i}"),
                },
                &app,
            );
        }

        let s = app.state.read().unwrap();
        assert_eq!(s.tasks[0].vm_progress_lines.len(), 12);
        assert_eq!(
            s.tasks[0].vm_progress_lines.first().unwrap(),
            "line 0",
            "first entry should be preserved"
        );
        assert_eq!(
            s.tasks[0].vm_progress_lines.last().unwrap(),
            "line 11",
            "newest entry should be line 11"
        );
        let evt: serde_json::Value = serde_json::from_str(&rx.try_recv().unwrap()).unwrap();
        assert_eq!(evt["type"], "vm_progress");
    }

    #[test]
    fn forward_progress_script_output_line_appends_without_cap() {
        let (app, mut rx) = make_app();
        {
            let mut s = app.state.write().unwrap();
            s.tasks.push(make_task(0, TaskState::Running));
        }

        for i in 0..12usize {
            forward_progress(
                0,
                &RunProgress::ScriptOutputLine {
                    line: format!("output {i}"),
                },
                &app,
            );
        }

        let s = app.state.read().unwrap();
        assert_eq!(s.tasks[0].script_output_lines.len(), 12);
        assert_eq!(
            s.tasks[0].script_output_lines.first().unwrap(),
            "output 0",
            "first entry should be preserved"
        );
        assert_eq!(
            s.tasks[0].script_output_lines.last().unwrap(),
            "output 11",
            "newest entry should be output 11"
        );
        let evt: serde_json::Value = serde_json::from_str(&rx.try_recv().unwrap()).unwrap();
        assert_eq!(evt["type"], "script_log");
    }

    #[test]
    fn forward_progress_ignores_unknown_task_id_silently() {
        let (app, _rx) = make_app();
        // No tasks in state — should not panic.
        forward_progress(99, &RunProgress::Phase(RunPhase::StartingVm), &app);
    }
}
