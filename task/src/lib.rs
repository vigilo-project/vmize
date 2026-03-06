use std::path::{Component, Path, PathBuf};

use serde::Deserialize;

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq, Default)]
pub enum TaskVmBoot {
    #[default]
    #[serde(rename = "ubuntu", alias = "cloud")]
    Ubuntu,
    #[serde(rename = "custom")]
    Custom,
}

fn default_clone_rootfs() -> bool {
    true
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq, Default)]
pub enum TaskVmMountMode {
    #[default]
    #[serde(rename = "ro")]
    ReadOnly,
    #[serde(rename = "rw")]
    ReadWrite,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct TaskVmMount {
    pub host: String,
    pub guest: String,
    #[serde(default)]
    pub mode: TaskVmMountMode,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct TaskVmConfig {
    #[serde(default)]
    pub boot: TaskVmBoot,
    #[serde(default)]
    pub memory: Option<String>,
    #[serde(default)]
    pub cpus: Option<u32>,
    #[serde(default)]
    pub kernel: Option<String>,
    #[serde(default)]
    pub rootfs: Option<String>,
    #[serde(default)]
    pub kernel_config: Option<String>,
    #[serde(default)]
    pub required_kernel_config: Option<Vec<String>>,
    #[serde(default)]
    pub mounts: Vec<TaskVmMount>,
    #[serde(default = "default_clone_rootfs")]
    pub clone_rootfs: bool,
}

impl Default for TaskVmConfig {
    fn default() -> Self {
        Self {
            boot: TaskVmBoot::Ubuntu,
            memory: None,
            cpus: None,
            kernel: None,
            rootfs: None,
            kernel_config: None,
            required_kernel_config: None,
            mounts: Vec::new(),
            clone_rootfs: true,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct TaskDefinition {
    pub name: Option<String>,
    pub description: Option<String>,
    #[serde(default)]
    pub disk_size: Option<String>,
    #[serde(default)]
    pub commands: Vec<String>,
    #[serde(default)]
    pub artifacts: Option<Vec<String>>,
    #[serde(default)]
    pub next_task_dir: Option<String>,
    #[serde(default)]
    pub vm: Option<TaskVmConfig>,
}

#[derive(Debug, Clone)]
pub struct LoadedTask {
    pub definition: TaskDefinition,
    pub input_dir: PathBuf,  // task_dir/input/
    pub output_dir: PathBuf, // task_dir/output/
    pub logs_dir: PathBuf,   // task_dir/output/logs/
}

pub fn load_task(task_dir: &Path) -> Result<LoadedTask, String> {
    let json_path = task_dir.join("task.json");
    let contents = std::fs::read_to_string(&json_path)
        .map_err(|err| format!("Cannot read {}: {err}", json_path.display()))?;
    let definition: TaskDefinition = serde_json::from_str(&contents)
        .map_err(|err| format!("Invalid JSON in {}: {err}", json_path.display()))?;

    let input_dir = task_dir.join("input");
    if !input_dir.exists() {
        return Err(format!(
            "Input directory not found: {}",
            input_dir.display()
        ));
    }
    if !input_dir.is_dir() {
        return Err(format!(
            "Input path is not a directory: {}",
            input_dir.display()
        ));
    }

    for cmd in &definition.commands {
        let cmd_path = input_dir.join(cmd);
        if !cmd_path.exists() {
            return Err(format!(
                "Command '{cmd}' not found in input: {}",
                cmd_path.display()
            ));
        }
        if !cmd_path.is_file() {
            return Err(format!(
                "Command '{cmd}' is not a file in input: {}",
                cmd_path.display()
            ));
        }
    }

    if let Some(next_task_dir) = definition.next_task_dir.as_deref() {
        validate_relative_task_path(next_task_dir).map_err(|message| {
            format!(
                "Invalid next_task_dir '{next_task_dir}' in {}: {message}",
                json_path.display()
            )
        })?;
    }

    if let Some(artifacts) = definition.artifacts.as_ref() {
        for artifact in artifacts {
            validate_artifact_path(artifact).map_err(|message| {
                format!(
                    "Invalid artifact '{artifact}' in {}: {message}",
                    json_path.display()
                )
            })?;
        }
    }

    if let Some(vm) = definition.vm.as_ref() {
        validate_vm_config(vm).map_err(|message| {
            format!("Invalid vm config in {}: {message}", json_path.display())
        })?;
    }

    let output_dir = task_dir.join("output");
    let logs_dir = output_dir.join("logs");
    std::fs::create_dir_all(&logs_dir)
        .map_err(|err| format!("Cannot create logs dir {}: {err}", logs_dir.display()))?;

    Ok(LoadedTask {
        definition,
        input_dir,
        output_dir,
        logs_dir,
    })
}

// ═══════════════════════════════════════════════════════════════════════════════
// Unit Tests
// ═══════════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn create_task_dir(json_content: &str) -> TempDir {
        let temp = TempDir::new().expect("failed to create temp dir");
        let json_path = temp.path().join("task.json");
        fs::write(&json_path, json_content).expect("failed to write task.json");
        temp
    }

    fn create_task_dir_with_input(json_content: &str, commands: &[&str]) -> TempDir {
        let temp = create_task_dir(json_content);
        let input_dir = temp.path().join("input");
        fs::create_dir(&input_dir).unwrap();
        for cmd in commands {
            fs::write(input_dir.join(cmd), "#!/bin/bash").unwrap();
        }
        temp
    }

    #[test]
    fn load_task_parses_valid_json_with_all_fields() {
        let temp = create_task_dir_with_input(
            r#"{"name": "test-task", "description": "A test", "disk_size": "10G", "commands": ["00_first.sh", "10_second.sh"]}"#,
            &["00_first.sh", "10_second.sh"],
        );

        let result = load_task(temp.path()).unwrap();

        assert_eq!(result.definition.name, Some("test-task".to_string()));
        assert_eq!(result.definition.description, Some("A test".to_string()));
        assert_eq!(result.definition.disk_size, Some("10G".to_string()));
        assert_eq!(
            result.definition.commands,
            vec!["00_first.sh", "10_second.sh"]
        );
        assert_eq!(result.input_dir, temp.path().join("input"));
        assert_eq!(result.output_dir, temp.path().join("output"));
        assert_eq!(result.logs_dir, temp.path().join("output").join("logs"));
    }

    #[test]
    fn load_task_parses_json_with_no_commands() {
        let temp = create_task_dir_with_input(r#"{"name": "minimal"}"#, &[]);

        let result = load_task(temp.path()).unwrap();

        assert_eq!(result.definition.name, Some("minimal".to_string()));
        assert!(result.definition.commands.is_empty());
        assert!(result.definition.artifacts.is_none());
    }

    #[test]
    fn load_task_parses_artifacts_field() {
        let temp = create_task_dir_with_input(
            r#"{"commands": [], "artifacts": ["out.txt", "result.tar"]}"#,
            &[],
        );

        let result = load_task(temp.path()).unwrap();

        assert_eq!(
            result.definition.artifacts,
            Some(vec!["out.txt".to_string(), "result.tar".to_string()])
        );
    }

    #[test]
    fn load_task_parses_next_task_dir_field() {
        let temp = create_task_dir_with_input(
            r#"{"commands": [], "artifacts": ["handoff.txt"], "next_task_dir": "task2"}"#,
            &[],
        );

        let result = load_task(temp.path()).unwrap();

        assert_eq!(result.definition.next_task_dir, Some("task2".to_string()));
    }

    #[test]
    fn load_task_parses_vm_custom_config() {
        let temp = create_task_dir_with_input(
            r#"{
                "commands": [],
                "vm": {
                    "boot": "custom",
                    "memory": "16G",
                    "cpus": 8,
                    "kernel": "../image/bzImage",
                    "rootfs": "../image/rootfs.qcow2",
                    "kernel_config": "../image/kernel.config",
                    "required_kernel_config": [
                        "CONFIG_DM_VERITY=y",
                        "CONFIG_CRYPTO_SHA256=y"
                    ],
                    "mounts": [
                        {"host": "../vigilo", "guest": "/mnt/vigilo"},
                        {"host": "/tmp", "guest": "/mnt/tmp", "mode": "rw"}
                    ],
                    "clone_rootfs": false
                }
            }"#,
            &[],
        );

        let result = load_task(temp.path()).unwrap();
        let vm = result.definition.vm.expect("vm config should be present");
        assert_eq!(vm.boot, TaskVmBoot::Custom);
        assert_eq!(vm.memory.as_deref(), Some("16G"));
        assert_eq!(vm.cpus, Some(8));
        assert_eq!(vm.kernel.as_deref(), Some("../image/bzImage"));
        assert_eq!(vm.rootfs.as_deref(), Some("../image/rootfs.qcow2"));
        assert_eq!(vm.kernel_config.as_deref(), Some("../image/kernel.config"));
        assert_eq!(
            vm.required_kernel_config,
            Some(vec![
                "CONFIG_DM_VERITY=y".to_string(),
                "CONFIG_CRYPTO_SHA256=y".to_string()
            ])
        );
        assert_eq!(vm.mounts.len(), 2);
        assert_eq!(vm.mounts[0].host, "../vigilo");
        assert_eq!(vm.mounts[0].guest, "/mnt/vigilo");
        assert_eq!(vm.mounts[0].mode, TaskVmMountMode::ReadOnly);
        assert_eq!(vm.mounts[1].mode, TaskVmMountMode::ReadWrite);
        assert!(!vm.clone_rootfs);
    }

    #[test]
    fn load_task_parses_vm_cloud_alias_as_ubuntu() {
        let temp = create_task_dir_with_input(
            r#"{
                "commands": [],
                "vm": {
                    "boot": "cloud"
                }
            }"#,
            &[],
        );

        let result = load_task(temp.path()).unwrap();
        let vm = result.definition.vm.expect("vm config should be present");
        assert_eq!(vm.boot, TaskVmBoot::Ubuntu);
        assert!(vm.clone_rootfs);
    }

    #[test]
    fn load_task_fails_when_vm_custom_kernel_missing() {
        let temp = create_task_dir_with_input(
            r#"{
                "commands": [],
                "vm": {
                    "boot": "custom",
                    "rootfs": "../image/rootfs.qcow2"
                }
            }"#,
            &[],
        );

        let result = load_task(temp.path());
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("vm.kernel is required"));
    }

