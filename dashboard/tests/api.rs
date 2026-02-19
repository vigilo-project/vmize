//! HTTP API integration tests for dashboard.
//!
//! These tests spawn the dashboard server in-process via `dashboard::start()`,
//! exercise every endpoint, and verify responses.  No QEMU is required —
//! `POST /api/run` is only tested end-to-end when `DASHBOARD_IT=1` is set.
//!
//! Requires:
//!   - `curl` on PATH (used for the SSE replay test)

use std::path::Path;
use std::time::{Duration, Instant};

use reqwest::blocking::Client;
use tokio::task::JoinHandle;

// ── helpers ───────────────────────────────────────────────────────────────────

fn example_task_dir() -> String {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../batch/example/task1")
        .canonicalize()
        .expect("batch/example/task1 must exist")
        .to_string_lossy()
        .into_owned()
}

fn find_free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

struct TestServer {
    pub port: u16,
    handle: JoinHandle<()>,
}

impl TestServer {
    fn start() -> Self {
        let port = find_free_port();

        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("failed to create tokio runtime");

        let handle = rt.spawn(dashboard::start(port));

        // Leak the runtime so it stays alive for the test duration.
        // The JoinHandle::abort in Drop will stop the server task.
        std::mem::forget(rt);

        // Poll until the server responds or the deadline passes.
        let client = Client::new();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if Instant::now() > deadline {
                panic!("dashboard did not become ready within 5 s on port {port}");
            }
            if client
                .get(format!("http://127.0.0.1:{port}/api/status"))
                .timeout(Duration::from_millis(200))
                .send()
                .is_ok()
            {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }

        TestServer { port, handle }
    }

    fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{}", self.port, path)
    }

    fn client(&self) -> Client {
        Client::new()
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

// ── GET / ─────────────────────────────────────────────────────────────────────

#[test]
fn serve_dashboard_returns_html() {
    let server = TestServer::start();
    let resp = server.client().get(server.url("/")).send().unwrap();

    assert_eq!(resp.status(), 200);
    let ct = resp.headers()["content-type"].to_str().unwrap();
    assert!(ct.contains("text/html"), "content-type: {ct}");

    let body = resp.text().unwrap();
    assert!(body.contains("<!DOCTYPE html>"), "missing DOCTYPE in body");
    assert!(body.contains("VMIZE"), "title 'VMIZE' not found in HTML");
    assert!(
        body.contains("EventSource"),
        "HTML must include SSE client code"
    );
}

// ── GET /api/status ───────────────────────────────────────────────────────────

#[test]
fn get_status_returns_empty_initial_state() {
    let server = TestServer::start();
    let resp = server
        .client()
        .get(server.url("/api/status"))
        .send()
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().unwrap();
    assert_eq!(
        body["tasks"],
        serde_json::json!([]),
        "tasks must be empty initially"
    );
    assert_eq!(body["running"], false, "running must be false initially");
}

// ── GET /api/task-candidates ────────────────────────────────────────────────

#[test]
fn get_task_candidates_returns_workspace_tasks_sorted() {
    let server = TestServer::start();
    let resp = server
        .client()
        .get(server.url("/api/task-candidates"))
        .send()
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().unwrap();
    let tasks = body["tasks"].as_array().expect("tasks must be an array");
    assert!(!tasks.is_empty(), "expected workspace tasks in response");

    let tasks_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../tasks")
        .canonicalize()
        .expect("workspace tasks dir must exist");

    let mut names = Vec::new();
    for task in tasks {
        let name = task["name"].as_str().unwrap_or("").to_string();
        let dir = task["dir"]
            .as_str()
            .expect("candidate must include task directory");
        assert!(
            !name.is_empty(),
            "candidate must include non-empty task name: {task}"
        );

        let dir_path = Path::new(dir)
            .canonicalize()
            .expect("candidate dir must exist on disk");
        assert!(
            dir_path.starts_with(&tasks_root),
            "candidate dir must be under tasks/: {}",
            dir_path.display()
        );
        assert!(
            dir_path.join("task.json").is_file(),
            "candidate dir must contain task.json: {}",
            dir_path.display()
        );
        names.push(name);
    }

    let mut sorted_names = names.clone();
    sorted_names.sort_by_key(|name| name.to_lowercase());
    assert_eq!(
        names, sorted_names,
        "task candidates must be sorted by name"
    );
}

// ── POST /api/tasks ────────────────────────────────────────────────────────────

#[test]
fn add_task_with_valid_dir_returns_201_and_appears_in_status() {
    let server = TestServer::start();
    let client = server.client();
    let dir = example_task_dir();

    let resp = client
        .post(server.url("/api/tasks"))
        .json(&serde_json::json!({"dir": dir}))
        .send()
        .unwrap();
    assert_eq!(resp.status(), 201);
    let body: serde_json::Value = resp.json().unwrap();
    assert!(
        body["id"].is_number(),
        "response must contain numeric id: {body}"
    );

    // Confirm task appears in status
    let status: serde_json::Value = client
        .get(server.url("/api/status"))
        .send()
        .unwrap()
        .json()
        .unwrap();
    let tasks = status["tasks"].as_array().unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0]["state"], "queued");
    assert_eq!(tasks[0]["name"], "task1-print-result");
    assert_eq!(tasks[0]["running"], serde_json::Value::Null);
}

