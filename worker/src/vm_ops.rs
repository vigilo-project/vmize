//! VM operations trait for dependency injection.
//!
//! This module provides an abstraction layer over the `vm` crate,
//! enabling unit testing without requiring QEMU/VM infrastructure.

use anyhow::Result;
use std::path::PathBuf;

#[cfg(test)]
use std::sync::Mutex;

// Re-export VmRecord for convenience
pub use vm::VmRecord;

/// Options for VM creation (mirrors vm::RunOptions).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VmOptions {
    pub disk_size: Option<String>,
    pub kernel: Option<PathBuf>,
    pub rootfs: Option<PathBuf>,
    pub mounts: Vec<vm::MountSpec>,
    pub show_progress: bool,
}

/// Trait abstracting VM operations.
///
/// This trait enables dependency injection for testing worker logic
/// without requiring actual VM infrastructure.
pub trait VmOps: Send + Sync {
    /// Start a new VM and return its record.
    fn run(&self, options: VmOptions) -> impl std::future::Future<Output = Result<VmRecord>>;

    /// Execute a command in the VM via SSH.
    fn ssh(&self, vm_id: &str, command: &str) -> impl std::future::Future<Output = Result<String>>;

    /// Copy files from host to VM.
    fn cp_to(&self, vm_id: &str, local: &str, remote: &str, recursive: bool) -> Result<()>;

    /// Copy files from VM to host.
    fn cp_from(&self, vm_id: &str, remote: &str, local: &str, recursive: bool) -> Result<()>;

    /// Remove/delete a VM.
    fn rm(&self, vm_id: &str) -> Result<()>;

    /// Stream SSH command output line by line.
    fn ssh_stream<F>(&self, record: &VmRecord, command: &str, on_line: F) -> Result<()>
    where
        F: FnMut(String) + Send;
}

// ═══════════════════════════════════════════════════════════════════════════════
// Production Implementation
// ═══════════════════════════════════════════════════════════════════════════════

/// Production implementation that delegates to the vm crate.
pub struct RealVmOps;

impl VmOps for RealVmOps {
    async fn run(&self, options: VmOptions) -> Result<VmRecord> {
        let vm_options = vm::RunOptions {
            disk_size: options.disk_size,
            kernel: options.kernel,
            rootfs: options.rootfs,
            mounts: options.mounts,
            show_progress: options.show_progress,
            on_progress: None,
            ..Default::default()
        };
        vm::run(vm_options).await
    }

    async fn ssh(&self, vm_id: &str, command: &str) -> Result<String> {
        vm::ssh(vm_id, Some(command)).await
    }

    fn cp_to(&self, vm_id: &str, local: &str, remote: &str, recursive: bool) -> Result<()> {
        vm::cp_to(vm_id, local, remote, recursive)
    }

    fn cp_from(&self, vm_id: &str, remote: &str, local: &str, recursive: bool) -> Result<()> {
        vm::cp_from(vm_id, remote, local, recursive)
    }

    fn rm(&self, vm_id: &str) -> Result<()> {
        vm::rm(Some(vm_id))
    }

