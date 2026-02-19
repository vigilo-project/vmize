use super::QemuConfig;
use crate::process::{is_process_absent_error, is_qemu_process};
use anyhow::{Context, Result, bail};
use std::process::Child;
use std::time::{Duration, Instant};
use std::{fs, thread};
use tracing::{debug, info};

/// QEMU process manager
pub struct QemuRunner {
    child: Option<Child>,
}

impl QemuRunner {
    /// Create a new QEMU runner
    pub fn new() -> Self {
        Self { child: None }
    }

    /// Start QEMU with the given configuration
    pub fn start(&mut self, config: &QemuConfig) -> Result<u32> {
        info!("Starting QEMU VM");

        // Validate configuration
        config.validate().context("Invalid QEMU configuration")?;

        // Build command
        let mut cmd = config
            .build_command()
            .context("Failed to build QEMU command")?;

        debug!("QEMU command: {:?}", cmd);

        if let Some(pid_file) = config.pid_file_path() {
            let _ = fs::remove_file(pid_file);
        }

        // Start the process
        let mut child = cmd.spawn().context("Failed to start QEMU process")?;
        let child_pid = child.id();

        let mut pid = child_pid;

        if let Some(pid_file) = config.pid_file_path() {
            let deadline = Instant::now() + Duration::from_secs(3);
            loop {
                if let Ok(output) = fs::read_to_string(pid_file)
                    && let Ok(pid_from_file) = output.trim().parse::<u32>()
                {
                    pid = pid_from_file;
                    break;
                }

                if Instant::now() > deadline {
                    break;
                }

                if let Some(status) = child
                    .try_wait()
                    .context("Failed to inspect child process")?
                {
                    bail!("QEMU exited while starting (status: {}).", status);
                }

                thread::sleep(Duration::from_millis(50));
            }
        }

        info!("QEMU VM started with PID: {}", pid);

        self.child = Some(child);
        Ok(pid)
    }
}

impl Default for QemuRunner {
    fn default() -> Self {
        Self::new()
    }
}

fn kill_qemu_process(pid: u32, signal: &str) -> Result<()> {
    info!("Stopping QEMU process with PID ({}): {}", signal, pid);

    let output = std::process::Command::new("kill")
        .arg(format!("-{signal}"))
        .arg(pid.to_string())
        .output()
        .with_context(|| format!("Failed to execute kill -{signal} command"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if !is_process_absent_error(&stderr) {
            bail!("Failed to kill process {}: {}", pid, stderr);
        }
    }

    Ok(())
}

/// Stop a QEMU process by PID
pub fn stop_qemu_by_pid(pid: u32) -> Result<()> {
    if !is_qemu_process(pid) {
        bail!(
            "Refusing to stop PID {}: process is not a QEMU process",
            pid
        );
    }

    kill_qemu_process(pid, "TERM")
}

/// Force stop a QEMU process by PID.
pub fn force_stop_qemu_by_pid(pid: u32) -> Result<()> {
    if !is_qemu_process(pid) {
        bail!(
            "Refusing to stop PID {}: process is not a QEMU process",
            pid
        );
    }

    kill_qemu_process(pid, "KILL")
}

#[cfg(test)]
mod tests {
    use crate::process::is_process_running;

    #[test]
    fn test_is_process_running() {
        assert!(is_process_running(std::process::id()));
        assert!(!is_process_running(999999));
    }
}
