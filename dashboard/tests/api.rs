//! HTTP API integration tests for dashboard.
//!
//! These tests spawn the real `dashboard` binary on a free port, exercise
//! every endpoint, and verify responses.  No QEMU is required — `POST /api/run`
//! is only tested end-to-end when `DASHBOARD_IT=1` is set.
//!
//! Requires:
//!   - `dashboard` binary to be built (debug or release)
//!   - `curl` on PATH (used for the SSE replay test)

use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use reqwest::blocking::Client;

// ── helpers ───────────────────────────────────────────────────────────────────

fn dashboard_bin() -> String {
    std::env::var("CARGO_BIN_EXE_dashboard").unwrap_or_else(|_| {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../target/debug/dashboard")
            .to_string_lossy()
            .into_owned()
    })
}

fn example_job_dir() -> String {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../batch/example/job1")
        .canonicalize()
        .expect("batch/example/job1 must exist")
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
    process: Child,
}

impl TestServer {
    fn start() -> Self {
        let port = find_free_port();
        let bin = dashboard_bin();
        let process = Command::new(&bin)
            .arg("--port")
            .arg(port.to_string())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap_or_else(|e| panic!("failed to start {bin}: {e}"));

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

        TestServer { port, process }
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
        self.process.kill().ok();
        self.process.wait().ok();
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
    assert!(
        body.contains("<!DOCTYPE html>"),
        "missing DOCTYPE in body"
    );
    assert!(
        body.contains("dashboard"),
        "title 'dashboard' not found in HTML"
    );
    assert!(
        body.contains("EventSource"),
        "HTML must include SSE client code"
    );
}

// ── GET /api/status ───────────────────────────────────────────────────────────

#[test]
fn get_status_returns_empty_initial_state() {
    let server = TestServer::start();
    let resp = server.client().get(server.url("/api/status")).send().unwrap();

    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().unwrap();
    assert_eq!(body["jobs"], serde_json::json!([]), "jobs must be empty initially");
    assert_eq!(body["running"], false, "running must be false initially");
}

// ── POST /api/jobs ────────────────────────────────────────────────────────────

#[test]
fn add_job_with_valid_dir_returns_201_and_appears_in_status() {
    let server = TestServer::start();
    let client = server.client();
    let dir = example_job_dir();

    let resp = client
        .post(server.url("/api/jobs"))
        .json(&serde_json::json!({"dir": dir}))
        .send()
        .unwrap();
    assert_eq!(resp.status(), 201);
    let body: serde_json::Value = resp.json().unwrap();
    assert!(body["id"].is_number(), "response must contain numeric id: {body}");

    // Confirm job appears in status
    let status: serde_json::Value = client
        .get(server.url("/api/status"))
        .send()
        .unwrap()
        .json()
        .unwrap();
    let jobs = status["jobs"].as_array().unwrap();
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0]["state"], "queued");
    assert_eq!(jobs[0]["name"], "job1-print-result");
    assert_eq!(jobs[0]["running"], serde_json::Value::Null);
}

#[test]
fn add_job_with_invalid_dir_returns_400_with_error_field() {
    let server = TestServer::start();
    let resp = server
        .client()
        .post(server.url("/api/jobs"))
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
fn add_five_jobs_returns_400_on_fifth() {
    let server = TestServer::start();
    let client = server.client();
    let dir = example_job_dir();

    for i in 0..4 {
        let r = client
            .post(server.url("/api/jobs"))
            .json(&serde_json::json!({"dir": dir}))
            .send()
            .unwrap();
        assert_eq!(r.status(), 201, "job {i} should be accepted");
    }

    let resp = client
        .post(server.url("/api/jobs"))
        .json(&serde_json::json!({"dir": dir}))
        .send()
        .unwrap();
    assert_eq!(resp.status(), 400, "5th job must be rejected");
    let body: serde_json::Value = resp.json().unwrap();
    assert!(
        body["error"].as_str().unwrap_or("").contains("full"),
        "error message should mention 'full': {body}"
    );
}

// ── DELETE /api/jobs/{id} ─────────────────────────────────────────────────────

#[test]
fn remove_queued_job_returns_204_and_disappears_from_status() {
    let server = TestServer::start();
    let client = server.client();

    let add: serde_json::Value = client
        .post(server.url("/api/jobs"))
        .json(&serde_json::json!({"dir": example_job_dir()}))
        .send()
        .unwrap()
        .json()
        .unwrap();
    let id = add["id"].as_u64().unwrap();

    let del = client
        .delete(server.url(&format!("/api/jobs/{id}")))
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
        status["jobs"].as_array().unwrap().len(),
        0,
        "job must be gone from status after DELETE"
    );
}

