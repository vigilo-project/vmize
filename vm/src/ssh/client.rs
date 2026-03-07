use anyhow::{Context, Result, bail};
use openssh::{KnownHosts, Session, SessionBuilder};
use std::path::PathBuf;
use std::time::Duration;
use tracing::{debug, info, warn};

/// Common SSH options used when spawning `ssh` / `scp` subprocesses.
/// These suppress host-key verification to avoid interactive prompts
/// with ephemeral VMs.
pub const SSH_STRICT_OPTIONS: [&str; 6] = [
    "-o",
    "BatchMode=yes",
    "-o",
    "StrictHostKeyChecking=no",
    "-o",
    "UserKnownHostsFile=/dev/null",
];

/// SSH client for connecting to VMs
pub struct SshClient;

impl SshClient {
    /// Create a new SSH client
    pub fn new() -> Self {
        Self
    }

    /// Connect to a remote host with retry logic
    pub async fn connect_with_retry(
        &self,
        host: &str,
        port: u16,
        username: &str,
        key_path: &std::path::Path,
        max_retries: u32,
        retry_interval: Duration,
    ) -> Result<Session> {
        info!(
            "Connecting to {}@{}:{} with retry logic",
            username, host, port
        );

        let mut attempt = 0;
        loop {
            attempt += 1;

            match self.connect_once(host, port, username, key_path).await {
                Ok(session) => {
                    info!("Successfully connected to {}@{}:{}", username, host, port);
                    return Ok(session);
                }
                Err(e) => {
                    if attempt >= max_retries {
                        warn!("Failed to connect after {} attempts", max_retries);
                        return Err(e);
                    }

                    warn!(
                        "SSH connection attempt {} of {} failed: {:?}, retrying in {:?}",
                        attempt, max_retries, e, retry_interval
                    );
                    tokio::time::sleep(retry_interval).await;
                }
            }
        }
    }

    /// Attempt a single connection
    async fn connect_once(
        &self,
        host: &str,
        port: u16,
        username: &str,
        key_path: &std::path::Path,
    ) -> Result<Session> {
        let control_dir = PathBuf::from(format!("/tmp/.ssh-mux-{}", username));
        std::fs::create_dir_all(&control_dir).with_context(|| {
            format!(
                "Failed to create SSH mux directory: {}",
                control_dir.display()
            )
        })?;

        let mut builder = SessionBuilder::default();
        builder.user(username.to_string());
        builder.keyfile(key_path);
        builder.port(port);
        builder.known_hosts_check(KnownHosts::Accept);
        builder.user_known_hosts_file("/dev/null");
        builder.control_directory(control_dir);

        let session = builder
            .connect(host)
            .await
            .context("Failed to establish SSH session")?;

        Ok(session)
    }

    /// Execute a command on the remote host and capture output
    pub async fn execute_command(&self, session: &Session, command: &str) -> Result<String> {
        debug!("Executing command: {}", command);

        let output = session
            .command("sh")
            .arg("-lc")
            .arg(command)
            .output()
            .await
            .context("Failed to execute command")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("Command failed with status {}: {}", output.status, stderr);
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        Ok(stdout.to_string())
    }

    /// Execute a command and stream output to the host's stdout/stderr in
    /// real time.  Uses a raw `ssh` process because the openssh crate's
    /// `native-mux` backend does not support piping stdout from `spawn()`.
    pub fn execute_command_stream_raw(
        &self,
        host: &str,
        port: u16,
        username: &str,
        key_path: &std::path::Path,
        command: &str,
    ) -> Result<()> {
        debug!("Executing command (streaming-raw): {}", command);

        let port_str = port.to_string();
        let user_host = format!("{}@{}", username, host);
        let mut args: Vec<&str> = vec![
            "-i",
            key_path.to_str().context("Invalid key path")?,
            "-p",
            &port_str,
        ];
        args.extend_from_slice(&SSH_STRICT_OPTIONS);
        args.extend_from_slice(&["-o", "ConnectTimeout=10", &user_host, "--", command]);

        let status = std::process::Command::new("ssh")
            .args(&args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::inherit())
            .stderr(std::process::Stdio::inherit())
            .status()
            .context("Failed to run ssh")?;

        if !status.success() {
            bail!("Command failed with status {}", status.code().unwrap_or(-1));
        }

        Ok(())
    }
}

impl Default for SshClient {
    fn default() -> Self {
        Self::new()
    }
}