    #[test]
    fn load_task_fails_when_vm_ubuntu_has_custom_paths() {
        let temp = create_task_dir_with_input(
            r#"{
                "commands": [],
                "vm": {
                    "boot": "ubuntu",
                    "kernel": "../image/bzImage"
                }
            }"#,
            &[],
        );

        let result = load_task(temp.path());
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("vm.boot='ubuntu'"));
    }

    #[test]
    fn load_task_fails_when_vm_ubuntu_declares_required_kernel_config() {
        let temp = create_task_dir_with_input(
            r#"{
                "commands": [],
                "vm": {
                    "boot": "ubuntu",
                    "required_kernel_config": ["CONFIG_DM_VERITY=y"]
                }
            }"#,
            &[],
        );

        let result = load_task(temp.path());
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("vm.boot='ubuntu'"));
        assert!(err.contains("required_kernel_config"));
    }

    #[test]
    fn load_task_fails_when_required_kernel_config_missing_kernel_config_path() {
        let temp = create_task_dir_with_input(
            r#"{
                "commands": [],
                "vm": {
                    "boot": "custom",
                    "kernel": "../image/bzImage",
                    "rootfs": "../image/rootfs.qcow2",
                    "required_kernel_config": ["CONFIG_DM_VERITY=y"]
                }
            }"#,
            &[],
        );

        let result = load_task(temp.path());
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("vm.kernel_config"));
    }

    #[test]
    fn load_task_fails_when_required_kernel_config_entry_is_invalid() {
        let temp = create_task_dir_with_input(
            r#"{
                "commands": [],
                "vm": {
                    "boot": "custom",
                    "kernel": "../image/bzImage",
                    "rootfs": "../image/rootfs.qcow2",
                    "kernel_config": "../image/kernel.config",
                    "required_kernel_config": ["CONFIG_DM_VERITY=maybe"]
                }
            }"#,
            &[],
        );

        let result = load_task(temp.path());
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("required_kernel_config"));
        assert!(err.contains("must be in form"));
    }

    #[test]
    fn load_task_fails_when_required_kernel_config_is_empty() {
        let temp = create_task_dir_with_input(
            r#"{
                "commands": [],
                "vm": {
                    "boot": "custom",
                    "kernel": "../image/bzImage",
                    "rootfs": "../image/rootfs.qcow2",
                    "kernel_config": "../image/kernel.config",
                    "required_kernel_config": []
                }
            }"#,
            &[],
        );

        let result = load_task(temp.path());
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("required_kernel_config"));
        assert!(err.contains("must not be empty"));
    }

    #[test]
    fn load_task_fails_when_vm_custom_path_has_curdir_component() {
        let temp = create_task_dir_with_input(
            r#"{
                "commands": [],
                "vm": {
                    "boot": "custom",
                    "kernel": "./image/bzImage",
                    "rootfs": "../image/rootfs.qcow2"
                }
            }"#,
            &[],
        );

        let result = load_task(temp.path());
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("vm.kernel"));
        assert!(err.contains("must not include '.'"));
    }

    #[test]
    fn load_task_fails_when_vm_mount_guest_is_not_absolute() {
        let temp = create_task_dir_with_input(
            r#"{
                "commands": [],
                "vm": {
                    "mounts": [{"host": "../vigilo", "guest": "mnt/vigilo"}]
                }
            }"#,
            &[],
        );

        let result = load_task(temp.path());
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("vm.mounts[0].guest"));
        assert!(err.contains("absolute path"));
    }

    #[test]
    fn load_task_fails_when_vm_memory_is_empty() {
        let temp = create_task_dir_with_input(
            r#"{
                "commands": [],
                "vm": {
                    "memory": "   "
                }
            }"#,
            &[],
        );

        let result = load_task(temp.path());
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("vm.memory"));
        assert!(err.contains("must not be empty"));
    }

    #[test]
    fn load_task_fails_when_vm_cpus_is_zero() {
        let temp = create_task_dir_with_input(
            r#"{
                "commands": [],
                "vm": {
                    "cpus": 0
                }
            }"#,
            &[],
        );

        let result = load_task(temp.path());
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("vm.cpus"));
        assert!(err.contains("must be greater than 0"));
    }

    #[test]
    fn load_task_fails_for_missing_file() {
        let temp = TempDir::new().expect("failed to create temp dir");

        let result = load_task(temp.path());

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("Cannot read"));
        assert!(err.contains("task.json"));
    }

    #[test]
    fn load_task_fails_for_invalid_json() {
        let temp = create_task_dir(r#"not valid json"#);

        let result = load_task(temp.path());

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("Invalid JSON"));
    }

    #[test]
    fn load_task_fails_if_input_dir_missing() {
        let temp = create_task_dir(r#"{"name": "test", "commands": ["00_run.sh"]}"#);

        let result = load_task(temp.path());

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("Input directory not found"));
    }

    #[test]
    fn load_task_fails_if_command_file_missing() {
        let temp = create_task_dir_with_input(
            r#"{"name": "test", "commands": ["00_run.sh", "10_missing.sh"]}"#,
            &["00_run.sh"],
        );

        let result = load_task(temp.path());

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("10_missing.sh"));
    }

    #[test]
    fn load_task_fails_if_command_path_is_directory() {
        let temp = create_task_dir_with_input(
            r#"{"name": "test", "commands": ["00_run.sh", "10_dir"]}"#,
            &["00_run.sh"],
        );
        fs::create_dir_all(temp.path().join("input").join("10_dir")).unwrap();

        let result = load_task(temp.path());

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("is not a file"));
        assert!(err.contains("10_dir"));
    }

    #[test]
    fn load_task_creates_output_and_logs_directories() {
        let temp = create_task_dir_with_input(r#"{"name": "test"}"#, &[]);
        let output_dir = temp.path().join("output");
        let logs_dir = output_dir.join("logs");

        assert!(!output_dir.exists());

        let result = load_task(temp.path());

        assert!(result.is_ok());
        assert!(output_dir.is_dir());
        assert!(logs_dir.is_dir());
    }

    #[test]
    fn load_task_output_directory_already_exists() {
        let temp = create_task_dir_with_input(r#"{"name": "test"}"#, &[]);
        let output_dir = temp.path().join("output");
        fs::create_dir_all(output_dir.join("logs")).unwrap();
        fs::write(output_dir.join("existing.txt"), "data").unwrap();

        let result = load_task(temp.path());

        assert!(result.is_ok());
        assert!(output_dir.join("existing.txt").exists());
    }

    #[test]
    fn load_task_handles_unicode_in_name() {
        let temp = create_task_dir_with_input(
            r#"{"name": "테스트-タスク-🔥", "description": "한글 설명"}"#,
            &[],
        );

        let result = load_task(temp.path()).unwrap();

        assert_eq!(result.definition.name, Some("테스트-タスク-🔥".to_string()));
        assert_eq!(result.definition.description, Some("한글 설명".to_string()));
    }

    #[test]
    fn load_task_fails_if_next_task_dir_is_absolute() {
        let temp = create_task_dir_with_input(
            r#"{"commands": [], "artifacts": ["handoff.txt"], "next_task_dir": "/abs/path"}"#,
            &[],
        );

        let result = load_task(temp.path());

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("Invalid next_task_dir"));
        assert!(err.contains("absolute paths are not allowed"));
    }

    #[test]
    fn load_task_allows_next_task_dir_with_parent_component() {
        let temp = create_task_dir_with_input(
            r#"{"commands": [], "artifacts": ["handoff.txt"], "next_task_dir": "../task2"}"#,
            &[],
        );

        let result = load_task(temp.path()).unwrap();

        assert_eq!(
            result.definition.next_task_dir,
            Some("../task2".to_string())
        );
    }

    #[test]
    fn load_task_fails_if_artifact_has_parent_component() {
        let temp = create_task_dir_with_input(r#"{"commands": [], "artifacts": ["../x"]}"#, &[]);

        let result = load_task(temp.path());

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("Invalid artifact"));
        assert!(err.contains("must not include '..'"));
    }

    #[test]
    fn load_task_fails_if_artifact_is_absolute() {
        let temp = create_task_dir_with_input(r#"{"commands": [], "artifacts": ["/abs"]}"#, &[]);

        let result = load_task(temp.path());

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("Invalid artifact"));
        assert!(err.contains("absolute paths are not allowed"));
    }

    #[test]
    fn load_task_fails_if_artifact_has_curdir_component() {
        let temp = create_task_dir_with_input(r#"{"commands": [], "artifacts": ["./x"]}"#, &[]);

        let result = load_task(temp.path());

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("Invalid artifact"));
        assert!(err.contains("must not include '.'"));
    }

    #[test]
    fn load_task_allows_nested_relative_artifact_path() {
        let temp = create_task_dir_with_input(
            r#"{"commands": [], "artifacts": ["rootfs/config.json"]}"#,
            &[],
        );

        let result = load_task(temp.path()).unwrap();

        assert_eq!(
            result.definition.artifacts,
            Some(vec!["rootfs/config.json".to_string()])
        );
    }
}

