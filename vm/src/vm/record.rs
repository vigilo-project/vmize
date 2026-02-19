use crate::process::{is_process_absent_error, is_process_alive, is_qemu_process};
use crate::qemu;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;
use std::time::Instant;
use tracing::{info, warn};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VmStatus {
    Running,
    Stopped,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct VmRecord {
    pub id: String,
    pub hostname: String,
    pub username: String,
    pub ssh_port: u16,
    pub private_key_path: String,
    pub disk_path: String,
    pub seed_iso_path: String,
    pub pid: Option<u32>,
    pub status: VmStatus,
    pub created_at: u64,
    pub host_profile: String,
}

#[derive(Debug, PartialEq)]
pub enum VmRuntimeStatus {
    Running,
    Stopped,
    Stale,
}

impl VmRecord {
    pub fn runtime_status(&self) -> VmRuntimeStatus {
        if self.status == VmStatus::Stopped {
            return VmRuntimeStatus::Stopped;
        }

        match self.pid {
            Some(pid) if is_process_alive(pid) => VmRuntimeStatus::Running,
            Some(_) => VmRuntimeStatus::Stale,
            None => VmRuntimeStatus::Stopped,
        }
    }

    pub fn runtime_status_label(&self) -> &'static str {
        match self.runtime_status() {
            VmRuntimeStatus::Running => "running",
            VmRuntimeStatus::Stopped => "stopped",
            VmRuntimeStatus::Stale => "stale",
        }
    }
}

pub fn is_sequential_vm_id(id: &str) -> bool {
    if id.len() < 2 || !id.starts_with("vm") {
        return false;
    }

    let suffix = &id[2..];
    !suffix.is_empty() && suffix.chars().all(|c| c.is_ascii_digit())
}

pub fn vm_index_from_id(id: &str) -> Option<u32> {
    if !is_sequential_vm_id(id) {
        return None;
    }

    id[2..].parse::<u32>().ok()
}

pub fn next_vm_id(instances_dir: &Path) -> Result<String> {
    let mut max_index = None;
    if instances_dir.exists() {
        for entry in
            std::fs::read_dir(instances_dir).context("Failed to read instance directory")?
        {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }

            let id = entry.file_name().to_string_lossy().into_owned();
            if let Some(index) = vm_index_from_id(&id)
                && max_index.is_none_or(|max| index > max) {
                    max_index = Some(index);
                }
        }
    }

    let next_index = max_index.map_or(0, |max| max + 1);
    Ok(format!("vm{next_index}"))
}

pub fn keep_key_paths(records: &[(String, VmRecord)]) -> HashSet<String> {
    records
        .iter()
        .map(|(_, record)| record.private_key_path.clone())
        .collect()
}

pub fn vm_dir(instances_dir: &Path, id: &str) -> PathBuf {
    instances_dir.join(id)
}

pub fn vm_record_path(instances_dir: &Path, id: &str) -> PathBuf {
    vm_dir(instances_dir, id).join("vm.json")
}

pub fn write_vm_record(instances_dir: &Path, record: &VmRecord) -> Result<()> {
    let vm_dir = vm_dir(instances_dir, &record.id);
    std::fs::create_dir_all(&vm_dir).context("Failed to create VM directory")?;
    let path = vm_record_path(instances_dir, &record.id);
    let data = serde_json::to_string_pretty(record)?;
    std::fs::write(&path, data).context("Failed to write VM record")?;
    Ok(())
}

pub fn read_vm_record(instances_dir: &Path, id: &str) -> Result<VmRecord> {
    let path = vm_record_path(instances_dir, id);
    let data = std::fs::read_to_string(path).context("Failed to read VM record")?;
    let record: VmRecord = serde_json::from_str(&data).context("Invalid VM record data")?;
    Ok(record)
}

pub fn list_vm_records(instances_dir: &Path) -> Result<Vec<(String, VmRecord)>> {
    if !instances_dir.exists() {
        return Ok(Vec::new());
    }

    let mut records = Vec::new();
    for entry in std::fs::read_dir(instances_dir).context("Failed to read instance directory")? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }

        let id = entry.file_name().to_string_lossy().into_owned();
        let path = entry.path().join("vm.json");
        let data = match std::fs::read_to_string(path) {
            Ok(value) => value,
            Err(_) => continue,
        };

        match serde_json::from_str::<VmRecord>(&data) {
            Ok(record) => records.push((id, record)),
            Err(_) => continue,
        }
    }

    records.sort_by_key(|(_, record)| record.created_at);
    Ok(records)
}

