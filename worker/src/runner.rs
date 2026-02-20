use std::collections::HashSet;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::time::Instant;

use crate::vm_ops::{VmOps, VmOptions};
use crate::{ChainRunResult, ChainStepResult, Error, RunResult};
use task::{LoadedTask, load_task};

const VM_WORK_DIR: &str = "/tmp/vmize-worker/work";
const VM_OUTPUT_DIR: &str = "/tmp/vmize-worker/out";
const VM_LOGS_DIR: &str = "/tmp/vmize-worker/logs";
static OVERLAY_DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone)]
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

pub async fn run_loaded_task(
    task: &LoadedTask,
    options: TaskRunOptions,
) -> Result<RunResult, Error> {
    run_loaded_task_with_progress(task, options, None).await
}

pub async fn run_loaded_task_with_progress(
    task: &LoadedTask,
    options: TaskRunOptions,
    progress_tx: Option<mpsc::Sender<RunProgress>>,
) -> Result<RunResult, Error> {
    run_loaded_task_with_ops(&crate::vm_ops::RealVmOps, task, options, progress_tx).await
}

pub fn run_loaded_task_blocking(
    task: &LoadedTask,
    options: TaskRunOptions,
) -> Result<RunResult, Error> {
    run_loaded_task_blocking_with_progress(task, options, None)
}

pub fn run_loaded_task_blocking_with_progress(
    task: &LoadedTask,
    options: TaskRunOptions,
    progress_tx: Option<mpsc::Sender<RunProgress>>,
) -> Result<RunResult, Error> {
    if tokio::runtime::Handle::try_current().is_ok() {
        return Err(Error::BlockingInAsyncContext);
    }

    let runtime = tokio::runtime::Runtime::new().map_err(|err| Error::Runtime {
        message: err.to_string(),
    })?;

    runtime.block_on(run_loaded_task_with_progress(task, options, progress_tx))
}

pub fn run_task_chain_blocking(
    start_task_dir: &Path,
    options: TaskRunOptions,
) -> Result<ChainRunResult, Error> {
    let mut chain_result = ChainRunResult::default();
    let mut visited = HashSet::new();
    let mut current_task_dir = fs::canonicalize(start_task_dir).map_err(|err| Error::Runtime {
        message: format!(
            "Failed to resolve task directory {}: {err}",
            start_task_dir.display()
        ),
    })?;
    let mut pending_handoff: Option<Vec<HandoffArtifact>> = None;

    loop {
        if !visited.insert(current_task_dir.clone()) {
            return Err(Error::Runtime {
                message: format!(
                    "Task chain cycle detected at {}",
                    current_task_dir.display()
                ),
            });
        }

        let loaded = load_task(&current_task_dir).map_err(|err| Error::Runtime {
            message: format!("Failed to load task {}: {err}", current_task_dir.display()),
        })?;

        let mut overlay_input_dir: Option<PathBuf> = None;
        let task_to_run = if let Some(handoff) = pending_handoff.take() {
            let overlay = create_overlay_input_dir(&loaded.input_dir, &handoff)?;
            overlay_input_dir = Some(overlay.clone());
            LoadedTask {
                input_dir: overlay,
                ..loaded.clone()
            }
        } else {
            loaded.clone()
        };

        let run_result = run_loaded_task_blocking(&task_to_run, options.clone());
        if let Some(overlay) = overlay_input_dir.as_deref() {
            let _ = fs::remove_dir_all(overlay);
        }
        let run_result = run_result.map_err(|err| Error::Runtime {
            message: format!("Task chain failed at {}: {err}", current_task_dir.display()),
        })?;

        let mut handoff_artifacts = Vec::new();
        let next_task_dir = if let Some(next_task_dir) = loaded.definition.next_task_dir.as_deref()
        {
            let artifacts = collect_handoff_artifacts(&loaded)?;
            handoff_artifacts = artifacts
                .iter()
                .map(|artifact| artifact.relative_path.to_string_lossy().to_string())
                .collect();
            let next_dir = resolve_next_task_dir(&current_task_dir, next_task_dir)?;
            pending_handoff = Some(artifacts);
            Some(next_dir)
        } else {
            None
        };

        chain_result.steps.push(ChainStepResult {
            task_dir: current_task_dir.clone(),
            task_name: loaded.definition.name.clone(),
            handoff_artifacts,
            run_result,
        });

        match next_task_dir {
            Some(next_dir) => {
                if visited.contains(&next_dir) {
                    return Err(Error::Runtime {
                        message: format!(
                            "Task chain cycle detected: {} -> {}",
                            current_task_dir.display(),
                            next_dir.display()
                        ),
                    });
                }
                current_task_dir = next_dir;
            }
            None => break,
        }
    }

    Ok(chain_result)
}

