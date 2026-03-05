use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::time::Instant;

use crate::vm_ops::{VmOps, VmOptions};
use crate::{ChainRunResult, ChainStepResult, Error, RunResult};
use task::{LoadedTask, TaskVmBoot, TaskVmMountMode, load_task};

const VM_WORK_DIR: &str = "/tmp/vmize-worker/work";
const VM_OUTPUT_DIR: &str = "/tmp/vmize-worker/out";
const VM_LOGS_DIR: &str = "/tmp/vmize-worker/logs";
static OVERLAY_DIR_COUNTER: AtomicU64 = AtomicU64::new(0);
static ROOTFS_CLONE_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KernelConfigValue {
    Yes,
    Module,
    No,
}

impl KernelConfigValue {
    fn from_char(value: char) -> Option<Self> {
        match value {
            'y' => Some(Self::Yes),
            'm' => Some(Self::Module),
            'n' => Some(Self::No),
            _ => None,
        }
    }

    fn as_char(self) -> char {
        match self {
            Self::Yes => 'y',
            Self::Module => 'm',
            Self::No => 'n',
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct KernelConfigRequirement {
    symbol: String,
    expected: KernelConfigValue,
}

#[derive(Debug, Clone)]
struct VmLaunchOptions {
    vm_options: VmOptions,
    temporary_rootfs_path: Option<PathBuf>,
}

#[derive(Debug)]
struct TemporaryRootfsCleanup {
    path: Option<PathBuf>,
}

impl TemporaryRootfsCleanup {
    fn new(path: Option<PathBuf>) -> Self {
        Self { path }
    }
}

impl Drop for TemporaryRootfsCleanup {
    fn drop(&mut self) {
        cleanup_temporary_rootfs(self.path.as_deref());
    }
}

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChainStepProgress {
    StepStarted {
        step_index: usize,
        total_steps: usize,
        task_dir: PathBuf,
        task_name: Option<String>,
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
    run_task_chain_blocking_with_progress(start_task_dir, options, None, None)
}

pub fn run_task_chain_blocking_with_progress(
    start_task_dir: &Path,
    options: TaskRunOptions,
    progress_tx: Option<mpsc::Sender<RunProgress>>,
    chain_step_tx: Option<mpsc::Sender<ChainStepProgress>>,
) -> Result<ChainRunResult, Error> {
    let chain_steps = resolve_chain_steps(start_task_dir)?;
    let total_steps = chain_steps.len();

    let mut chain_result = ChainRunResult::default();
    let mut pending_handoff: Option<Vec<HandoffArtifact>> = None;

    for (step_idx, chain_step) in chain_steps.into_iter().enumerate() {
        let step_index = step_idx + 1;
        send_chain_step(
            &chain_step_tx,
            ChainStepProgress::StepStarted {
                step_index,
                total_steps,
                task_dir: chain_step.task_dir.clone(),
                task_name: chain_step.loaded.definition.name.clone(),
            },
        );

        let loaded = chain_step.loaded;
        let current_task_dir = chain_step.task_dir;

        let mut overlay_input_dir = None;
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

        let run_result = run_loaded_task_blocking_with_progress(
            &task_to_run,
            options.clone(),
            progress_tx.clone(),
        );
        if let Some(overlay) = overlay_input_dir.as_deref() {
            let _ = fs::remove_dir_all(overlay);
        }
        let run_result = run_result.map_err(|err| Error::Runtime {
            message: format!("Task chain failed at {}: {err}", current_task_dir.display()),
        })?;

        let (handoff_artifacts, next_handoff) = if loaded.definition.next_task_dir.is_some() {
            let artifacts = collect_handoff_artifacts(&loaded)?;
            let names = artifacts
                .iter()
                .map(|a| a.relative_path.to_string_lossy().to_string())
                .collect();
            (names, Some(artifacts))
        } else {
            (Vec::new(), None)
        };
        pending_handoff = next_handoff;

        chain_result.steps.push(ChainStepResult {
            task_dir: current_task_dir.clone(),
            task_name: loaded.definition.name.clone(),
            handoff_artifacts,
            run_result,
        });
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
    let kernel_requirements = resolve_kernel_config_requirements(task)?;
    verify_kernel_config_static(task, &kernel_requirements)?;
    let vm_launch = prepare_vm_launch_options(task, &options)?;
    let _temporary_rootfs_cleanup =
        TemporaryRootfsCleanup::new(vm_launch.temporary_rootfs_path.clone());

    send_progress(&progress_tx, RunProgress::Phase(RunPhase::StartingVm));
    let record = vm_ops
        .run(vm_launch.vm_options)
        .await
        .map_err(|err| Error::VmStart {
            message: err.to_string(),
        })?;

    let vm_id = record.id.clone();
    let mut result = RunResult::new(&vm_id, output_dir, logs_dir);

    let run_error = match prepare_vm_with_ops(vm_ops, &vm_id, input_dir, &progress_tx).await {
        Ok(()) => {
            if let Err(err) =
                verify_kernel_config_runtime_with_ops(vm_ops, &vm_id, &kernel_requirements).await
            {
                Some(err)
            } else {
                send_progress(&progress_tx, RunProgress::Phase(RunPhase::RunningScripts));
                execute_commands_with_ops(vm_ops, commands, &record, &progress_tx, &mut result)
            }
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

    let final_error = match (run_error, cleanup_result) {
        (Some(run_err), Err(cleanup_err)) => Some(combine_errors(run_err, cleanup_err)),
        (Some(err), Ok(_)) | (None, Err(err)) => Some(err),
        (None, Ok(_)) => None,
    };

    if let Some(err) = final_error {
        result.exit_code = 1;
        Err(err)
    } else {
        result.exit_code = 0;
        Ok(result)
    }
}

fn prepare_vm_launch_options(
    task: &LoadedTask,
    options: &TaskRunOptions,
) -> Result<VmLaunchOptions, Error> {
    let effective_disk_size = options
        .disk_size
        .clone()
        .or_else(|| task.definition.disk_size.clone());
    let vm_config = task.definition.vm.clone().unwrap_or_default();
    let resolved_mounts = resolve_vm_mounts(task, &vm_config)?;

    match vm_config.boot {
        TaskVmBoot::Ubuntu => Ok(VmLaunchOptions {
            vm_options: VmOptions {
                disk_size: effective_disk_size,
                kernel: None,
                rootfs: None,
                mounts: resolved_mounts,
                show_progress: options.show_progress,
            },
            temporary_rootfs_path: None,
        }),
        TaskVmBoot::Custom => {
            let kernel_value = vm_config.kernel.as_deref().ok_or_else(|| Error::Runtime {
                message: "vm.kernel is required for vm.boot='custom'".to_string(),
            })?;
            let rootfs_value = vm_config.rootfs.as_deref().ok_or_else(|| Error::Runtime {
                message: "vm.rootfs is required for vm.boot='custom'".to_string(),
            })?;

            let kernel = resolve_vm_config_path(task, "vm.kernel", kernel_value)?;
            let rootfs = resolve_vm_config_path(task, "vm.rootfs", rootfs_value)?;

            if effective_disk_size.is_some() && !vm_config.clone_rootfs {
                return Err(Error::Runtime {
                    message: "custom VM boot requires vm.clone_rootfs=true when disk_size is set"
                        .to_string(),
                });
            }

            let mut runtime_rootfs = rootfs.clone();
            let temporary_rootfs_path = if vm_config.clone_rootfs {
                let cloned = clone_rootfs_for_task(&rootfs, task.definition.name.as_deref())?;
                runtime_rootfs = cloned.clone();
                Some(cloned)
            } else {
                None
            };

            if let Some(size) = effective_disk_size.as_deref()
                && let Err(err) = resize_rootfs_image(&runtime_rootfs, size)
            {
                cleanup_temporary_rootfs(temporary_rootfs_path.as_deref());
                return Err(err);
            }

            Ok(VmLaunchOptions {
                vm_options: VmOptions {
                    disk_size: None,
                    kernel: Some(kernel),
                    rootfs: Some(runtime_rootfs),
                    mounts: resolved_mounts,
                    show_progress: options.show_progress,
                },
                temporary_rootfs_path,
            })
        }
    }
}

fn resolve_vm_mounts(task: &LoadedTask, vm_config: &task::TaskVmConfig) -> Result<Vec<vm::MountSpec>, Error> {
    if vm_config.mounts.is_empty() {
        return Ok(Vec::new());
    }

    let task_dir = task.output_dir.parent().ok_or_else(|| Error::Runtime {
        message: "Failed to resolve task directory for vm.mounts".to_string(),
    })?;

    let mut mounts = Vec::with_capacity(vm_config.mounts.len());
    for (index, mount) in vm_config.mounts.iter().enumerate() {
        let host_path = {
            let configured = Path::new(&mount.host);
            if configured.is_absolute() {
                configured.to_path_buf()
            } else {
                task_dir.join(configured)
            }
        };
        if !host_path.exists() {
            return Err(Error::Runtime {
                message: format!(
                    "vm.mounts[{index}].host path does not exist: {}",
                    host_path.display()
                ),
            });
        }
        if !host_path.is_dir() {
            return Err(Error::Runtime {
                message: format!(
                    "vm.mounts[{index}].host path is not a directory: {}",
                    host_path.display()
                ),
            });
        }

        let guest_path = PathBuf::from(&mount.guest);
        if !guest_path.is_absolute() {
            return Err(Error::Runtime {
                message: format!(
                    "vm.mounts[{index}].guest path is not absolute: {}",
                    mount.guest
                ),
            });
        }

        let mode = match mount.mode {
            TaskVmMountMode::ReadOnly => vm::MountMode::ReadOnly,
            TaskVmMountMode::ReadWrite => vm::MountMode::ReadWrite,
        };
        mounts.push(vm::MountSpec {
            host_path,
            guest_path,
            mode,
        });
    }

    Ok(mounts)
}

fn resolve_vm_config_path(task: &LoadedTask, field: &str, value: &str) -> Result<PathBuf, Error> {
    let configured = Path::new(value);
    let task_dir = task.output_dir.parent().ok_or_else(|| Error::Runtime {
        message: format!("Failed to resolve task directory for {field}"),
    })?;
    let resolved = if configured.is_absolute() {
        configured.to_path_buf()
    } else {
        task_dir.join(configured)
    };

    if !resolved.exists() {
        return Err(Error::Runtime {
            message: format!("{field} path does not exist: {}", resolved.display()),
        });
    }
    if !resolved.is_file() {
        return Err(Error::Runtime {
            message: format!("{field} path is not a file: {}", resolved.display()),
        });
    }

    Ok(resolved)
}

fn resolve_kernel_config_requirements(
    task: &LoadedTask,
) -> Result<Vec<KernelConfigRequirement>, Error> {
    let Some(vm) = task.definition.vm.as_ref() else {
        return Ok(Vec::new());
    };
    let Some(requirements) = vm.required_kernel_config.as_ref() else {
        return Ok(Vec::new());
    };

    let mut parsed = Vec::with_capacity(requirements.len());
    for requirement in requirements {
        parsed.push(parse_kernel_config_requirement(requirement)?);
    }
    Ok(parsed)
}

fn parse_kernel_config_requirement(value: &str) -> Result<KernelConfigRequirement, Error> {
    let value = value.trim();
    let (symbol, expected_raw) = value.split_once('=').ok_or_else(|| Error::Runtime {
        message: format!(
            "Invalid vm.required_kernel_config entry '{value}': expected CONFIG_FOO=y|m|n"
        ),
    })?;

    if !symbol.starts_with("CONFIG_")
        || symbol.len() <= "CONFIG_".len()
        || !symbol
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
    {
        return Err(Error::Runtime {
            message: format!(
                "Invalid vm.required_kernel_config symbol '{symbol}' in '{value}' (expected CONFIG_FOO)"
            ),
        });
    }

    let expected = match expected_raw {
        "y" => KernelConfigValue::Yes,
        "m" => KernelConfigValue::Module,
        "n" => KernelConfigValue::No,
        _ => {
            return Err(Error::Runtime {
                message: format!(
                    "Invalid vm.required_kernel_config value '{expected_raw}' in '{value}' (expected y|m|n)"
                ),
            });
        }
    };

    Ok(KernelConfigRequirement {
        symbol: symbol.to_string(),
        expected,
    })
}

fn verify_kernel_config_static(
    task: &LoadedTask,
    requirements: &[KernelConfigRequirement],
) -> Result<(), Error> {
    if requirements.is_empty() {
        return Ok(());
    }

    let vm = task.definition.vm.as_ref().ok_or_else(|| Error::Runtime {
        message: "vm config is required when vm.required_kernel_config is set".to_string(),
    })?;
    let kernel_config_value = vm.kernel_config.as_deref().ok_or_else(|| Error::Runtime {
        message: "vm.kernel_config is required when vm.required_kernel_config is set".to_string(),
    })?;
    let kernel_config_path = resolve_vm_config_path(task, "vm.kernel_config", kernel_config_value)?;
    let config_text = fs::read_to_string(&kernel_config_path).map_err(|err| Error::Runtime {
        message: format!(
            "Failed to read vm.kernel_config {}: {err}",
            kernel_config_path.display()
        ),
    })?;
    let parsed = parse_kernel_config_text(&config_text);
    let mismatches = collect_kernel_config_mismatches(&parsed, requirements);
    if mismatches.is_empty() {
        return Ok(());
    }

    Err(Error::Runtime {
        message: format!(
            "Kernel config preflight failed for {} using {}: {}",
            task.definition.name.as_deref().unwrap_or("<unnamed-task>"),
            kernel_config_path.display(),
            mismatches.join(", ")
        ),
    })
}

async fn verify_kernel_config_runtime_with_ops<V: VmOps + ?Sized>(
    vm_ops: &V,
    vm_id: &str,
    requirements: &[KernelConfigRequirement],
) -> Result<(), Error> {
    if requirements.is_empty() {
        return Ok(());
    }

    const MISSING_SENTINEL: &str = "__VMIZE_KERNEL_CONFIG_UNAVAILABLE__";
    let command = format!(
        "if [ -r /proc/config.gz ]; then zcat /proc/config.gz; \
         elif [ -r /boot/config-$(uname -r) ]; then cat /boot/config-$(uname -r); \
         else echo {MISSING_SENTINEL}; fi"
    );

    let output = vm_ops
        .ssh(vm_id, &command)
        .await
        .map_err(|err| Error::Runtime {
            message: format!("Failed runtime kernel-config probe in VM {vm_id}: {err}"),
        })?;
    if output.contains(MISSING_SENTINEL) {
        return Err(Error::Runtime {
            message: format!(
                "Runtime kernel config probe failed in VM {vm_id}: /proc/config.gz and /boot/config-$(uname -r) are unavailable (enable CONFIG_IKCONFIG_PROC=y or provide runtime config file)"
            ),
        });
    }

    let parsed = parse_kernel_config_text(&output);
    let mismatches = collect_kernel_config_mismatches(&parsed, requirements);
    if mismatches.is_empty() {
        return Ok(());
    }

    Err(Error::Runtime {
        message: format!(
            "Runtime kernel config check failed in VM {vm_id}: {}",
            mismatches.join(", ")
        ),
    })
}

fn parse_kernel_config_text(contents: &str) -> HashMap<String, KernelConfigValue> {
    let mut values = HashMap::new();

    for raw_line in contents.lines() {
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }

        if let Some(rest) = line.strip_prefix("# CONFIG_")
            && let Some(symbol) = rest.strip_suffix(" is not set")
        {
            values.insert(format!("CONFIG_{symbol}"), KernelConfigValue::No);
            continue;
        }

        if let Some((symbol, value)) = line.split_once('=')
            && symbol.starts_with("CONFIG_")
            && symbol
                .chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
        {
            if value.len() == 1
                && let Some(parsed) = value.chars().next().and_then(KernelConfigValue::from_char)
            {
                values.insert(symbol.to_string(), parsed);
            }
        }
    }

    values
}

fn collect_kernel_config_mismatches(
    actual: &HashMap<String, KernelConfigValue>,
    requirements: &[KernelConfigRequirement],
) -> Vec<String> {
    let mut mismatches = Vec::new();

    for requirement in requirements {
        match actual.get(&requirement.symbol) {
            Some(value) if *value == requirement.expected => {}
            Some(value) => mismatches.push(format!(
                "{} expected={} actual={}",
                requirement.symbol,
                requirement.expected.as_char(),
                value.as_char()
            )),
            None => mismatches.push(format!(
                "{} expected={} actual=<missing>",
                requirement.symbol,
                requirement.expected.as_char()
            )),
        }
    }

    mismatches
}

fn clone_rootfs_for_task(rootfs: &Path, task_name: Option<&str>) -> Result<PathBuf, Error> {
    let clone_path = next_rootfs_clone_path(rootfs, task_name);
    fs::copy(rootfs, &clone_path).map_err(|err| Error::Runtime {
        message: format!(
            "Failed to clone custom rootfs {} to {}: {err}",
            rootfs.display(),
            clone_path.display()
        ),
    })?;
    Ok(clone_path)
}

fn next_rootfs_clone_path(rootfs: &Path, task_name: Option<&str>) -> PathBuf {
    let counter = ROOTFS_CLONE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let label = task_name
        .map(sanitize_for_path)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "task".to_string());
    let ext = rootfs
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("img");

    unique_temp_path(&format!("vmize-rootfs-{label}"), counter, Some(ext))
}

fn sanitize_for_path(value: &str) -> String {
    value
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect()
}

fn resize_rootfs_image(rootfs: &Path, size: &str) -> Result<(), Error> {
    let size = size.trim();
    if size.is_empty() {
        return Ok(());
    }

    let rootfs_str = path_to_str(rootfs)?;
    let output = Command::new("qemu-img")
        .args(["resize", rootfs_str, size])
        .output()
        .map_err(|err| Error::Runtime {
            message: format!(
                "Failed to execute qemu-img resize for {}: {err}",
                rootfs.display()
            ),
        })?;

    if output.status.success() {
        return Ok(());
    }

    Err(Error::Runtime {
        message: format!(
            "qemu-img resize failed for {} to {}: {}",
            rootfs.display(),
            size,
            String::from_utf8_lossy(&output.stderr).trim()
        ),
    })
}

fn cleanup_temporary_rootfs(path: Option<&Path>) {
    if let Some(path) = path {
        let _ = fs::remove_file(path);
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
                let relative_path = match parse_artifact_relative_path(artifact) {
                    Ok(path) => path,
                    Err(err) => return Some(err),
                };
                let remote_path = format!("{}/{}", VM_OUTPUT_DIR, relative_path.to_string_lossy());
                let local_path = task.output_dir.join(&relative_path);
                if let Ok(metadata) = fs::symlink_metadata(&local_path) {
                    let remove_result = if metadata.is_dir() {
                        fs::remove_dir_all(&local_path)
                    } else {
                        fs::remove_file(&local_path)
                    };
                    if let Err(err) = remove_result {
                        return Some(Error::Runtime {
                            message: format!(
                                "Failed to clear existing artifact {}: {err}",
                                local_path.display()
                            ),
                        });
                    }
                }

                if let Err(err) = vm_ops.cp_from(vm_id, &remote_path, output_dir_str, true) {
                    return Some(Error::CopyFromVm {
                        message: err.to_string(),
                    });
                }

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

#[derive(Debug, Clone)]
struct ChainStepPlan {
    task_dir: PathBuf,
    loaded: LoadedTask,
}

fn resolve_chain_steps(start_task_dir: &Path) -> Result<Vec<ChainStepPlan>, Error> {
    let mut visited = HashSet::new();
    let mut current_task_dir = fs::canonicalize(start_task_dir).map_err(|err| Error::Runtime {
        message: format!(
            "Failed to resolve task directory {}: {err}",
            start_task_dir.display()
        ),
    })?;
    let mut steps = Vec::new();

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

        let next_task_dir = loaded.definition.next_task_dir.clone();
        steps.push(ChainStepPlan {
            task_dir: current_task_dir.clone(),
            loaded,
        });

        match next_task_dir {
            Some(next_task_dir) => {
                let next_dir = resolve_next_task_dir(&current_task_dir, &next_task_dir)?;
                current_task_dir = next_dir;
            }
            None => break,
        }
    }

    Ok(steps)
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
    let counter = OVERLAY_DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
    unique_temp_path("vmize-chain-input", counter, None)
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

fn unique_temp_path(prefix: &str, counter: u64, extension: Option<&str>) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let pid = std::process::id();
    let name = match extension {
        Some(ext) => format!("{prefix}-{pid}-{nanos}-{counter}.{ext}"),
        None => format!("{prefix}-{pid}-{nanos}-{counter}"),
    };
    std::env::temp_dir().join(name)
}

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

fn send_optional<T>(tx: &Option<mpsc::Sender<T>>, event: T) {
    if let Some(tx) = tx {
        let _ = tx.send(event);
    }
}

fn send_progress(progress_tx: &Option<mpsc::Sender<RunProgress>>, event: RunProgress) {
    send_optional(progress_tx, event);
}

fn send_chain_step(
    chain_step_tx: &Option<mpsc::Sender<ChainStepProgress>>,
    event: ChainStepProgress,
) {
    send_optional(chain_step_tx, event);
}

// ═══════════════════════════════════════════════════════════════════════════════
// Unit Tests
// ═══════════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vm_ops::MockVmOps;
    use std::fs;
    use std::path::Path;
    use std::path::PathBuf;
    use std::sync::Mutex;
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
                vm: None,
            },
            input_dir,
            output_dir,
            logs_dir,
        };
        (temp, task)
    }

    #[derive(Default)]
    struct CopyingVmOps {
        cp_from_calls: Mutex<Vec<(String, String, bool)>>,
    }

    impl CopyingVmOps {
        fn cp_from_calls(&self) -> Vec<(String, String, bool)> {
            self.cp_from_calls.lock().unwrap().clone()
        }
    }

    impl VmOps for CopyingVmOps {
        async fn run(&self, _options: crate::vm_ops::VmOptions) -> anyhow::Result<vm::VmRecord> {
            unreachable!("run() is not used in collect_output_with_ops tests")
        }

        async fn ssh(&self, _vm_id: &str, _command: &str) -> anyhow::Result<String> {
            unreachable!("ssh() is not used in collect_output_with_ops tests")
        }

        fn cp_to(
            &self,
            _vm_id: &str,
            _local: &str,
            _remote: &str,
            _recursive: bool,
        ) -> anyhow::Result<()> {
            unreachable!("cp_to() is not used in collect_output_with_ops tests")
        }

        fn cp_from(
            &self,
            _vm_id: &str,
            remote: &str,
            local: &str,
            recursive: bool,
        ) -> anyhow::Result<()> {
            self.cp_from_calls.lock().unwrap().push((
                remote.to_string(),
                local.to_string(),
                recursive,
            ));

            if let Some(relative) = remote.strip_prefix(&format!("{}/", VM_OUTPUT_DIR)) {
                let destination = Path::new(local).join(relative);
                if relative.contains('.') {
                    if let Some(parent) = destination.parent() {
                        fs::create_dir_all(parent)?;
                    }
                    fs::write(&destination, format!("copied:{relative}"))?;
                } else {
                    fs::create_dir_all(&destination)?;
                    fs::write(
                        destination.join("copied.marker"),
                        format!("copied:{relative}"),
                    )?;
                }
            }

            Ok(())
        }

        fn rm(&self, _vm_id: &str) -> anyhow::Result<()> {
            unreachable!("rm() is not used in collect_output_with_ops tests")
        }

        fn ssh_stream<F>(
            &self,
            _record: &vm::VmRecord,
            _command: &str,
            _on_line: F,
        ) -> anyhow::Result<()>
        where
            F: FnMut(String) + Send,
        {
            unreachable!("ssh_stream() is not used in collect_output_with_ops tests")
        }
    }

    // ── shell_quote ────────────────────────────────────────────────────────────

    #[test]
    fn parse_kernel_config_text_parses_yes_module_and_not_set_values() {
        let parsed = parse_kernel_config_text(
            r#"
            CONFIG_DM_VERITY=y
            CONFIG_BLK_DEV_LOOP=m
            # CONFIG_USER_NS is not set
            "#,
        );

        assert_eq!(
            parsed.get("CONFIG_DM_VERITY"),
            Some(&KernelConfigValue::Yes)
        );
        assert_eq!(
            parsed.get("CONFIG_BLK_DEV_LOOP"),
            Some(&KernelConfigValue::Module)
        );
        assert_eq!(parsed.get("CONFIG_USER_NS"), Some(&KernelConfigValue::No));
    }

    #[test]
    fn parse_kernel_config_requirement_rejects_config_prefix_only() {
        let err = parse_kernel_config_requirement("CONFIG_=y").unwrap_err();
        match err {
            Error::Runtime { message } => {
                assert!(message.contains("Invalid vm.required_kernel_config symbol"))
            }
            other => panic!("Expected Runtime error, got: {other}"),
        }
    }

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

    #[test]
    fn collect_output_with_ops_replaces_existing_file_and_directory_artifacts() {
        let (_temp, mut task) = create_test_loaded_task(&[]);
        task.definition.artifacts = Some(vec!["artifact.txt".to_string(), "rootfs".to_string()]);

        let stale_file = task.output_dir.join("artifact.txt");
        fs::write(&stale_file, "stale").unwrap();
        let stale_dir = task.output_dir.join("rootfs");
        fs::create_dir_all(&stale_dir).unwrap();
        fs::write(stale_dir.join("old.txt"), "old").unwrap();

        let vm_ops = CopyingVmOps::default();
        let mut result = RunResult::new("vm0", &task.output_dir, &task.logs_dir);
        let run_error = None;

        let rt = tokio::runtime::Runtime::new().unwrap();
        let collect_error = rt.block_on(collect_output_with_ops(
            &vm_ops,
            "vm0",
            &task,
            &mut result,
            &run_error,
        ));

        assert!(collect_error.is_none());
        assert_eq!(
            fs::read_to_string(&stale_file).unwrap(),
            "copied:artifact.txt"
        );
        assert!(!stale_dir.join("old.txt").exists());
        assert!(stale_dir.join("copied.marker").exists());
        assert_eq!(
            result.collected_artifacts,
            vec!["artifact.txt".to_string(), "rootfs".to_string()]
        );

        let calls = vm_ops.cp_from_calls();
        assert!(calls.iter().any(|(remote, _, recursive)| {
            remote == &format!("{}/artifact.txt", VM_OUTPUT_DIR) && *recursive
        }));
        assert!(calls.iter().any(|(remote, _, recursive)| {
            remote == &format!("{}/rootfs", VM_OUTPUT_DIR) && *recursive
        }));
    }

    #[test]
    fn collect_output_with_ops_rejects_invalid_artifact_path_before_copying_artifact() {
        let (_temp, mut task) = create_test_loaded_task(&[]);
        task.definition.artifacts = Some(vec!["../escape.txt".to_string()]);

        let vm_ops = CopyingVmOps::default();
        let mut result = RunResult::new("vm0", &task.output_dir, &task.logs_dir);
        let run_error = None;

        let rt = tokio::runtime::Runtime::new().unwrap();
        let collect_error = rt.block_on(collect_output_with_ops(
            &vm_ops,
            "vm0",
            &task,
            &mut result,
            &run_error,
        ));

        let err = collect_error.expect("expected invalid artifact path error");
        match err {
            Error::Runtime { message } => {
                assert!(message.contains("artifact path"));
                assert!(message.contains(".."));
            }
            other => panic!("Expected Runtime error, got: {other}"),
        }

        let calls = vm_ops.cp_from_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, format!("{}/*", VM_LOGS_DIR));
    }

    #[test]
    fn resolve_chain_steps_expands_linear_chain() {
        let temp = TempDir::new().unwrap();
        let task1 = temp.path().join("task1");
        let task2 = temp.path().join("task2");
        fs::create_dir_all(task1.join("input")).unwrap();
        fs::create_dir_all(task2.join("input")).unwrap();
        fs::write(
            task1.join("task.json"),
            r#"{"name":"one","commands":[],"next_task_dir":"../task2"}"#,
        )
        .unwrap();
        fs::write(task2.join("task.json"), r#"{"name":"two","commands":[]}"#).unwrap();

        let steps = resolve_chain_steps(&task1).unwrap();

        assert_eq!(steps.len(), 2);
        assert_eq!(steps[0].loaded.definition.name.as_deref(), Some("one"));
        assert_eq!(steps[1].loaded.definition.name.as_deref(), Some("two"));
    }

    #[test]
    fn resolve_chain_steps_rejects_cycle() {
        let temp = TempDir::new().unwrap();
        let task1 = temp.path().join("task1");
        let task2 = temp.path().join("task2");
        fs::create_dir_all(task1.join("input")).unwrap();
        fs::create_dir_all(task2.join("input")).unwrap();
        fs::write(
            task1.join("task.json"),
            r#"{"commands":[],"next_task_dir":"../task2"}"#,
        )
        .unwrap();
        fs::write(
            task2.join("task.json"),
            r#"{"commands":[],"next_task_dir":"../task1"}"#,
        )
        .unwrap();

        let err = resolve_chain_steps(&task1).unwrap_err();

        match err {
            Error::Runtime { message } => assert!(message.contains("cycle")),
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

    #[test]
    fn run_loaded_task_with_ops_uses_custom_vm_kernel_and_rootfs() {
        let (temp, mut task) = create_test_loaded_task(&["00_first.sh"]);
        let kernel_path = temp.path().join("kernel").join("bzImage");
        fs::create_dir_all(kernel_path.parent().unwrap()).unwrap();
        fs::write(&kernel_path, "kernel").unwrap();
        let rootfs_path = temp.path().join("rootfs.qcow2");
        fs::write(&rootfs_path, "rootfs").unwrap();
        task.definition.vm = Some(task::TaskVmConfig {
            boot: TaskVmBoot::Custom,
            kernel: Some("kernel/bzImage".to_string()),
            rootfs: Some("rootfs.qcow2".to_string()),
            kernel_config: None,
            required_kernel_config: None,
            mounts: vec![],
            clone_rootfs: false,
        });

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
        assert_eq!(run_calls[0].disk_size, None);
        assert_eq!(run_calls[0].kernel.as_deref(), Some(kernel_path.as_path()));
        assert_eq!(run_calls[0].rootfs.as_deref(), Some(rootfs_path.as_path()));
    }

    #[test]
    fn run_loaded_task_with_ops_passes_vm_mounts_to_run_options() {
        let (temp, mut task) = create_test_loaded_task(&["00_first.sh"]);
        let host_mount_dir = temp.path().join("vigilo");
        fs::create_dir_all(&host_mount_dir).unwrap();
        task.definition.vm = Some(task::TaskVmConfig {
            boot: TaskVmBoot::Ubuntu,
            kernel: None,
            rootfs: None,
            kernel_config: None,
            required_kernel_config: None,
            mounts: vec![task::TaskVmMount {
                host: "vigilo".to_string(),
                guest: "/mnt/vigilo".to_string(),
                mode: task::TaskVmMountMode::ReadOnly,
            }],
            clone_rootfs: true,
        });

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
        assert_eq!(run_calls[0].mounts.len(), 1);
        assert_eq!(run_calls[0].mounts[0].host_path, host_mount_dir);
        assert_eq!(
            run_calls[0].mounts[0].guest_path,
            PathBuf::from("/mnt/vigilo")
        );
    }

    #[test]
    fn run_loaded_task_with_ops_fails_when_vm_mount_host_missing() {
        let (_temp, mut task) = create_test_loaded_task(&["00_first.sh"]);
        task.definition.vm = Some(task::TaskVmConfig {
            boot: TaskVmBoot::Ubuntu,
            kernel: None,
            rootfs: None,
            kernel_config: None,
            required_kernel_config: None,
            mounts: vec![task::TaskVmMount {
                host: "missing-dir".to_string(),
                guest: "/mnt/vigilo".to_string(),
                mode: task::TaskVmMountMode::ReadOnly,
            }],
            clone_rootfs: true,
        });

        let mock = MockVmOps::new().with_run_ok("test-vm");

        let rt = tokio::runtime::Runtime::new().unwrap();
        let err = rt
            .block_on(run_loaded_task_with_ops(
                &mock,
                &task,
                TaskRunOptions::default(),
                None,
            ))
            .unwrap_err();

        match err {
            Error::Runtime { message } => {
                assert!(message.contains("vm.mounts[0].host"));
                assert!(message.contains("does not exist"));
            }
            other => panic!("Expected Runtime error, got: {other}"),
        }
        assert!(mock.run_calls().is_empty());
    }

    #[test]
    fn run_loaded_task_with_ops_clones_custom_rootfs_when_enabled() {
        let (temp, mut task) = create_test_loaded_task(&["00_first.sh"]);
        let kernel_path = temp.path().join("bzImage");
        fs::write(&kernel_path, "kernel").unwrap();
        let rootfs_path = temp.path().join("rootfs.qcow2");
        fs::write(&rootfs_path, "rootfs").unwrap();
        task.definition.vm = Some(task::TaskVmConfig {
            boot: TaskVmBoot::Custom,
            kernel: Some("bzImage".to_string()),
            rootfs: Some("rootfs.qcow2".to_string()),
            kernel_config: None,
            required_kernel_config: None,
            mounts: vec![],
            clone_rootfs: true,
        });

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
        let cloned_rootfs = run_calls[0]
            .rootfs
            .as_ref()
            .expect("rootfs must be set for custom boot")
            .to_path_buf();
        assert_ne!(cloned_rootfs, rootfs_path);
        assert!(!cloned_rootfs.exists());
    }

    #[test]
    fn run_loaded_task_with_ops_cleans_cloned_rootfs_on_vm_start_failure() {
        let (temp, mut task) = create_test_loaded_task(&["00_first.sh"]);
        let kernel_path = temp.path().join("bzImage");
        fs::write(&kernel_path, "kernel").unwrap();
        let rootfs_path = temp.path().join("rootfs.qcow2");
        fs::write(&rootfs_path, "rootfs").unwrap();
        task.definition.vm = Some(task::TaskVmConfig {
            boot: TaskVmBoot::Custom,
            kernel: Some("bzImage".to_string()),
            rootfs: Some("rootfs.qcow2".to_string()),
            kernel_config: None,
            required_kernel_config: None,
            mounts: vec![],
            clone_rootfs: true,
        });

        let mock = MockVmOps::new().with_run_err("failed");

        let rt = tokio::runtime::Runtime::new().unwrap();
        let _ = rt.block_on(run_loaded_task_with_ops(
            &mock,
            &task,
            TaskRunOptions::default(),
            None,
        ));

        let run_calls = mock.run_calls();
        assert_eq!(run_calls.len(), 1);
        let cloned_rootfs = run_calls[0]
            .rootfs
            .as_ref()
            .expect("rootfs must be set for custom boot")
            .to_path_buf();
        assert_ne!(cloned_rootfs, rootfs_path);
        assert!(!cloned_rootfs.exists());
        assert!(mock.rm_calls().is_empty());
    }

    #[test]
    fn run_loaded_task_with_ops_rejects_disk_resize_without_rootfs_clone() {
        let (temp, mut task) = create_test_loaded_task(&["00_first.sh"]);
        let kernel_path = temp.path().join("bzImage");
        fs::write(&kernel_path, "kernel").unwrap();
        let rootfs_path = temp.path().join("rootfs.qcow2");
        fs::write(&rootfs_path, "rootfs").unwrap();
        task.definition.vm = Some(task::TaskVmConfig {
            boot: TaskVmBoot::Custom,
            kernel: Some("bzImage".to_string()),
            rootfs: Some("rootfs.qcow2".to_string()),
            kernel_config: None,
            required_kernel_config: None,
            mounts: vec![],
            clone_rootfs: false,
        });

        let mock = MockVmOps::new().with_run_ok("test-vm");

        let rt = tokio::runtime::Runtime::new().unwrap();
        let err = rt
            .block_on(run_loaded_task_with_ops(
                &mock,
                &task,
                TaskRunOptions {
                    disk_size: Some("12G".to_string()),
                    ..Default::default()
                },
                None,
            ))
            .unwrap_err();

        match err {
            Error::Runtime { message } => assert!(message.contains("vm.clone_rootfs=true")),
            other => panic!("Expected Runtime error, got: {other}"),
        }

        assert!(mock.run_calls().is_empty());
    }

    #[test]
    fn run_loaded_task_with_ops_fails_static_kernel_config_preflight_before_vm_start() {
        let (temp, mut task) = create_test_loaded_task(&["00_first.sh"]);
        let kernel_path = temp.path().join("bzImage");
        let rootfs_path = temp.path().join("rootfs.qcow2");
        let kernel_config_path = temp.path().join("kernel.config");
        fs::write(&kernel_path, "kernel").unwrap();
        fs::write(&rootfs_path, "rootfs").unwrap();
        fs::write(&kernel_config_path, "CONFIG_DM_VERITY=m\n").unwrap();
        task.definition.vm = Some(task::TaskVmConfig {
            boot: TaskVmBoot::Custom,
            kernel: Some("bzImage".to_string()),
            rootfs: Some("rootfs.qcow2".to_string()),
            kernel_config: Some("kernel.config".to_string()),
            required_kernel_config: Some(vec!["CONFIG_DM_VERITY=y".to_string()]),
            mounts: vec![],
            clone_rootfs: false,
        });

        let mock = MockVmOps::new().with_run_ok("test-vm");

        let rt = tokio::runtime::Runtime::new().unwrap();
        let err = rt
            .block_on(run_loaded_task_with_ops(
                &mock,
                &task,
                TaskRunOptions::default(),
                None,
            ))
            .unwrap_err();

        match err {
            Error::Runtime { message } => {
                assert!(message.contains("Kernel config preflight failed"));
                assert!(message.contains("CONFIG_DM_VERITY expected=y actual=m"));
            }
            other => panic!("Expected Runtime error, got: {other}"),
        }

        assert!(mock.run_calls().is_empty());
    }

    #[test]
    fn run_loaded_task_with_ops_fails_runtime_kernel_config_check_before_scripts() {
        let (temp, mut task) = create_test_loaded_task(&["00_first.sh"]);
        let kernel_path = temp.path().join("bzImage");
        let rootfs_path = temp.path().join("rootfs.qcow2");
        let kernel_config_path = temp.path().join("kernel.config");
        fs::write(&kernel_path, "kernel").unwrap();
        fs::write(&rootfs_path, "rootfs").unwrap();
        fs::write(&kernel_config_path, "CONFIG_DM_VERITY=y\n").unwrap();
        task.definition.vm = Some(task::TaskVmConfig {
            boot: TaskVmBoot::Custom,
            kernel: Some("bzImage".to_string()),
            rootfs: Some("rootfs.qcow2".to_string()),
            kernel_config: Some("kernel.config".to_string()),
            required_kernel_config: Some(vec!["CONFIG_DM_VERITY=y".to_string()]),
            mounts: vec![],
            clone_rootfs: false,
        });

        let mock = MockVmOps::new()
            .with_run_ok("test-vm")
            .with_ssh_ok("")
            .with_ssh_ok("CONFIG_DM_VERITY=m\n");

        let rt = tokio::runtime::Runtime::new().unwrap();
        let err = rt
            .block_on(run_loaded_task_with_ops(
                &mock,
                &task,
                TaskRunOptions::default(),
                None,
            ))
            .unwrap_err();

        match err {
            Error::Runtime { message } => {
                assert!(message.contains("Runtime kernel config check failed"));
                assert!(message.contains("CONFIG_DM_VERITY expected=y actual=m"));
            }
            other => panic!("Expected Runtime error, got: {other}"),
        }

        assert_eq!(mock.run_calls().len(), 1);
        assert_eq!(mock.rm_calls(), vec!["test-vm"]);
        assert!(mock.stream_commands.lock().unwrap().is_empty());
    }
}