fn validate_relative_task_path(value: &str) -> Result<(), String> {
    if value.trim().is_empty() {
        return Err("must not be empty".to_string());
    }

    let path = Path::new(value);
    if path.is_absolute() {
        return Err("absolute paths are not allowed".to_string());
    }

    if path
        .components()
        .any(|component| matches!(component, Component::RootDir | Component::Prefix(_)))
    {
        return Err("must be a relative path (root/drive-prefix is not allowed)".to_string());
    }

    if path
        .components()
        .any(|component| matches!(component, Component::CurDir))
    {
        return Err("must not include '.' path components".to_string());
    }

    Ok(())
}

fn validate_artifact_path(value: &str) -> Result<(), String> {
    if value.trim().is_empty() {
        return Err("must not be empty".to_string());
    }

    let path = Path::new(value);
    if path.is_absolute() {
        return Err("absolute paths are not allowed".to_string());
    }

    if path
        .components()
        .any(|component| matches!(component, Component::RootDir | Component::Prefix(_)))
    {
        return Err("must be a relative path (root/drive-prefix is not allowed)".to_string());
    }

    if path
        .components()
        .any(|component| matches!(component, Component::CurDir))
    {
        return Err("must not include '.' path components".to_string());
    }

    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err("must not include '..' path components".to_string());
    }

    Ok(())
}