// ═══════════════════════════════════════════════════════════════════════════════
// Core Implementation (uses VmOps trait)
// ═══════════════════════════════════════════════════════════════════════════════

pub async fn run_loaded_task_with_ops<V: VmOps + ?Sized>(
    vm_ops: &V,
    task: &LoadedTask,
    options: TaskRunOptions,
    progress_tx: Option<mpsc::Sender<RunProgress>>,
) -> Result<RunResult, Error> {
    let start = Instant::now();

    let commands = &task.definition.commands;
    let input_dir = &task.input_dir;
    let output_dir = &task.output_dir;
    let logs_dir = &task.logs_dir;
    let effective_disk_size = options
        .disk_size
        .clone()
        .or_else(|| task.definition.disk_size.clone());

    send_progress(&progress_tx, RunProgress::Phase(RunPhase::StartingVm));
    let record = vm_ops
        .run(VmOptions {
            disk_size: effective_disk_size,
            show_progress: options.show_progress,
        })
        .await
        .map_err(|err| Error::VmStart {
            message: err.to_string(),
        })?;

    let vm_id = record.id.clone();
    let mut result = RunResult::new(&vm_id, output_dir, logs_dir);

    let run_error = match prepare_vm_with_ops(vm_ops, &vm_id, input_dir, &progress_tx).await {
        Ok(()) => {
            send_progress(&progress_tx, RunProgress::Phase(RunPhase::RunningScripts));
            execute_commands_with_ops(vm_ops, commands, &record, &progress_tx, &mut result)
        }
        Err(err) => Some(err),
    };

    send_progress(&progress_tx, RunProgress::Phase(RunPhase::CollectingOutput));

    let collect_error =
        collect_output_with_ops(vm_ops, &vm_id, task, &mut result, &run_error).await;

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

    // Create out/ and logs/ but NOT work/ — scp will create work/ as a copy of input_dir
    vm_ops
        .ssh(
            vm_id,
            &format!(
                "mkdir -p {} {}",
                shell_quote(VM_OUTPUT_DIR),
                shell_quote(VM_LOGS_DIR)
            ),
        )
        .await
        .map_err(|err| Error::VmCommand {
            message: err.to_string(),
        })?;

    // Copy input_dir as /tmp/vmize-worker/work (contents land directly in work/)
    vm_ops
        .cp_to(vm_id, path_to_str(input_dir)?, VM_WORK_DIR, true)
        .map_err(|err| Error::CopyToVm {
            message: err.to_string(),
        })
}

fn execute_commands_with_ops<V: VmOps + ?Sized>(
    vm_ops: &V,
    commands: &[String],
    record: &vm::VmRecord,
    progress_tx: &Option<mpsc::Sender<RunProgress>>,
    result: &mut RunResult,
) -> Option<Error> {
    for cmd in commands {
        let next_index = result.executed_commands.len() + 1;
        send_progress(
            progress_tx,
            RunProgress::ScriptStarted {
                script: cmd.clone(),
                index: next_index,
                total: commands.len(),
            },
        );

        // basename of cmd (commands are relative paths, typically just a filename)
        let cmd_basename = Path::new(cmd)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(cmd.as_str());
        let log_path = format!("{}/{}.log", VM_LOGS_DIR, cmd_basename);
        let command = format!(
            "cd {} && /bin/bash {} 2>&1 | tee {}",
            shell_quote(VM_WORK_DIR),
            shell_quote(cmd),
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
                script: cmd.clone(),
                message: err.to_string(),
            });
        }

        result.executed_commands.push(cmd.clone());
        let finished_index = result.executed_commands.len();
        send_progress(
            progress_tx,
            RunProgress::ScriptFinished {
                script: cmd.clone(),
                index: finished_index,
                total: commands.len(),
            },
        );
    }

    None
}

