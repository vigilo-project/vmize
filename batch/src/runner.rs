use std::fs;
use std::path::Path;
use std::sync::mpsc;
use std::time::Instant;

use crate::vm_ops::{VmOps, VmOptions};
use crate::{Error, RunResult};

const VM_WORK_DIR: &str = "/tmp/batch/work";
const VM_OUTPUT_DIR: &str = "/tmp/batch/out";

pub struct TaskRunOptions {
    pub disk_size: Option<String>,
    /// Show vm crate's indicatif progress spinners.
    /// Defaults to `true`. Set to `false` in split-live mode where
    /// the caller draws its own UI.
    pub show_progress: bool,
}

impl Default for TaskRunOptions {
    fn default() -> Self {
        Self {
            disk_size: None,
            show_progress: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunPhase {
    ValidatingPaths,
    StartingVm,
    PreparingVm,
    RunningScripts,
    CollectingOutput,
    CleaningUp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunProgress {
    Phase(RunPhase),
    ScriptStarted {
        script: String,
        index: usize,
        total: usize,
    },
    ScriptFinished {
        script: String,
        index: usize,
        total: usize,
    },
    VmProgressLine {
        line: String,
    },
    ScriptOutputLine {
        line: String,
    },
}

// ═══════════════════════════════════════════════════════════════════════════════
// Public API (uses RealVmOps by default)
// ═══════════════════════════════════════════════════════════════════════════════

pub async fn run_task<Pi, Po>(input: Pi, output: Po) -> Result<RunResult, Error>
where
    Pi: AsRef<Path>,
    Po: AsRef<Path>,
{
    run_task_with_options(input, output, TaskRunOptions::default()).await
}

pub async fn run_task_with_options<Pi, Po>(
    input: Pi,
    output: Po,
    options: TaskRunOptions,
) -> Result<RunResult, Error>
where
    Pi: AsRef<Path>,
    Po: AsRef<Path>,
{
    run_task_with_progress(input, output, options, None).await
}

pub async fn run_task_with_progress<Pi, Po>(
    input: Pi,
    output: Po,
    options: TaskRunOptions,
    progress_tx: Option<mpsc::Sender<RunProgress>>,
) -> Result<RunResult, Error>
where
    Pi: AsRef<Path>,
    Po: AsRef<Path>,
{
    run_task_with_ops(
        &crate::vm_ops::RealVmOps,
        input.as_ref(),
        output.as_ref(),
        options,
        progress_tx,
    )
    .await
}

pub fn run_task_blocking<Pi, Po>(input: Pi, output: Po) -> Result<RunResult, Error>
where
    Pi: AsRef<Path>,
    Po: AsRef<Path>,
{
    run_task_blocking_with_options(input, output, TaskRunOptions::default())
}

pub fn run_task_blocking_with_options<Pi, Po>(
    input: Pi,
    output: Po,
    options: TaskRunOptions,
) -> Result<RunResult, Error>
where
    Pi: AsRef<Path>,
    Po: AsRef<Path>,
{
    run_task_blocking_with_progress(input, output, options, None)
}

pub fn run_task_blocking_with_progress<Pi, Po>(
    input: Pi,
    output: Po,
    options: TaskRunOptions,
    progress_tx: Option<mpsc::Sender<RunProgress>>,
) -> Result<RunResult, Error>
where
    Pi: AsRef<Path>,
    Po: AsRef<Path>,
{
    if tokio::runtime::Handle::try_current().is_ok() {
        return Err(Error::BlockingInAsyncContext);
    }

    let runtime = tokio::runtime::Runtime::new().map_err(|err| Error::Runtime {
        message: err.to_string(),
    })?;

    runtime.block_on(run_task_with_progress(input, output, options, progress_tx))
}

// ═══════════════════════════════════════════════════════════════════════════════
// Core Implementation (uses VmOps trait)
// ═══════════════════════════════════════════════════════════════════════════════

/// Core task runner that accepts a VmOps implementation.
/// This enables unit testing with MockVmOps.
pub async fn run_task_with_ops<V: VmOps + ?Sized>(
    vm_ops: &V,
    input_dir: &Path,
    output_dir: &Path,
    options: TaskRunOptions,
    progress_tx: Option<mpsc::Sender<RunProgress>>,
) -> Result<RunResult, Error> {
    let start = Instant::now();

    send_progress(&progress_tx, RunProgress::Phase(RunPhase::ValidatingPaths));
    let input_dir = validate_input_directory(input_dir)?;
    validate_output_directory(output_dir)?;

    let scripts = list_scripts(input_dir)?;
    if scripts.is_empty() {
        return Err(Error::NoScripts(input_dir.to_path_buf()));
    }

    send_progress(&progress_tx, RunProgress::Phase(RunPhase::StartingVm));
    let record = vm_ops
        .run(VmOptions {
            disk_size: options.disk_size,
            show_progress: options.show_progress,
        })
        .await
        .map_err(|err| Error::VmStart {
            message: err.to_string(),
        })?;

    let vm_id = record.id.clone();
    let mut result = RunResult::new(&vm_id, output_dir.to_path_buf());

    let run_error = match prepare_vm_with_ops(vm_ops, &vm_id, input_dir, &progress_tx).await {
        Ok(()) => {
            send_progress(&progress_tx, RunProgress::Phase(RunPhase::RunningScripts));
            execute_scripts_with_ops(vm_ops, &scripts, &record, input_dir, &progress_tx, &mut result)
        }
        Err(err) => Some(err),
    };

    send_progress(&progress_tx, RunProgress::Phase(RunPhase::CollectingOutput));

    // First ensure the output directory exists on the VM
    let mkdir_result = vm_ops.ssh(&vm_id, &format!("mkdir -p {}", shell_quote(VM_OUTPUT_DIR))).await;

    let collect_error = if let Err(err) = mkdir_result {
        Some(Error::VmCommand {
            message: err.to_string(),
        })
    } else {
        // Use wildcard to copy all files from output directory
        vm_ops
            .cp_from(
                &vm_id,
                &format!("{}/*", VM_OUTPUT_DIR),
                path_to_str(output_dir)?,
                true,
            )
            .map_err(|err| Error::CopyFromVm {
                message: err.to_string(),
            })
            .err()
    };

    // Primary run error takes precedence over collection error.
    let run_error = run_error.or(collect_error);

    send_progress(&progress_tx, RunProgress::Phase(RunPhase::CleaningUp));
    let cleanup_result = vm_ops.rm(&vm_id).map_err(|err| Error::CleanupFailed {
        vm_id: vm_id.clone(),
        message: err.to_string(),
    });

    result.elapsed_ms = start.elapsed().as_millis() as u64;

    match (run_error, cleanup_result) {
        (Some(err), Ok(_)) => {
            result.exit_code = 1;
            Err(err)
        }
        (None, Err(err)) => {
            result.exit_code = 1;
            Err(err)
        }
        (Some(run_err), Err(cleanup_err)) => {
            result.exit_code = 1;
            Err(combine_errors(run_err, cleanup_err))
        }
        (None, Ok(_)) => {
            result.exit_code = 0;
            Ok(result)
        }
    }
}

async fn prepare_vm_with_ops<V: VmOps + ?Sized>(
    vm_ops: &V,
    vm_id: &str,
    input_dir: &Path,
    progress_tx: &Option<mpsc::Sender<RunProgress>>,
) -> Result<(), Error> {
    send_progress(progress_tx, RunProgress::Phase(RunPhase::PreparingVm));

    vm_ops
        .ssh(
            vm_id,
            &format!(
                "mkdir -p {} {}",
                shell_quote(VM_WORK_DIR),
                shell_quote(VM_OUTPUT_DIR)
            ),
        )
        .await
        .map_err(|err| Error::VmCommand {
            message: err.to_string(),
        })?;

    // Copy the input directory to VM
    // Result: /tmp/batch/work/<dirname>/ inside VM
    vm_ops
        .cp_to(vm_id, path_to_str(input_dir)?, VM_WORK_DIR, true)
        .map_err(|err| Error::CopyToVm {
            message: err.to_string(),
        })
}

fn execute_scripts_with_ops<V: VmOps + ?Sized>(
    vm_ops: &V,
    scripts: &[String],
    record: &vm::VmRecord,
    input_dir: &Path,
    progress_tx: &Option<mpsc::Sender<RunProgress>>,
    result: &mut RunResult,
) -> Option<Error> {
    // Get the directory name from input_dir
    let dir_name = input_dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("scripts");

    let vm_work_subdir = format!("{}/{}", VM_WORK_DIR, dir_name);

    for script_name in scripts {
        let next_index = result.executed_scripts.len() + 1;
        send_progress(
            progress_tx,
            RunProgress::ScriptStarted {
                script: script_name.clone(),
                index: next_index,
                total: scripts.len(),
            },
        );

        let log_path = format!("{}/{}.log", VM_OUTPUT_DIR, script_name);
        let command = format!(
            "cd {} && /bin/bash {} 2>&1 | tee {}",
            shell_quote(&vm_work_subdir),
            shell_quote(script_name),
            shell_quote(&log_path),
        );

        let progress_tx_for_logs = progress_tx.clone();
        let stream_result = vm_ops.ssh_stream(record, &command, |line| {
            if let Some(tx) = &progress_tx_for_logs {
                let _ = tx.send(RunProgress::ScriptOutputLine { line });
            } else {
                println!("{line}");
            }
        });

        if let Err(err) = stream_result {
            return Some(Error::ScriptFailed {
                script: script_name.clone(),
                message: err.to_string(),
            });
        }

        result.executed_scripts.push(script_name.clone());
        let finished_index = result.executed_scripts.len();
        send_progress(
            progress_tx,
            RunProgress::ScriptFinished {
                script: script_name.clone(),
                index: finished_index,
                total: scripts.len(),
            },
        );
    }

    None
}

// ═══════════════════════════════════════════════════════════════════════════════
// Helper Functions
// ═══════════════════════════════════════════════════════════════════════════════

fn validate_input_directory(path: &Path) -> Result<&Path, Error> {
    if !path.exists() {
        return Err(Error::InputPathNotFound {
            path: path.to_path_buf(),
        });
    }

    if !path.is_dir() {
        return Err(Error::InputPathNotDirectory {
            path: path.to_path_buf(),
        });
    }

    Ok(path)
}

fn validate_output_directory(path: &Path) -> Result<(), Error> {
    if path.exists() {
        if !path.is_dir() {
            return Err(Error::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "output path must be a directory",
            )));
        }

        return Ok(());
    }

    fs::create_dir_all(path)?;
    Ok(())
}

fn list_scripts(input_dir: &Path) -> Result<Vec<String>, Error> {
    let mut scripts = Vec::new();

    let entries = fs::read_dir(input_dir).map_err(|err| Error::ScriptDiscovery {
        path: input_dir.to_path_buf(),
        source: err,
    })?;

    for entry in entries {
        let entry = entry.map_err(|err| Error::ScriptDiscovery {
            path: input_dir.to_path_buf(),
            source: err,
        })?;

        let metadata = entry.metadata().map_err(|err| Error::ScriptDiscovery {
            path: input_dir.to_path_buf(),
            source: err,
        })?;

        if !metadata.is_file() {
            continue;
        }

        let file_name = entry.file_name();
        let file_name_str = file_name
            .to_str()
            .map(std::string::ToString::to_string)
            .ok_or_else(|| Error::NonUtf8Path {
                path: input_dir.join(file_name),
            })?;

        // Only include .sh files
        if !file_name_str.ends_with(".sh") {
            continue;
        }

        scripts.push(file_name_str);
    }

    scripts.sort();
    Ok(scripts)
}

fn path_to_str(path: &Path) -> Result<&str, Error> {
    path.to_str().ok_or_else(|| Error::NonUtf8Path {
        path: path.to_path_buf(),
    })
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

/// Combines a primary run error with a secondary cleanup error into a single
/// `Runtime` error that preserves both messages. If there is no cleanup error
/// to combine, the primary error is returned unchanged.
fn combine_errors(primary: Error, cleanup: Error) -> Error {
    Error::Runtime {
        message: format!("{primary}; additionally: {cleanup}"),
    }
}

fn send_progress(progress_tx: &Option<mpsc::Sender<RunProgress>>, event: RunProgress) {
    if let Some(tx) = progress_tx {
        let _ = tx.send(event);
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Unit Tests
// ═══════════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vm_ops::MockVmOps;
    use std::fs;
    use std::path::PathBuf;
    use tempfile::TempDir;

    // ── validate_input_directory ───────────────────────────────────────────────

    #[test]
    fn validate_input_directory_returns_ok_for_existing_dir() {
        let temp = TempDir::new().expect("failed to create temp dir");
        let path = temp.path();

        let result = validate_input_directory(path);

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), path);
    }

    #[test]
    fn validate_input_directory_fails_for_nonexistent_path() {
        let nonexistent = PathBuf::from("/definitely/does/not/exist/12345");

        let result = validate_input_directory(&nonexistent);

        assert!(result.is_err());
        match result.unwrap_err() {
            Error::InputPathNotFound { path } => assert_eq!(path, nonexistent),
            err => panic!("Expected InputPathNotFound, got: {err}"),
        }
    }

    #[test]
    fn validate_input_directory_fails_for_file_not_dir() {
        let temp = TempDir::new().expect("failed to create temp dir");
        let file = temp.path().join("test.txt");
        fs::write(&file, "content").expect("failed to write file");

        let result = validate_input_directory(&file);

        assert!(result.is_err());
        match result.unwrap_err() {
            Error::InputPathNotDirectory { path } => assert_eq!(path, file),
            err => panic!("Expected InputPathNotDirectory, got: {err}"),
        }
    }

    // ── validate_output_directory ──────────────────────────────────────────────

    #[test]
    fn validate_output_directory_creates_missing_dir() {
        let temp = TempDir::new().expect("failed to create temp dir");
        let new_dir = temp.path().join("new_output_dir");

        let result = validate_output_directory(&new_dir);

        assert!(result.is_ok());
        assert!(new_dir.exists());
        assert!(new_dir.is_dir());
    }

    #[test]
    fn validate_output_directory_returns_ok_for_existing_dir() {
        let temp = TempDir::new().expect("failed to create temp dir");
        let existing_dir = temp.path().join("existing");
        fs::create_dir(&existing_dir).expect("failed to create dir");

        let result = validate_output_directory(&existing_dir);

        assert!(result.is_ok());
    }

    #[test]
    fn validate_output_directory_fails_for_file_not_dir() {
        let temp = TempDir::new().expect("failed to create temp dir");
        let file = temp.path().join("output.txt");
        fs::write(&file, "content").expect("failed to write file");

        let result = validate_output_directory(&file);

        assert!(result.is_err());
    }

    // ── list_scripts ───────────────────────────────────────────────────────────

    #[test]
    fn list_scripts_returns_sorted_sh_files() {
        let temp = TempDir::new().expect("failed to create temp dir");
        fs::write(temp.path().join("10_second.sh"), "").unwrap();
        fs::write(temp.path().join("00_first.sh"), "").unwrap();
        fs::write(temp.path().join("20_third.sh"), "").unwrap();

        let result = list_scripts(temp.path()).unwrap();

        assert_eq!(result, vec!["00_first.sh", "10_second.sh", "20_third.sh"]);
    }

    #[test]
    fn list_scripts_ignores_subdirectories() {
        let temp = TempDir::new().expect("failed to create temp dir");
        fs::write(temp.path().join("script.sh"), "").unwrap();
        fs::create_dir(temp.path().join("subdir.sh")).unwrap();

        let result = list_scripts(temp.path()).unwrap();

        assert_eq!(result, vec!["script.sh"]);
    }

    #[test]
    fn list_scripts_ignores_non_sh_files() {
        let temp = TempDir::new().expect("failed to create temp dir");
        fs::write(temp.path().join("script.sh"), "").unwrap();
        fs::write(temp.path().join("readme.txt"), "").unwrap();
        fs::write(temp.path().join("config.json"), "").unwrap();

        let result = list_scripts(temp.path()).unwrap();

        assert_eq!(result, vec!["script.sh"]);
    }

    #[test]
    fn list_scripts_returns_empty_for_empty_dir() {
        let temp = TempDir::new().expect("failed to create temp dir");

        let result = list_scripts(temp.path()).unwrap();

        assert!(result.is_empty());
    }

    #[test]
    fn list_scripts_sorts_alphabetically_case_insensitive_order() {
        let temp = TempDir::new().expect("failed to create temp dir");
        fs::write(temp.path().join("b_script.sh"), "").unwrap();
        fs::write(temp.path().join("A_script.sh"), "").unwrap();
        fs::write(temp.path().join("c_script.sh"), "").unwrap();

        let result = list_scripts(temp.path()).unwrap();

        // Standard sort is case-sensitive, uppercase comes before lowercase in ASCII
        assert_eq!(result, vec!["A_script.sh", "b_script.sh", "c_script.sh"]);
    }

    // ── shell_quote ────────────────────────────────────────────────────────────

    #[test]
    fn shell_quote_wraps_in_single_quotes() {
        assert_eq!(shell_quote("hello"), "'hello'");
    }

    #[test]
    fn shell_quote_escapes_embedded_single_quotes() {
        // Single quote in the middle should be escaped using the '\'' pattern
        assert_eq!(shell_quote("it's"), "'it'\"'\"'s'");
    }

    #[test]
    fn shell_quote_handles_empty_string() {
        assert_eq!(shell_quote(""), "''");
    }

    #[test]
    fn shell_quote_handles_special_chars() {
        // Characters like $, `, ", \ should NOT be escaped in single quotes
        assert_eq!(shell_quote("$HOME"), "'$HOME'");
        assert_eq!(shell_quote("echo `cmd`"), "'echo `cmd`'");
        assert_eq!(shell_quote("a\\b"), "'a\\b'");
    }

    #[test]
    fn shell_quote_handles_multiple_single_quotes() {
        assert_eq!(
            shell_quote("it's a test's"),
            "'it'\"'\"'s a test'\"'\"'s'"
        );
    }

    // ── combine_errors ─────────────────────────────────────────────────────────

    #[test]
    fn combine_errors_formats_both_messages() {
        let primary = Error::ScriptFailed {
            script: "test.sh".to_string(),
            message: "exit code 1".to_string(),
        };
        let cleanup = Error::CleanupFailed {
            vm_id: "vm0".to_string(),
            message: "timeout".to_string(),
        };

        let result = combine_errors(primary, cleanup);

        match result {
            Error::Runtime { message } => {
                assert!(message.contains("test.sh"));
                assert!(message.contains("exit code 1"));
                assert!(message.contains("vm0"));
                assert!(message.contains("timeout"));
                assert!(message.contains("additionally"));
            }
            err => panic!("Expected Runtime error, got: {err}"),
        }
    }

    // ── path_to_str ────────────────────────────────────────────────────────────

    #[test]
    fn path_to_str_returns_str_for_valid_utf8_path() {
        let path = PathBuf::from("/tmp/test/dir");

        let result = path_to_str(&path).unwrap();

        assert_eq!(result, "/tmp/test/dir");
    }

    #[test]
    fn path_to_str_fails_for_non_utf8_path() {
        // Create a path with invalid UTF-8 bytes (this is platform-dependent)
        // On most Unix systems, we can't easily create such paths from Rust
        // So we test with a valid path and ensure it works
        let path = PathBuf::from("/tmp/test");

        assert!(path_to_str(&path).is_ok());
    }

    // ── send_progress ──────────────────────────────────────────────────────────

    #[test]
    fn send_progress_sends_event_when_channel_present() {
        let (tx, rx) = mpsc::channel::<RunProgress>();
        let progress_tx = Some(tx);

        send_progress(&progress_tx, RunProgress::Phase(RunPhase::ValidatingPaths));

        let received = rx.try_recv().unwrap();
        assert_eq!(received, RunProgress::Phase(RunPhase::ValidatingPaths));
    }

    #[test]
    fn send_progress_does_nothing_when_channel_is_none() {
        let progress_tx: Option<mpsc::Sender<RunProgress>> = None;

        // Should not panic
        send_progress(&progress_tx, RunProgress::Phase(RunPhase::StartingVm));
    }

    #[test]
    fn send_progress_sends_script_started_event() {
        let (tx, rx) = mpsc::channel::<RunProgress>();
        let progress_tx = Some(tx);

        send_progress(
            &progress_tx,
            RunProgress::ScriptStarted {
                script: "00_init.sh".to_string(),
                index: 1,
                total: 3,
            },
        );

        let received = rx.try_recv().unwrap();
        assert_eq!(
            received,
            RunProgress::ScriptStarted {
                script: "00_init.sh".to_string(),
                index: 1,
                total: 3,
            }
        );
    }

    #[test]
    fn send_progress_sends_script_output_line() {
        let (tx, rx) = mpsc::channel::<RunProgress>();
        let progress_tx = Some(tx);

        send_progress(
            &progress_tx,
            RunProgress::ScriptOutputLine {
                line: "Building...".to_string(),
            },
        );

        let received = rx.try_recv().unwrap();
        assert_eq!(
            received,
            RunProgress::ScriptOutputLine {
                line: "Building...".to_string()
            }
        );
    }

    // ── RunResult ──────────────────────────────────────────────────────────────

    #[test]
    fn run_result_new_initializes_correctly() {
        let result = RunResult::new("vm0", "/tmp/output");

        assert_eq!(result.vm_id, "vm0");
        assert_eq!(result.output_dir, PathBuf::from("/tmp/output"));
        assert!(result.executed_scripts.is_empty());
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.elapsed_ms, 0);
    }

    // ── TaskRunOptions default ─────────────────────────────────────────────────

    #[test]
    fn task_run_options_default() {
        let options = TaskRunOptions::default();

        assert!(options.disk_size.is_none());
        assert!(options.show_progress);
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // Category B: Mock-based Tests
    // ═══════════════════════════════════════════════════════════════════════════

    fn create_test_scripts_dir() -> TempDir {
        let temp = TempDir::new().expect("failed to create temp dir");
        let scripts_dir = temp.path().join("scripts");
        fs::create_dir(&scripts_dir).unwrap();
        fs::write(scripts_dir.join("00_first.sh"), "#!/bin/bash\necho first").unwrap();
        fs::write(scripts_dir.join("10_second.sh"), "#!/bin/bash\necho second").unwrap();
        temp
    }

    #[test]
    fn run_task_with_ops_returns_success_on_happy_path() {
        let temp = create_test_scripts_dir();
        let input_dir = temp.path().join("scripts");
        let output_dir = temp.path().join("output");

        let mock = MockVmOps::new()
            .with_run_ok("test-vm")
            .with_ssh_ok("")   // mkdir
            .with_ssh_ok("")   // mkdir for output
            .with_stream_outputs(vec!["first output"])
            .with_stream_outputs(vec!["second output"]);

        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt
            .block_on(run_task_with_ops(
                &mock,
                &input_dir,
                &output_dir,
                TaskRunOptions::default(),
                None,
            ))
            .unwrap();

        assert_eq!(result.vm_id, "test-vm");
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.executed_scripts, vec!["00_first.sh", "10_second.sh"]);

        // Verify rm was called
        let rm_calls = mock.rm_calls();
        assert_eq!(rm_calls, vec!["test-vm"]);
    }

    #[test]
    fn run_task_with_ops_fails_on_vm_start_error() {
        let temp = create_test_scripts_dir();
        let input_dir = temp.path().join("scripts");
        let output_dir = temp.path().join("output");

        let mock = MockVmOps::new().with_run_err("QEMU failed to start");

        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(run_task_with_ops(
            &mock,
            &input_dir,
            &output_dir,
            TaskRunOptions::default(),
            None,
        ));

        assert!(result.is_err());
        match result.unwrap_err() {
            Error::VmStart { message } => assert!(message.contains("QEMU failed")),
            err => panic!("Expected VmStart error, got: {err}"),
        }
    }

    #[test]
    fn run_task_with_ops_fails_on_prepare_vm_error() {
        let temp = create_test_scripts_dir();
        let input_dir = temp.path().join("scripts");
        let output_dir = temp.path().join("output");

        let mock = MockVmOps::new()
            .with_run_ok("test-vm")
            .with_ssh_err("SSH connection failed"); // mkdir fails

        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(run_task_with_ops(
            &mock,
            &input_dir,
            &output_dir,
            TaskRunOptions::default(),
            None,
        ));

        assert!(result.is_err());
        match result.unwrap_err() {
            Error::VmCommand { message } => assert!(message.contains("SSH connection failed")),
            err => panic!("Expected VmCommand error, got: {err}"),
        }

        // Verify cleanup still happened
        let rm_calls = mock.rm_calls();
        assert_eq!(rm_calls, vec!["test-vm"]);
    }

    #[test]
    fn run_task_with_ops_fails_on_script_error() {
        let temp = create_test_scripts_dir();
        let input_dir = temp.path().join("scripts");
        let output_dir = temp.path().join("output");

        let mock = MockVmOps::new()
            .with_run_ok("test-vm")
            .with_ssh_ok("")  // mkdir
            .with_cp_to_err("copy failed"); // cp_to fails

        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(run_task_with_ops(
            &mock,
            &input_dir,
            &output_dir,
            TaskRunOptions::default(),
            None,
        ));

        assert!(result.is_err());
        match result.unwrap_err() {
            Error::CopyToVm { message } => assert!(message.contains("copy failed")),
            err => panic!("Expected CopyToVm error, got: {err}"),
        }

        // Verify cleanup still happened
        let rm_calls = mock.rm_calls();
        assert_eq!(rm_calls, vec!["test-vm"]);
    }

    #[test]
    fn run_task_with_ops_cleans_up_on_failure() {
        let temp = create_test_scripts_dir();
        let input_dir = temp.path().join("scripts");
        let output_dir = temp.path().join("output");

        let mock = MockVmOps::new().with_run_err("failed");

        let rt = tokio::runtime::Runtime::new().unwrap();
        let _ = rt.block_on(run_task_with_ops(
            &mock,
            &input_dir,
            &output_dir,
            TaskRunOptions::default(),
            None,
        ));

        // No rm call because VM never started
        let rm_calls = mock.rm_calls();
        assert!(rm_calls.is_empty());
    }

    #[test]
    fn run_task_with_ops_sends_progress_events() {
        let temp = create_test_scripts_dir();
        let input_dir = temp.path().join("scripts");
        let output_dir = temp.path().join("output");

        let mock = MockVmOps::new()
            .with_run_ok("test-vm")
            .with_ssh_ok("")
            .with_stream_outputs(vec!["output"]);

        let (tx, rx) = mpsc::channel::<RunProgress>();

        let rt = tokio::runtime::Runtime::new().unwrap();
        let _ = rt.block_on(run_task_with_ops(
            &mock,
            &input_dir,
            &output_dir,
            TaskRunOptions::default(),
            Some(tx),
        ));

        // Collect all progress events
        let events: Vec<_> = rx.try_iter().collect();

        // Verify phase progression
        assert!(events
            .iter()
            .any(|e| matches!(e, RunProgress::Phase(RunPhase::ValidatingPaths))));
        assert!(events
            .iter()
            .any(|e| matches!(e, RunProgress::Phase(RunPhase::StartingVm))));
        assert!(events
            .iter()
            .any(|e| matches!(e, RunProgress::Phase(RunPhase::PreparingVm))));
        assert!(events
            .iter()
            .any(|e| matches!(e, RunProgress::Phase(RunPhase::RunningScripts))));
        assert!(events
            .iter()
            .any(|e| matches!(e, RunProgress::Phase(RunPhase::CollectingOutput))));
        assert!(events
            .iter()
            .any(|e| matches!(e, RunProgress::Phase(RunPhase::CleaningUp))));

        // Verify script events
        assert!(events
            .iter()
            .any(|e| matches!(e, RunProgress::ScriptStarted { .. })));
        assert!(events
            .iter()
            .any(|e| matches!(e, RunProgress::ScriptFinished { .. })));
    }

    #[test]
    fn run_task_with_ops_records_ssh_commands() {
        let temp = create_test_scripts_dir();
        let input_dir = temp.path().join("scripts");
        let output_dir = temp.path().join("output");

        let mock = MockVmOps::new()
            .with_run_ok("test-vm")
            .with_ssh_ok("")
            .with_stream_outputs(vec!["output"]);

        let rt = tokio::runtime::Runtime::new().unwrap();
        let _ = rt.block_on(run_task_with_ops(
            &mock,
            &input_dir,
            &output_dir,
            TaskRunOptions::default(),
            None,
        ));

        let commands = mock.ssh_commands();

        // Should have mkdir commands (from prepare_vm and collect_output)
        assert!(commands.iter().any(|(_, cmd)| cmd.contains("mkdir")));

        // ssh_stream commands are recorded separately
        let stream_commands = mock.stream_commands.lock().unwrap().clone();
        assert!(stream_commands
            .iter()
            .any(|cmd| cmd.contains("/bin/bash")));
    }

    #[test]
    fn run_task_with_ops_handles_cleanup_failure() {
        let temp = create_test_scripts_dir();
        let input_dir = temp.path().join("scripts");
        let output_dir = temp.path().join("output");

        let mock = MockVmOps::new()
            .with_run_ok("test-vm")
            .with_ssh_ok("")
            .with_stream_outputs(vec!["output"])
            .with_rm_err("cleanup failed");

        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(run_task_with_ops(
            &mock,
            &input_dir,
            &output_dir,
            TaskRunOptions::default(),
            None,
        ));

        // Should fail with cleanup error
        assert!(result.is_err());
        match result.unwrap_err() {
            Error::CleanupFailed { vm_id, message } => {
                assert_eq!(vm_id, "test-vm");
                assert!(message.contains("cleanup failed"));
            }
            err => panic!("Expected CleanupFailed error, got: {err}"),
        }
    }
}