#[test]
fn add_task_with_invalid_dir_returns_400_with_error_field() {
    let server = TestServer::start();
    let resp = server
        .client()
        .post(server.url("/api/tasks"))
        .json(&serde_json::json!({"dir": "/definitely/does/not/exist"}))
        .send()
        .unwrap();

    assert_eq!(resp.status(), 400);
    let body: serde_json::Value = resp.json().unwrap();
    assert!(
        body["error"].is_string(),
        "response must have an 'error' string: {body}"
    );
}

#[test]
fn add_five_tasks_returns_400_on_fifth() {
    let server = TestServer::start();
    let client = server.client();
    let dir = example_task_dir();

    for i in 0..4 {
        let r = client
            .post(server.url("/api/tasks"))
            .json(&serde_json::json!({"dir": dir}))
            .send()
            .unwrap();
        assert_eq!(r.status(), 201, "task {i} should be accepted");
    }

    let resp = client
        .post(server.url("/api/tasks"))
        .json(&serde_json::json!({"dir": dir}))
        .send()
        .unwrap();
    assert_eq!(resp.status(), 400, "5th task must be rejected");
    let body: serde_json::Value = resp.json().unwrap();
    assert!(
        body["error"].as_str().unwrap_or("").contains("full"),
        "error message should mention 'full': {body}"
    );
}

// ── DELETE /api/tasks/{id} ─────────────────────────────────────────────────────

#[test]
fn remove_queued_task_returns_204_and_disappears_from_status() {
    let server = TestServer::start();
    let client = server.client();

    let add: serde_json::Value = client
        .post(server.url("/api/tasks"))
        .json(&serde_json::json!({"dir": example_task_dir()}))
        .send()
        .unwrap()
        .json()
        .unwrap();
    let id = add["id"].as_u64().unwrap();

    let del = client
        .delete(server.url(&format!("/api/tasks/{id}")))
        .send()
        .unwrap();
    assert_eq!(del.status(), 204);

    let status: serde_json::Value = client
        .get(server.url("/api/status"))
        .send()
        .unwrap()
        .json()
        .unwrap();
    assert_eq!(
        status["tasks"].as_array().unwrap().len(),
        0,
        "task must be gone from status after DELETE"
    );
}

#[test]
fn remove_nonexistent_task_returns_404() {
    let server = TestServer::start();
    let resp = server
        .client()
        .delete(server.url("/api/tasks/9999"))
        .send()
        .unwrap();
    assert_eq!(resp.status(), 404);
}

// ── POST /api/run ─────────────────────────────────────────────────────────────

#[test]
fn run_with_empty_queue_returns_400() {
    let server = TestServer::start();
    let resp = server.client().post(server.url("/api/run")).send().unwrap();
    assert_eq!(resp.status(), 400);
    let body: serde_json::Value = resp.json().unwrap();
    assert!(body["error"].is_string(), "expected error field: {body}");
}