async fn collect_output_with_ops<V: VmOps + ?Sized>(
    vm_ops: &V,
    vm_id: &str,
    task: &LoadedTask,
    result: &mut RunResult,
    run_error: &Option<Error>,
) -> Option<Error> {
    let output_dir_str = match path_to_str(&task.output_dir) {
        Ok(s) => s,
        Err(err) => return Some(err),
    };
    let logs_dir_str = match path_to_str(&task.logs_dir) {
        Ok(s) => s,
        Err(err) => return Some(err),
    };

    // Always collect logs (best-effort, ignore errors)
    let _ = vm_ops.cp_from(vm_id, &format!("{}/*", VM_LOGS_DIR), logs_dir_str, false);

    // Only collect output if commands ran successfully
    if run_error.is_some() {
        return None;
    }

    match &task.definition.artifacts {
        Some(artifacts) if !artifacts.is_empty() => {
            for artifact in artifacts {
                let remote_path = format!("{}/{}", VM_OUTPUT_DIR, artifact);
                if let Err(err) = vm_ops.cp_from(vm_id, &remote_path, output_dir_str, false) {
                    return Some(Error::CopyFromVm {
                        message: err.to_string(),
                    });
                }

                let local_path = task.output_dir.join(artifact);
                if !local_path.exists() {
                    return Some(Error::MissingArtifact {
                        file: artifact.clone(),
                    });
                }
                result.collected_artifacts.push(artifact.clone());
            }
        }
        _ => {
            // No artifacts specified: copy entire out/ directory
            if let Err(err) =
                vm_ops.cp_from(vm_id, &format!("{}/*", VM_OUTPUT_DIR), output_dir_str, true)
            {
                return Some(Error::CopyFromVm {
                    message: err.to_string(),
                });
            }
        }
    }

    None
}

#[derive(Debug, Clone)]
struct HandoffArtifact {
    relative_path: PathBuf,
    source_path: PathBuf,
}

fn collect_handoff_artifacts(task: &LoadedTask) -> Result<Vec<HandoffArtifact>, Error> {
    let artifacts = task
        .definition
        .artifacts
        .as_ref()
        .filter(|artifacts| !artifacts.is_empty())
        .ok_or_else(|| Error::Runtime {
            message: format!(
                "Task {} declares next_task_dir but has no artifacts to hand off",
                task.output_dir
                    .parent()
                    .unwrap_or_else(|| Path::new("<unknown>"))
                    .display()
            ),
        })?;

    let mut handoff = Vec::with_capacity(artifacts.len());
    for artifact in artifacts {
        let relative_path = parse_artifact_relative_path(artifact)?;
        let source_path = task.output_dir.join(&relative_path);
        if !source_path.exists() {
            return Err(Error::MissingArtifact {
                file: artifact.clone(),
            });
        }

        handoff.push(HandoffArtifact {
            relative_path,
            source_path,
        });
    }

    Ok(handoff)
}

fn resolve_next_task_dir(current_task_dir: &Path, next_task_dir: &str) -> Result<PathBuf, Error> {
    let relative = parse_next_task_relative_path(next_task_dir)?;
    let candidate = current_task_dir.join(relative);
    let canonical = fs::canonicalize(&candidate).map_err(|err| Error::Runtime {
        message: format!(
            "Failed to resolve next_task_dir '{next_task_dir}' from {}: {err}",
            current_task_dir.display()
        ),
    })?;

    if !canonical.join("task.json").is_file() {
        return Err(Error::Runtime {
            message: format!(
                "Resolved next task directory {} does not contain task.json",
                canonical.display()
            ),
        });
    }

    Ok(canonical)
}

fn create_overlay_input_dir(
    input_dir: &Path,
    handoff: &[HandoffArtifact],
) -> Result<PathBuf, Error> {
    let overlay_dir = next_overlay_input_dir();
    fs::create_dir_all(&overlay_dir)?;

    let setup_result = (|| -> Result<(), Error> {
        copy_directory_contents(input_dir, &overlay_dir)?;

        for artifact in handoff {
            let destination = overlay_dir.join(&artifact.relative_path);
            if destination.exists() {
                return Err(Error::Runtime {
                    message: format!(
                        "Artifact handoff conflict: {} already exists in downstream input",
                        destination.display()
                    ),
                });
            }

            copy_path_recursive(&artifact.source_path, &destination)?;
        }

        Ok(())
    })();

    if let Err(err) = setup_result {
        let _ = fs::remove_dir_all(&overlay_dir);
        return Err(err);
    }

    Ok(overlay_dir)
}