#[test]
fn remove_nonexistent_job_returns_404() {
    let server = TestServer::start();
    let resp = server
        .client()
        .delete(server.url("/api/jobs/9999"))
        .send()
        .unwrap();
    assert_eq!(resp.status(), 404);
}

// ── POST /api/run ─────────────────────────────────────────────────────────────

#[test]
fn run_with_empty_queue_returns_400() {
    let server = TestServer::start();
    let resp = server
        .client()
        .post(server.url("/api/run"))
        .send()
        .unwrap();
    assert_eq!(resp.status(), 400);
    let body: serde_json::Value = resp.json().unwrap();
    assert!(body["error"].is_string(), "expected error field: {body}");
}

#[test]
fn run_starts_jobs_and_sets_running_true() {
    let server = TestServer::start();
    let client = server.client();

    client
        .post(server.url("/api/jobs"))
        .json(&serde_json::json!({"dir": example_job_dir()}))
        .send()
        .unwrap();

    let resp = client
        .post(server.url("/api/run"))
        .send()
        .unwrap();
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
        .post(server.url("/api/jobs"))
        .json(&serde_json::json!({"dir": example_job_dir()}))
        .send()
        .unwrap();
    client.post(server.url("/api/run")).send().unwrap();

    let resp = client.post(server.url("/api/run")).send().unwrap();
    assert_eq!(resp.status(), 409);
}

/// Full end-to-end test: adds a job, runs it, waits for completion, checks output.
/// Skipped unless `DASHBOARD_IT=1` is set (requires QEMU).
#[test]
fn run_api_run_job_succeeds() {
    if std::env::var("DASHBOARD_IT").is_err() {
        eprintln!("Skipping VM end-to-end test: set DASHBOARD_IT=1 to run.");
        return;
    }

    let server = TestServer::start();
    let client = server.client();

    client
        .post(server.url("/api/jobs"))
        .json(&serde_json::json!({"dir": example_job_dir()}))
        .send()
        .unwrap();

    client.post(server.url("/api/run")).send().unwrap();

    // Poll status until job is no longer running (max 5 min).
    let deadline = Instant::now() + Duration::from_secs(300);
    loop {
        assert!(Instant::now() < deadline, "job did not complete within 5 min");
        std::thread::sleep(Duration::from_secs(5));

        let status: serde_json::Value = client
            .get(server.url("/api/status"))
            .send()
            .unwrap()
            .json()
            .unwrap();

        let job = &status["jobs"][0];
        let state = job["state"].as_str().unwrap_or("unknown");
        eprintln!("state={state} phase={}", job["phase"]);

        match state {
            "succeeded" => {
                assert!(
                    job["output"].is_string(),
                    "succeeded job must have output path"
                );
                assert!(job["elapsed_ms"].is_number());
                break;
            }
            "failed" => {
                panic!("job failed: {}", job["error"]);
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
            resp.headers()["content-type"]
                .to_str()
                .unwrap()
                .to_string(),
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

    // Add a job so the server has a "loaded" event in its replay buffer.
    client
        .post(server.url("/api/jobs"))
        .json(&serde_json::json!({"dir": example_job_dir()}))
        .send()
        .unwrap();

    // Connect via curl with a short max-time; the replay event arrives immediately.
    let output = Command::new("curl")
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
        body.contains("job1-print-result"),
        "SSE event must include job name from job.json; got: {body}"
    );
}