#[test]
fn run_starts_tasks_and_sets_running_true() {
    let server = TestServer::start();
    let client = server.client();

    client
        .post(server.url("/api/tasks"))
        .json(&serde_json::json!({"dir": example_task_dir()}))
        .send()
        .unwrap();

    let resp = client.post(server.url("/api/run")).send().unwrap();
    assert_eq!(resp.status(), 202);
    let body: serde_json::Value = resp.json().unwrap();
    assert_eq!(body["status"], "started");

    // running flag must be true immediately after /api/run
    let status: serde_json::Value = client
        .get(server.url("/api/status"))
        .send()
        .unwrap()
        .json()
        .unwrap();
    assert_eq!(
        status["running"], true,
        "running must be true after POST /api/run"
    );
}

#[test]
fn run_twice_returns_409_on_second_call() {
    let server = TestServer::start();
    let client = server.client();

    client
        .post(server.url("/api/tasks"))
        .json(&serde_json::json!({"dir": example_task_dir()}))
        .send()
        .unwrap();
    client.post(server.url("/api/run")).send().unwrap();

    let resp = client.post(server.url("/api/run")).send().unwrap();
    assert_eq!(resp.status(), 409);
}

/// Full end-to-end test: adds a task, runs it, waits for completion, checks output.
/// Skipped unless `DASHBOARD_IT=1` is set (requires QEMU).
#[test]
fn run_api_run_task_succeeds() {
    if std::env::var("DASHBOARD_IT").is_err() {
        eprintln!("Skipping VM end-to-end test: set DASHBOARD_IT=1 to run.");
        return;
    }

    let server = TestServer::start();
    let client = server.client();

    client
        .post(server.url("/api/tasks"))
        .json(&serde_json::json!({"dir": example_task_dir()}))
        .send()
        .unwrap();

    client.post(server.url("/api/run")).send().unwrap();

    // Poll status until task is no longer running (max 5 min).
    let deadline = Instant::now() + Duration::from_secs(300);
    loop {
        assert!(
            Instant::now() < deadline,
            "task did not complete within 5 min"
        );
        std::thread::sleep(Duration::from_secs(5));

        let status: serde_json::Value = client
            .get(server.url("/api/status"))
            .send()
            .unwrap()
            .json()
            .unwrap();

        let task = &status["tasks"][0];
        let state = task["state"].as_str().unwrap_or("unknown");
        eprintln!("state={state} phase={}", task["phase"]);

        match state {
            "succeeded" => {
                assert!(
                    task["output"].is_string(),
                    "succeeded task must have output path"
                );
                assert!(task["elapsed_ms"].is_number());
                break;
            }
            "failed" => {
                panic!("task failed: {}", task["error"]);
            }
            _ => {}
        }
    }
}

// ── GET /events (SSE) ─────────────────────────────────────────────────────────

#[test]
fn sse_endpoint_returns_event_stream_content_type() {
    let server = TestServer::start();

    // spawn in a thread so the main thread can assert without blocking
    let port = server.port;
    let handle = std::thread::spawn(move || {
        let client = Client::new();
        let resp = client
            .get(format!("http://127.0.0.1:{port}/events"))
            .send()
            .unwrap();
        (
            resp.status().as_u16(),
            resp.headers()["content-type"].to_str().unwrap().to_string(),
        )
    });

    let (status, content_type) = handle.join().expect("SSE thread panicked");
    assert_eq!(status, 200);
    assert!(
        content_type.contains("text/event-stream"),
        "content-type must be text/event-stream, got: {content_type}"
    );
}

#[test]
fn sse_replays_loaded_event_to_new_subscriber() {
    let server = TestServer::start();
    let client = server.client();

    // Add a task so the server has a "loaded" event in its replay buffer.
    client
        .post(server.url("/api/tasks"))
        .json(&serde_json::json!({"dir": example_task_dir()}))
        .send()
        .unwrap();

    // Connect via curl with a short max-time; the replay event arrives immediately.
    let output = std::process::Command::new("curl")
        .args(["-s", "-N", "--max-time", "2"])
        .arg(server.url("/events"))
        .output()
        .expect("curl must be on PATH for this test");

    let body = String::from_utf8_lossy(&output.stdout);
    assert!(
        body.contains("loaded"),
        "SSE replay must contain 'loaded' event; got: {body}"
    );
    assert!(
        body.contains("task1-print-result"),
        "SSE event must include task name from task.json; got: {body}"
    );
}