fn next_overlay_input_dir() -> PathBuf {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let counter = OVERLAY_DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "vmize-chain-input-{}-{}-{}",
        std::process::id(),
        now.as_nanos(),
        counter
    ))
}

fn copy_directory_contents(src: &Path, dest: &Path) -> Result<(), Error> {
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let source = entry.path();
        let destination = dest.join(entry.file_name());
        copy_path_recursive(&source, &destination)?;
    }
    Ok(())
}

fn copy_path_recursive(src: &Path, dest: &Path) -> Result<(), Error> {
    let metadata = fs::symlink_metadata(src)?;
    if metadata.is_dir() {
        fs::create_dir_all(dest)?;
        for entry in fs::read_dir(src)? {
            let entry = entry?;
            let source_child = entry.path();
            let dest_child = dest.join(entry.file_name());
            copy_path_recursive(&source_child, &dest_child)?;
        }
        return Ok(());
    }

    if metadata.is_file() {
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(src, dest)?;
        return Ok(());
    }

    Err(Error::Runtime {
        message: format!("Unsupported artifact type at {}", src.display()),
    })
}

fn parse_next_task_relative_path(value: &str) -> Result<PathBuf, Error> {
    parse_relative_path(value, "next_task_dir", true)
}

fn parse_artifact_relative_path(value: &str) -> Result<PathBuf, Error> {
    parse_relative_path(value, "artifact", false)
}

fn parse_relative_path(value: &str, field: &str, allow_parent: bool) -> Result<PathBuf, Error> {
    if value.trim().is_empty() {
        return Err(Error::Runtime {
            message: format!("{field} path must not be empty"),
        });
    }

    let path = Path::new(value);
    if path.is_absolute() {
        return Err(Error::Runtime {
            message: format!("{field} path must be relative: {value}"),
        });
    }

    if path.components().any(|component| {
        !matches!(component, Component::Normal(_) | Component::ParentDir)
            || (!allow_parent && matches!(component, Component::ParentDir))
    }) {
        return Err(Error::Runtime {
            message: format!(
                "{field} path must not contain '.', root, or drive-prefix components{}: {value}",
                if allow_parent {
                    ""
                } else {
                    ", and must not contain '..'"
                }
            ),
        });
    }

    Ok(path.to_path_buf())
}

// ═══════════════════════════════════════════════════════════════════════════════
// Helper Functions
// ═══════════════════════════════════════════════════════════════════════════════