    fn ssh_stream<F>(&self, record: &VmRecord, command: &str, mut on_line: F) -> Result<()>
    where
        F: FnMut(String) + Send,
    {
        use std::io::{BufRead, BufReader};
        use std::process::{Command, Stdio};
        use std::thread;

        let mut child = Command::new("ssh")
            .args([
                "-i",
                &record.private_key_path,
                "-p",
                &record.ssh_port.to_string(),
                "-o",
                "BatchMode=yes",
                "-o",
                "StrictHostKeyChecking=no",
                "-o",
                "UserKnownHostsFile=/dev/null",
                "-o",
                "ConnectTimeout=10",
                &format!("{}@127.0.0.1", record.username.as_str()),
                "--",
                command,
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;

        let stdout = child.stdout.take().expect("failed to capture stdout");
        let stderr = child.stderr.take().expect("failed to capture stderr");

        let (line_tx, line_rx) = std::sync::mpsc::channel::<String>();

        fn spawn_reader<R: std::io::Read + Send + 'static>(
            reader: R,
            tx: std::sync::mpsc::Sender<String>,
        ) -> thread::JoinHandle<()> {
            thread::spawn(move || {
                for line in BufReader::new(reader).lines().map_while(Result::ok) {
                    if tx.send(line).is_err() {
                        break;
                    }
                }
            })
        }

        let stdout_reader = spawn_reader(stdout, line_tx.clone());
        let stderr_reader = spawn_reader(stderr, line_tx);

        for line in line_rx {
            on_line(line);
        }

        let status = child.wait()?;
        let _ = stdout_reader.join();
        let _ = stderr_reader.join();

        if status.success() {
            Ok(())
        } else {
            anyhow::bail!("Command failed with status {}", status.code().unwrap_or(-1))
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Mock Implementation for Testing
// ═══════════════════════════════════════════════════════════════════════════════

#[cfg(test)]
pub use mock::*;

#[cfg(test)]
pub mod mock {
    use super::*;
    use std::collections::VecDeque;

    /// Helper to create a mock VmRecord for testing
    pub fn mock_vm_record(id: &str) -> VmRecord {
        VmRecord {
            id: id.to_string(),
            hostname: "vm".to_string(),
            username: "ubuntu".to_string(),
            ssh_port: 2222,
            private_key_path: "/tmp/test_key".to_string(),
            disk_path: "/tmp/disk.qcow2".to_string(),
            seed_iso_path: "/tmp/seed.iso".to_string(),
            pid: Some(12345),
            sidecar_pids: Vec::new(),
            status: vm::VmStatus::Running,
            created_at: 0,
            host_profile: "test".to_string(),
        }
    }

    /// Mock implementation for unit testing.
    ///
    /// # Example
    /// ```ignore
    /// let mock = MockVmOps::new()
    ///     .with_run_ok("vm0")
    ///     .with_ssh_ok("output")
    ///     .with_stream_outputs(vec!["line1", "line2"]);
    /// ```
    pub struct MockVmOps {
        /// Record to return from run()
        run_record: Mutex<Option<VmRecord>>,
        /// Error message to return from run() (takes precedence over record)
        run_error: Mutex<Option<String>>,
        /// Captured options for run() calls
        run_calls: Mutex<Vec<VmOptions>>,

        /// Queue of results for ssh() calls
        ssh_results: Mutex<VecDeque<Result<String>>>,

        /// Error message for cp_to()
        cp_to_error: Mutex<Option<String>>,

        /// Error message for cp_from()
        cp_from_error: Mutex<Option<String>>,

        /// Error message for rm()
        rm_error: Mutex<Option<String>>,

        /// Queue of output lines for ssh_stream()
        stream_outputs: Mutex<VecDeque<Vec<String>>>,

        // --- Call Recording ---
        pub ssh_commands: Mutex<Vec<(String, String)>>,
        pub cp_to_calls: Mutex<Vec<(String, String, String, bool)>>,
        pub cp_from_calls: Mutex<Vec<(String, String, String, bool)>>,
        pub rm_calls: Mutex<Vec<String>>,
        pub stream_commands: Mutex<Vec<String>>,
    }

    impl MockVmOps {
        pub fn new() -> Self {
            Self {
                run_record: Mutex::new(Some(mock_vm_record("vm0"))),
                run_error: Mutex::new(None),
                run_calls: Mutex::new(Vec::new()),
                ssh_results: Mutex::new(VecDeque::new()),
                cp_to_error: Mutex::new(None),
                cp_from_error: Mutex::new(None),
                rm_error: Mutex::new(None),
                stream_outputs: Mutex::new(VecDeque::new()),
                ssh_commands: Mutex::new(Vec::new()),
                cp_to_calls: Mutex::new(Vec::new()),
                cp_from_calls: Mutex::new(Vec::new()),
                rm_calls: Mutex::new(Vec::new()),
                stream_commands: Mutex::new(Vec::new()),
            }
        }

        /// Configure run() to return success with given VM ID
        pub fn with_run_ok(self, vm_id: &str) -> Self {
            *self.run_record.lock().unwrap() = Some(mock_vm_record(vm_id));
            *self.run_error.lock().unwrap() = None;
            self
        }

        /// Configure run() to return an error
        pub fn with_run_err(self, msg: &str) -> Self {
            *self.run_error.lock().unwrap() = Some(msg.to_string());
            self
        }

        /// Add a successful ssh() response
        pub fn with_ssh_ok(self, output: &str) -> Self {
            self.ssh_results
                .lock()
                .unwrap()
                .push_back(Ok(output.to_string()));
            self
        }

        /// Add a failing ssh() response
        pub fn with_ssh_err(self, msg: &str) -> Self {
            self.ssh_results
                .lock()
                .unwrap()
                .push_back(Err(anyhow::anyhow!(msg.to_string())));
            self
        }

        /// Configure cp_to() to fail
        pub fn with_cp_to_err(self, msg: &str) -> Self {
            *self.cp_to_error.lock().unwrap() = Some(msg.to_string());
            self
        }

        /// Configure cp_from() to fail
        pub fn with_cp_from_err(self, msg: &str) -> Self {
            *self.cp_from_error.lock().unwrap() = Some(msg.to_string());
            self
        }

        /// Configure rm() to fail
        pub fn with_rm_err(self, msg: &str) -> Self {
            *self.rm_error.lock().unwrap() = Some(msg.to_string());
            self
        }

        /// Add output lines for ssh_stream()
        pub fn with_stream_outputs(self, lines: Vec<&str>) -> Self {
            self.stream_outputs
                .lock()
                .unwrap()
                .push_back(lines.iter().map(|s| s.to_string()).collect());
            self
        }

        /// Get all recorded ssh commands
        pub fn ssh_commands(&self) -> Vec<(String, String)> {
            self.ssh_commands.lock().unwrap().clone()
        }

        /// Get all recorded rm calls
        pub fn rm_calls(&self) -> Vec<String> {
            self.rm_calls.lock().unwrap().clone()
        }

        /// Get all run options calls
        pub fn run_calls(&self) -> Vec<VmOptions> {
            self.run_calls.lock().unwrap().clone()
        }

        /// Clear all recorded calls
        pub fn clear_calls(&self) {
            self.run_calls.lock().unwrap().clear();
            self.ssh_commands.lock().unwrap().clear();
            self.cp_to_calls.lock().unwrap().clear();
            self.cp_from_calls.lock().unwrap().clear();
            self.rm_calls.lock().unwrap().clear();
            self.stream_commands.lock().unwrap().clear();
        }
    }

    impl Default for MockVmOps {
        fn default() -> Self {
            Self::new()
        }
    }

    impl VmOps for MockVmOps {
        async fn run(&self, options: VmOptions) -> Result<VmRecord> {
            self.run_calls.lock().unwrap().push(options);
            if let Some(err) = self.run_error.lock().unwrap().as_ref() {
                return Err(anyhow::anyhow!(err.clone()));
            }
            self.run_record
                .lock()
                .unwrap()
                .clone()
                .ok_or_else(|| anyhow::anyhow!("no run record configured"))
        }

        async fn ssh(&self, vm_id: &str, command: &str) -> Result<String> {
            self.ssh_commands
                .lock()
                .unwrap()
                .push((vm_id.to_string(), command.to_string()));
            let mut results = self.ssh_results.lock().unwrap();
            results.pop_front().unwrap_or(Ok(String::new()))
        }

        fn cp_to(&self, vm_id: &str, local: &str, remote: &str, recursive: bool) -> Result<()> {
            self.cp_to_calls.lock().unwrap().push((
                vm_id.to_string(),
                local.to_string(),
                remote.to_string(),
                recursive,
            ));
            if let Some(err) = self.cp_to_error.lock().unwrap().as_ref() {
                return Err(anyhow::anyhow!(err.clone()));
            }
            Ok(())
        }

        fn cp_from(&self, vm_id: &str, remote: &str, local: &str, recursive: bool) -> Result<()> {
            self.cp_from_calls.lock().unwrap().push((
                vm_id.to_string(),
                remote.to_string(),
                local.to_string(),
                recursive,
            ));
            if let Some(err) = self.cp_from_error.lock().unwrap().as_ref() {
                return Err(anyhow::anyhow!(err.clone()));
            }
            Ok(())
        }

        fn rm(&self, vm_id: &str) -> Result<()> {
            self.rm_calls.lock().unwrap().push(vm_id.to_string());
            if let Some(err) = self.rm_error.lock().unwrap().as_ref() {
                return Err(anyhow::anyhow!(err.clone()));
            }
            Ok(())
        }

        fn ssh_stream<F>(&self, _record: &VmRecord, command: &str, mut on_line: F) -> Result<()>
        where
            F: FnMut(String) + Send,
        {
            self.stream_commands
                .lock()
                .unwrap()
                .push(command.to_string());
            let outputs = self.stream_outputs.lock().unwrap();
            let lines = outputs.front().cloned().unwrap_or_default();
            drop(outputs);

            for line in lines {
                on_line(line);
            }
            Ok(())
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Unit Tests
// ═══════════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mock_vm_ops_run_returns_configured_result() {
        let mock = MockVmOps::new().with_run_ok("test-vm");

        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(mock.run(VmOptions::default())).unwrap();

        assert_eq!(result.id, "test-vm");
    }

    #[test]
    fn mock_vm_ops_run_returns_error() {
        let mock = MockVmOps::new().with_run_err("failed to start");

        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(mock.run(VmOptions::default()));

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("failed to start"));
    }

    #[test]
    fn mock_vm_ops_ssh_records_commands() {
        let mock = MockVmOps::new()
            .with_ssh_ok("output1")
            .with_ssh_ok("output2");

        let rt = tokio::runtime::Runtime::new().unwrap();
        let r1 = rt.block_on(mock.ssh("vm0", "ls")).unwrap();
        let r2 = rt.block_on(mock.ssh("vm0", "pwd")).unwrap();

        assert_eq!(r1, "output1");
        assert_eq!(r2, "output2");

        let commands = mock.ssh_commands();
        assert_eq!(commands.len(), 2);
        assert_eq!(commands[0], ("vm0".to_string(), "ls".to_string()));
        assert_eq!(commands[1], ("vm0".to_string(), "pwd".to_string()));
    }

    #[test]
    fn mock_vm_ops_cp_to_records_calls() {
        let mock = MockVmOps::new();

        mock.cp_to("vm0", "/local/path", "/remote/path", true)
            .unwrap();

        let calls = mock.cp_to_calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(
            calls[0],
            (
                "vm0".to_string(),
                "/local/path".to_string(),
                "/remote/path".to_string(),
                true
            )
        );
    }

    #[test]
    fn mock_vm_ops_rm_records_calls() {
        let mock = MockVmOps::new();

        mock.rm("vm0").unwrap();
        mock.rm("vm1").unwrap();

        let calls = mock.rm_calls();
        assert_eq!(calls, vec!["vm0", "vm1"]);
    }

    #[test]
    fn mock_vm_ops_ssh_stream_outputs_lines() {
        let mock = MockVmOps::new().with_stream_outputs(vec!["line1", "line2", "line3"]);

        let record = mock_vm_record("vm0");

        let mut collected = Vec::new();
        mock.ssh_stream(&record, "echo test", |line| collected.push(line))
            .unwrap();

        assert_eq!(collected, vec!["line1", "line2", "line3"]);
    }
}