fn validate_vm_config(vm: &TaskVmConfig) -> Result<(), String> {
    if let Some(memory) = vm.memory.as_deref()
        && memory.trim().is_empty()
    {
        return Err("vm.memory must not be empty when provided".to_string());
    }

    if let Some(cpus) = vm.cpus
        && cpus == 0
    {
        return Err("vm.cpus must be greater than 0 when provided".to_string());
    }

    for (index, mount) in vm.mounts.iter().enumerate() {
        validate_vm_mount(mount, index)?;
    }

    match vm.boot {
        TaskVmBoot::Ubuntu => {
            if vm.kernel.is_some()
                || vm.rootfs.is_some()
                || vm.kernel_config.is_some()
                || vm.required_kernel_config.is_some()
            {
                return Err(
                    "vm.boot='ubuntu' does not allow vm.kernel/vm.rootfs/vm.kernel_config/vm.required_kernel_config (use vm.boot='custom')"
                        .to_string(),
                );
            }
        }
        TaskVmBoot::Custom => {
            let kernel = vm
                .kernel
                .as_deref()
                .ok_or_else(|| "vm.kernel is required when vm.boot='custom'".to_string())?;
            let rootfs = vm
                .rootfs
                .as_deref()
                .ok_or_else(|| "vm.rootfs is required when vm.boot='custom'".to_string())?;
            validate_vm_path(kernel, "vm.kernel")?;
            validate_vm_path(rootfs, "vm.rootfs")?;

            if let Some(kernel_config) = vm.kernel_config.as_deref() {
                validate_vm_path(kernel_config, "vm.kernel_config")?;
            }

            if let Some(required) = vm.required_kernel_config.as_ref() {
                if required.is_empty() {
                    return Err(
                        "vm.required_kernel_config must not be empty when provided".to_string()
                    );
                }
                if vm.kernel_config.is_none() {
                    return Err(
                        "vm.kernel_config is required when vm.required_kernel_config is set"
                            .to_string(),
                    );
                }
                for entry in required {
                    validate_required_kernel_config_entry(entry).map_err(|message| {
                        format!("vm.required_kernel_config entry '{entry}' invalid: {message}")
                    })?;
                }
            }
        }
    }

    Ok(())
}