pub fn remove_path_if_exists(path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err).context(format!("Failed to remove {}", path.display())),
    }
}

fn read_pid_file(path: &Path) -> Option<u32> {
    let pid_text = std::fs::read_to_string(path).ok()?;
    pid_text.trim().parse::<u32>().ok()
}

fn scan_qemu_processes() -> Vec<(u32, String)> {
    let output = match Command::new("ps").args(["-eo", "pid=,args="]).output() {
        Ok(output) => output,
        Err(_) => return Vec::new(),
    };
    if !output.status.success() {
        return Vec::new();
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut processes = Vec::new();
    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let Some(split_idx) = line.find(' ') else {
            continue;
        };
        let pid_str = &line[..split_idx];
        let args = line[split_idx..].trim_start();
        if args.is_empty() {
            continue;
        }
        let Ok(pid) = pid_str.trim().parse::<u32>() else {
            continue;
        };
        if args.contains("qemu-system-") {
            processes.push((pid, args.to_string()));
        }
    }

    processes
}

fn resolve_qemu_pid(id: &str, record: &VmRecord, instances_dir: &Path) -> Option<u32> {
    if let Some(pid) = record.pid
        && is_qemu_process(pid) {
            return Some(pid);
        }

    let pid_file = vm_dir(instances_dir, id).join("qemu.pid");
    if let Some(pid) = read_pid_file(&pid_file)
        && is_qemu_process(pid) {
            return Some(pid);
        }

    let name_token = format!("-name {id}");
    let mut best_match: Option<(u32, u8)> = None;

    for (pid, args) in scan_qemu_processes() {
        let mut score = 0u8;
        if args.contains(&name_token) {
            score += 5;
        }
        if !record.disk_path.is_empty() && args.contains(&record.disk_path) {
            score += 4;
        }
        if !record.seed_iso_path.is_empty() && args.contains(&record.seed_iso_path) {
            score += 3;
        }

        if score == 0 {
            continue;
        }

        match best_match {
            Some((_, current_score)) if score <= current_score => {}
            _ => best_match = Some((pid, score)),
        }
    }

    best_match.map(|(pid, _)| pid)
}

pub fn remove_vm_instance(
    instances_dir: &Path,
    id: &str,
    record: &VmRecord,
    keep_key_paths: &HashSet<String>,
) -> Result<()> {
    let mut record = record.clone();
    stop_record_if_running(id, &mut record, instances_dir)?;

    let instance_dir = vm_dir(instances_dir, id);
    if instance_dir.exists() {
        std::fs::remove_dir_all(&instance_dir)
            .with_context(|| format!("Failed to remove VM directory {}", instance_dir.display()))?;
    } else {
        info!(
            "VM {} directory {} did not exist",
            id,
            instance_dir.display()
        );
    }

    let private_key_path = PathBuf::from(&record.private_key_path);
    if !keep_key_paths.contains(&record.private_key_path) {
        remove_path_if_exists(&private_key_path)?;
        remove_path_if_exists(&PathBuf::from(format!(
            "{}.pub",
            private_key_path.display()
        )))?;
    } else {
        info!(
            "Keeping key {} because it is used by another VM",
            private_key_path.display()
        );
    }

    info!("Removed VM {}", id);
    Ok(())
}

pub fn stop_qemu_and_wait(pid: u32, timeout: Duration) -> Result<()> {
    qemu::stop_qemu_by_pid(pid)?;

    let deadline = Instant::now() + timeout;
    let force_deadline = Instant::now() + timeout / 2;
    let mut force_kill_sent = false;

    while Instant::now() < deadline {
        if !is_process_alive(pid) {
            return Ok(());
        }

        if !force_kill_sent && Instant::now() >= force_deadline {
            let _ = qemu::force_stop_qemu_by_pid(pid);
            force_kill_sent = true;
        }

        std::thread::sleep(Duration::from_millis(50));
    }

    bail!("Timed out while waiting for qemu process {} to stop", pid);
}