fn path_to_str(path: &Path) -> Result<&str, Error> {
    path.to_str().ok_or_else(|| Error::NonUtf8Path {
        path: path.to_path_buf(),
    })
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

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
    use task::TaskDefinition;
    use tempfile::TempDir;

    fn create_test_loaded_task(commands: &[&str]) -> (TempDir, LoadedTask) {
        let temp = TempDir::new().expect("failed to create temp dir");
        let input_dir = temp.path().join("input");
        fs::create_dir(&input_dir).unwrap();
        for cmd in commands {
            fs::write(input_dir.join(cmd), "#!/bin/bash\necho test").unwrap();
        }
        let output_dir = temp.path().join("output");
        fs::create_dir(&output_dir).unwrap();
        let logs_dir = output_dir.join("logs");
        fs::create_dir(&logs_dir).unwrap();

        let task = LoadedTask {
            definition: TaskDefinition {
                name: Some("test".to_string()),
                description: None,
                disk_size: None,
                commands: commands.iter().map(|s| s.to_string()).collect(),
                artifacts: None,
                next_task_dir: None,
            },
            input_dir,
            output_dir,
            logs_dir,
        };
        (temp, task)
    }

    // ── shell_quote ────────────────────────────────────────────────────────────

    #[test]
    fn shell_quote_wraps_in_single_quotes() {
        assert_eq!(shell_quote("hello"), "'hello'");
    }

    #[test]
    fn shell_quote_escapes_embedded_single_quotes() {
        assert_eq!(shell_quote("it's"), "'it'\"'\"'s'");
    }

    #[test]
    fn shell_quote_handles_empty_string() {
        assert_eq!(shell_quote(""), "''");
    }

    #[test]
    fn shell_quote_handles_special_chars() {
        assert_eq!(shell_quote("$HOME"), "'$HOME'");
        assert_eq!(shell_quote("echo `cmd`"), "'echo `cmd`'");
        assert_eq!(shell_quote("a\\b"), "'a\\b'");
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

    // ── send_progress ──────────────────────────────────────────────────────────

    #[test]
    fn send_progress_sends_event_when_channel_present() {
        let (tx, rx) = mpsc::channel::<RunProgress>();
        let progress_tx = Some(tx);

        send_progress(&progress_tx, RunProgress::Phase(RunPhase::StartingVm));

        let received = rx.try_recv().unwrap();
        assert_eq!(received, RunProgress::Phase(RunPhase::StartingVm));
    }

    #[test]
    fn send_progress_does_nothing_when_channel_is_none() {
        let progress_tx: Option<mpsc::Sender<RunProgress>> = None;
        send_progress(&progress_tx, RunProgress::Phase(RunPhase::StartingVm));
    }

    // ── RunResult ──────────────────────────────────────────────────────────────

    #[test]
    fn run_result_new_initializes_correctly() {
        let result = RunResult::new("vm0", "/tmp/output", "/tmp/output/logs");

        assert_eq!(result.vm_id, "vm0");
        assert_eq!(result.output_dir, PathBuf::from("/tmp/output"));
        assert_eq!(result.logs_dir, PathBuf::from("/tmp/output/logs"));
        assert!(result.executed_commands.is_empty());
        assert!(result.collected_artifacts.is_empty());
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

    // ── chain path parsing and handoff ─────────────────────────────────────────

    #[test]
    fn parse_next_task_relative_path_accepts_nested_relative_paths() {
        let parsed = parse_next_task_relative_path("next/task2").unwrap();
        assert_eq!(parsed, PathBuf::from("next/task2"));
    }

    #[test]
    fn parse_next_task_relative_path_allows_parent_component() {
        let parsed = parse_next_task_relative_path("../task2").unwrap();
        assert_eq!(parsed, PathBuf::from("../task2"));
    }

    #[test]
    fn parse_artifact_relative_path_rejects_parent_component() {
        let err = parse_artifact_relative_path("../handoff.txt").unwrap_err();
        match err {
            Error::Runtime { message } => assert!(message.contains("must not contain")),
            other => panic!("Expected Runtime error, got: {other}"),
        }
    }

    #[test]
    fn create_overlay_input_dir_copies_input_and_handoff_artifacts() {
        let temp = TempDir::new().unwrap();
        let input_dir = temp.path().join("input");
        fs::create_dir_all(&input_dir).unwrap();
        fs::write(input_dir.join("00_run.sh"), "#!/usr/bin/env bash\n").unwrap();
        fs::create_dir_all(input_dir.join("assets")).unwrap();
        fs::write(input_dir.join("assets").join("base.txt"), "base").unwrap();

        let output_dir = temp.path().join("output");
        fs::create_dir_all(output_dir.join("nested")).unwrap();
        fs::write(output_dir.join("nested").join("handoff.txt"), "handoff").unwrap();

        let handoff = vec![HandoffArtifact {
            relative_path: PathBuf::from("nested/handoff.txt"),
            source_path: output_dir.join("nested").join("handoff.txt"),
        }];

        let overlay = create_overlay_input_dir(&input_dir, &handoff).unwrap();

        assert!(overlay.join("00_run.sh").exists());
        assert_eq!(
            fs::read_to_string(overlay.join("assets").join("base.txt")).unwrap(),
            "base"
        );
        assert_eq!(
            fs::read_to_string(overlay.join("nested").join("handoff.txt")).unwrap(),
            "handoff"
        );

        let _ = fs::remove_dir_all(&overlay);
    }

    #[test]
    fn create_overlay_input_dir_fails_on_conflict() {
        let temp = TempDir::new().unwrap();
        let input_dir = temp.path().join("input");
        fs::create_dir_all(&input_dir).unwrap();
        fs::write(input_dir.join("handoff.txt"), "existing").unwrap();

        let output_dir = temp.path().join("output");
        fs::create_dir_all(&output_dir).unwrap();
        fs::write(output_dir.join("handoff.txt"), "new").unwrap();

        let handoff = vec![HandoffArtifact {
            relative_path: PathBuf::from("handoff.txt"),
            source_path: output_dir.join("handoff.txt"),
        }];

        let err = create_overlay_input_dir(&input_dir, &handoff).unwrap_err();
        match err {
            Error::Runtime { message } => assert!(message.contains("handoff conflict")),
            other => panic!("Expected Runtime error, got: {other}"),
        }
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // Mock-based Tests
    // ═══════════════════════════════════════════════════════════════════════════

    #[test]
    fn run_loaded_task_with_ops_returns_success_on_happy_path() {
        let (_temp, task) = create_test_loaded_task(&["00_first.sh", "10_second.sh"]);

        let mock = MockVmOps::new()
            .with_run_ok("test-vm")
            .with_ssh_ok("") // mkdir out + logs
            .with_stream_outputs(vec!["first output"])
            .with_stream_outputs(vec!["second output"]);

        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt
            .block_on(run_loaded_task_with_ops(
                &mock,
                &task,
                TaskRunOptions::default(),
                None,
            ))
            .unwrap();

        assert_eq!(result.vm_id, "test-vm");
        assert_eq!(result.exit_code, 0);
        assert_eq!(
            result.executed_commands,
            vec!["00_first.sh", "10_second.sh"]
        );

        let rm_calls = mock.rm_calls();
        assert_eq!(rm_calls, vec!["test-vm"]);
    }

    #[test]
    fn run_loaded_task_with_ops_fails_on_vm_start_error() {
        let (_temp, task) = create_test_loaded_task(&["00_first.sh"]);

        let mock = MockVmOps::new().with_run_err("QEMU failed to start");

        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(run_loaded_task_with_ops(
            &mock,
            &task,
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
    fn run_loaded_task_with_ops_fails_on_prepare_vm_error() {
        let (_temp, task) = create_test_loaded_task(&["00_first.sh"]);

        let mock = MockVmOps::new()
            .with_run_ok("test-vm")
            .with_ssh_err("SSH connection failed"); // mkdir fails

        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(run_loaded_task_with_ops(
            &mock,
            &task,
            TaskRunOptions::default(),
            None,
        ));

        assert!(result.is_err());
        match result.unwrap_err() {
            Error::VmCommand { message } => assert!(message.contains("SSH connection failed")),
            err => panic!("Expected VmCommand error, got: {err}"),
        }

        let rm_calls = mock.rm_calls();
        assert_eq!(rm_calls, vec!["test-vm"]);
    }

    #[test]
    fn run_loaded_task_with_ops_fails_on_cp_to_error() {
        let (_temp, task) = create_test_loaded_task(&["00_first.sh"]);

        let mock = MockVmOps::new()
            .with_run_ok("test-vm")
            .with_ssh_ok("") // mkdir
            .with_cp_to_err("copy failed");

        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(run_loaded_task_with_ops(
            &mock,
            &task,
            TaskRunOptions::default(),
            None,
        ));

        assert!(result.is_err());
        match result.unwrap_err() {
            Error::CopyToVm { message } => assert!(message.contains("copy failed")),
            err => panic!("Expected CopyToVm error, got: {err}"),
        }

        let rm_calls = mock.rm_calls();
        assert_eq!(rm_calls, vec!["test-vm"]);
    }

    #[test]
    fn run_loaded_task_with_ops_cleans_up_on_vm_start_failure() {
        let (_temp, task) = create_test_loaded_task(&["00_first.sh"]);

        let mock = MockVmOps::new().with_run_err("failed");

        let rt = tokio::runtime::Runtime::new().unwrap();
        let _ = rt.block_on(run_loaded_task_with_ops(
            &mock,
            &task,
            TaskRunOptions::default(),
            None,
        ));

        // No rm call because VM never started
        assert!(mock.rm_calls().is_empty());
    }

    #[test]
    fn run_loaded_task_with_ops_sends_progress_events() {
        let (_temp, task) = create_test_loaded_task(&["00_first.sh"]);

        let mock = MockVmOps::new()
            .with_run_ok("test-vm")
            .with_ssh_ok("")
            .with_stream_outputs(vec!["output"]);

        let (tx, rx) = mpsc::channel::<RunProgress>();

        let rt = tokio::runtime::Runtime::new().unwrap();
        let _ = rt.block_on(run_loaded_task_with_ops(
            &mock,
            &task,
            TaskRunOptions::default(),
            Some(tx),
        ));

        let events: Vec<_> = rx.try_iter().collect();

        assert!(
            events
                .iter()
                .any(|e| matches!(e, RunProgress::Phase(RunPhase::StartingVm)))
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, RunProgress::Phase(RunPhase::PreparingVm)))
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, RunProgress::Phase(RunPhase::RunningScripts)))
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, RunProgress::Phase(RunPhase::CollectingOutput)))
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, RunProgress::Phase(RunPhase::CleaningUp)))
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, RunProgress::ScriptStarted { .. }))
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, RunProgress::ScriptFinished { .. }))
        );
    }

    #[test]
    fn run_loaded_task_with_ops_handles_cleanup_failure() {
        let (_temp, task) = create_test_loaded_task(&["00_first.sh"]);

        let mock = MockVmOps::new()
            .with_run_ok("test-vm")
            .with_ssh_ok("")
            .with_stream_outputs(vec!["output"])
            .with_rm_err("cleanup failed");

        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(run_loaded_task_with_ops(
            &mock,
            &task,
            TaskRunOptions::default(),
            None,
        ));

        assert!(result.is_err());
        match result.unwrap_err() {
            Error::CleanupFailed { vm_id, message } => {
                assert_eq!(vm_id, "test-vm");
                assert!(message.contains("cleanup failed"));
            }
            err => panic!("Expected CleanupFailed error, got: {err}"),
        }
    }

    #[test]
    fn run_loaded_task_with_ops_records_ssh_commands() {
        let (_temp, task) = create_test_loaded_task(&["00_first.sh"]);

        let mock = MockVmOps::new()
            .with_run_ok("test-vm")
            .with_ssh_ok("")
            .with_stream_outputs(vec!["output"]);

        let rt = tokio::runtime::Runtime::new().unwrap();
        let _ = rt.block_on(run_loaded_task_with_ops(
            &mock,
            &task,
            TaskRunOptions::default(),
            None,
        ));

        let commands = mock.ssh_commands();
        assert!(commands.iter().any(|(_, cmd)| cmd.contains("mkdir")));

        let stream_commands = mock.stream_commands.lock().unwrap().clone();
        assert!(stream_commands.iter().any(|cmd| cmd.contains("/bin/bash")));
    }

    #[test]
    fn run_loaded_task_with_ops_uses_task_definition_disk_size_when_option_missing() {
        let (_temp, mut task) = create_test_loaded_task(&["00_first.sh"]);
        task.definition.disk_size = Some("20G".to_string());

        let mock = MockVmOps::new()
            .with_run_ok("test-vm")
            .with_ssh_ok("")
            .with_stream_outputs(vec!["output"]);

        let rt = tokio::runtime::Runtime::new().unwrap();
        let _ = rt
            .block_on(run_loaded_task_with_ops(
                &mock,
                &task,
                TaskRunOptions::default(),
                None,
            ))
            .unwrap();

        let run_calls = mock.run_calls();
        assert_eq!(run_calls.len(), 1);
        assert_eq!(run_calls[0].disk_size, Some("20G".to_string()));
    }

    #[test]
    fn run_loaded_task_with_ops_prefers_option_disk_size_over_task_definition() {
        let (_temp, mut task) = create_test_loaded_task(&["00_first.sh"]);
        task.definition.disk_size = Some("20G".to_string());

        let mock = MockVmOps::new()
            .with_run_ok("test-vm")
            .with_ssh_ok("")
            .with_stream_outputs(vec!["output"]);

        let rt = tokio::runtime::Runtime::new().unwrap();
        let _ = rt
            .block_on(run_loaded_task_with_ops(
                &mock,
                &task,
                TaskRunOptions {
                    disk_size: Some("30G".to_string()),
                    ..Default::default()
                },
                None,
            ))
            .unwrap();

        let run_calls = mock.run_calls();
        assert_eq!(run_calls.len(), 1);
        assert_eq!(run_calls[0].disk_size, Some("30G".to_string()));
    }
}