fn validate_vm_mount(mount: &TaskVmMount, index: usize) -> Result<(), String> {
    if mount.host.trim().is_empty() {
        return Err(format!("vm.mounts[{index}].host must not be empty"));
    }
    if mount.guest.trim().is_empty() {
        return Err(format!("vm.mounts[{index}].guest must not be empty"));
    }

    let host = Path::new(&mount.host);
    if host
        .components()
        .any(|component| matches!(component, Component::CurDir))
    {
        return Err(format!(
            "vm.mounts[{index}].host must not include '.' path components"
        ));
    }

    let guest = Path::new(&mount.guest);
    if !guest.is_absolute() {
        return Err(format!("vm.mounts[{index}].guest must be an absolute path"));
    }
    if guest
        .components()
        .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
    {
        return Err(format!(
            "vm.mounts[{index}].guest must not include '.' or '..' path components"
        ));
    }

    Ok(())
}

fn validate_required_kernel_config_entry(value: &str) -> Result<(), String> {
    let value = value.trim();
    if value.is_empty() {
        return Err("vm.required_kernel_config entries must not be empty".to_string());
    }

    let (symbol, expected) = value
        .split_once('=')
        .ok_or_else(|| "must be in form CONFIG_FOO=y|m|n".to_string())?;

    if !symbol.starts_with("CONFIG_") || symbol.len() <= "CONFIG_".len() {
        return Err("must start with CONFIG_ and include a symbol name".to_string());
    }
    if !symbol
        .chars()
        .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
    {
        return Err("symbol must contain only A-Z, 0-9, and _".to_string());
    }

    match expected {
        "y" | "m" | "n" => Ok(()),
        _ => Err("must be in form CONFIG_FOO=y|m|n".to_string()),
    }
}

fn validate_vm_path(value: &str, field: &str) -> Result<(), String> {
    if value.trim().is_empty() {
        return Err(format!("{field} must not be empty"));
    }

    let path = Path::new(value);
    if path
        .components()
        .any(|component| matches!(component, Component::CurDir))
    {
        return Err(format!("{field} must not include '.' path components"));
    }

    Ok(())
}