pub fn stop_record_if_running(id: &str, record: &mut VmRecord, instances_dir: &Path) -> Result<()> {
    let resolved_pid = resolve_qemu_pid(id, record, instances_dir);

    // Stop a resolved QEMU process, tolerating "already absent" errors.
    let try_stop = |pid: u32| -> Result<()> {
        match stop_qemu_and_wait(pid, Duration::from_secs(3)) {
            Ok(_) => {
                info!("Stopped VM {} with pid {}", id, pid);
                Ok(())
            }
            Err(err) if is_process_absent_error(&err.to_string()) => {
                info!("VM {} process {} already absent", id, pid);
                Ok(())
            }
            Err(err) => Err(err),
        }
    };

    match (record.pid, resolved_pid) {
        (Some(record_pid), Some(resolved)) => {
            if record_pid != resolved
                && is_process_alive(record_pid)
                && !is_qemu_process(record_pid)
            {
                warn!(
                    "VM {} recorded pid {} is not a QEMU process, resolved running PID {} by metadata",
                    id, record_pid, resolved
                );
            }
            try_stop(resolved)?;
        }
        (Some(record_pid), None) => {
            if is_process_alive(record_pid) {
                warn!(
                    "VM {} recorded pid {} is running but does not look like a QEMU process",
                    id, record_pid
                );
            } else {
                info!("VM {} recorded pid {} already absent", id, record_pid);
            }
        }
        (None, Some(resolved)) => {
            info!(
                "VM {} has no recorded pid; resolved running pid {}",
                id, resolved
            );
            try_stop(resolved)?;
        }
        (None, None) => {
            info!("VM {} has no active process to stop", id);
        }
    }

    record.pid = None;
    record.status = VmStatus::Stopped;
    write_vm_record(instances_dir, record)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn temp_instances_dir() -> TempDir {
        TempDir::new().expect("failed to create temp instances dir")
    }

    fn sample_record(id: &str, key_path: &str) -> VmRecord {
        VmRecord {
            id: id.to_string(),
            hostname: "vm".to_string(),
            username: "ubuntu".to_string(),
            ssh_port: 2222,
            private_key_path: key_path.to_string(),
            disk_path: "/tmp/disk.qcow2".to_string(),
            seed_iso_path: "/tmp/seed.iso".to_string(),
            pid: None,
            status: VmStatus::Running,
            created_at: 0,
            host_profile: "linux/amd64".to_string(),
        }
    }

    #[test]
    fn test_next_vm_id_uses_next_available_number() {
        let instances_dir = temp_instances_dir();
        let base = instances_dir.path();

        std::fs::create_dir_all(base.join("vm0")).unwrap();
        std::fs::create_dir_all(base.join("vm2")).unwrap();
        std::fs::create_dir_all(base.join("vm3")).unwrap();
        std::fs::create_dir_all(base.join("550e8400-e29b-41d4-a716-446655440000")).unwrap();

        assert_eq!(next_vm_id(base).unwrap(), "vm4");
    }

    #[test]
    fn test_remove_vm_instance_deletes_directory_and_key_when_unused() {
        let instances_dir = temp_instances_dir();
        let base = instances_dir.path();
        let vm_id = "vm1";
        let vm_dir = base.join(vm_id);
        let keys_dir = base.join("keys");
        let key_path = keys_dir.join("vm-key");

        std::fs::create_dir_all(&keys_dir).unwrap();
        std::fs::create_dir_all(&vm_dir).unwrap();
        std::fs::write(&key_path, b"test-key").unwrap();
        std::fs::write(format!("{}.pub", key_path.display()), b"test-key-pub").unwrap();

        let record = sample_record(vm_id, key_path.to_string_lossy().as_ref());
        let keep_keys = keep_key_paths(&[]);
        remove_vm_instance(base, vm_id, &record, &keep_keys).unwrap();

        assert!(!vm_dir.exists());
        assert!(!key_path.exists());
        assert!(!base.join(format!("{}.pub", key_path.display())).exists());
    }

    #[test]
    fn test_remove_vm_instance_keeps_shared_key() {
        let instances_dir = temp_instances_dir();
        let base = instances_dir.path();
        let vm_dir = base.join("vm1");
        let keys_dir = base.join("keys");
        let key_path = keys_dir.join("shared-key");

        std::fs::create_dir_all(&keys_dir).unwrap();
        std::fs::create_dir_all(&vm_dir).unwrap();
        std::fs::write(&key_path, b"test-key").unwrap();
        std::fs::write(format!("{}.pub", key_path.display()), b"test-key-pub").unwrap();

        let record = sample_record("vm1", key_path.to_string_lossy().as_ref());
        let mut keep_keys = keep_key_paths(&[]);
        keep_keys.insert(key_path.to_string_lossy().to_string());
        remove_vm_instance(base, "vm1", &record, &keep_keys).unwrap();

        assert!(!vm_dir.exists());
        assert!(key_path.exists());
        assert!(base.join(format!("{}.pub", key_path.display())).exists());
    }
}
