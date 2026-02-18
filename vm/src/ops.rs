use crate::cloud_init::{CloudInitSeed, IsoCreator};
use crate::config::Config;
use crate::image::{copy_disk_image, ImageDownloader};
use crate::platform::HostProfile;
use crate::process::is_process_alive;
use crate::progress::{sp_complete, sp_fail, sp_start, StepProgress};
use crate::qemu::{QemuConfig, QemuRunner};
use crate::ssh::SshClient;
use crate::vm::{
    keep_key_paths, list_vm_records, next_vm_id, read_vm_record, remove_vm_instance,
    reserve_ssh_port, ssh_port_locks_dir, stop_qemu_and_wait, write_vm_record, VmRecord,
    VmRuntimeStatus, VmStatus,
};
use anyhow::{bail, Context, Result};
use std::io::ErrorKind;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::net::TcpStream;
use tokio::time::timeout;
use tracing::{debug, info};

pub type ProgressCallback = Option<Box<dyn Fn(u8, u8, &str) + Send>>;

pub struct RunOptions {
    pub username: Option<String>,
    pub ssh_port: Option<u16>,
    pub memory: Option<String>,
    pub cpus: Option<u32>,
    pub disk_size: Option<String>,
    pub force_download: bool,
    pub image_url: Option<String>,
    pub verbose: bool,
    /// Show indicatif progress spinners during VM startup.
    /// Defaults to `true`. Set to `false` to suppress terminal UI
    /// (e.g. when the caller draws its own UI).
    pub show_progress: bool,
    /// Optional callback invoked on each VM startup step.
    /// Arguments: `(current_step, total_steps, message)`.
    pub on_progress: ProgressCallback,
}

impl Default for RunOptions {
    fn default() -> Self {
        Self {
            username: None,
            ssh_port: None,
            memory: None,
            cpus: None,
            disk_size: None,
            force_download: false,
            image_url: None,
            verbose: false,
            show_progress: true,
            on_progress: None,
        }
    }
}

pub async fn run(options: RunOptions) -> Result<VmRecord> {
    let config = Config::default();
    run_vm_inner(&config, options).await
}

/// Read a VM record and validate it is running with a valid SSH key.
/// Returns the validated record and its private key path.
fn require_running_vm(config: &Config, id: &str) -> Result<(VmRecord, PathBuf)> {
    let instances_dir = config.instances_dir();
    let record = read_vm_record(&instances_dir, id)?;

    if record.runtime_status() == VmRuntimeStatus::Stopped {
        bail!("VM '{}' is stopped", id);
    }

    let key_path = PathBuf::from(&record.private_key_path);
    if !key_path.exists() {
        bail!("SSH key not found at: {}", key_path.display());
    }

    Ok((record, key_path))
}

pub async fn ssh_with_config(config: &Config, id: &str, command: &str) -> Result<String> {
    let (record, key_path) = require_running_vm(config, id)?;

    let ssh_client = SshClient::new();
    let session = ssh_client
        .connect_with_retry(
            "127.0.0.1",
            record.ssh_port,
            &record.username,
            &key_path,
            10,
            Duration::from_secs(2),
        )
        .await
        .context("Failed to connect to SSH server")?;

    info!("Executing command: {}", command);
    let output = ssh_client.execute_command(&session, command).await?;
    Ok(output)
}

pub fn ssh_stream_with_config(config: &Config, id: &str, command: &str) -> Result<()> {
    let (record, key_path) = require_running_vm(config, id)?;

    info!("Executing command (streaming): {}", command);
    let ssh_client = SshClient::new();
    ssh_client.execute_command_stream_raw(
        "127.0.0.1",
        record.ssh_port,
        &record.username,
        &key_path,
        command,
    )
}

pub fn cp_to_with_config(
    config: &Config,
    id: &str,
    local: &str,
    remote: &str,
    recursive: bool,
) -> Result<()> {
    scp_transfer_inner(
        config,
        id,
        local,
        remote,
        recursive,
        ScpDirection::LocalToVm,
    )
}

pub fn cp_from_with_config(
    config: &Config,
    id: &str,
    remote: &str,
    local: &str,
    recursive: bool,
) -> Result<()> {
    scp_transfer_inner(
        config,
        id,
        remote,
        local,
        recursive,
        ScpDirection::VmToLocal,
    )
}

pub fn rm_with_config(config: &Config, id: &str) -> Result<()> {
    let instances_dir = config.instances_dir();
    let records = list_vm_records(&instances_dir)?;
    let mut keep_records = Vec::new();
    let mut target_record = None;

    for (existing_id, record) in records {
        if existing_id == id {
            target_record = Some(record);
            continue;
        }
        keep_records.push((existing_id, record));
    }

    let record = target_record.with_context(|| format!("Failed to find VM record '{}'", id))?;
    let keep = keep_key_paths(&keep_records);
    remove_vm_instance(&instances_dir, id, &record, &keep)?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

enum ScpDirection {
    LocalToVm,
    VmToLocal,
}

fn scp_transfer_inner(
    config: &Config,
    id: &str,
    src: &str,
    dest: &str,
    recursive: bool,
    direction: ScpDirection,
) -> Result<()> {
    let (record, key_path) = require_running_vm(config, id)?;

    let key_str = key_path.to_str().context("Invalid key path")?;
    let port_str = record.ssh_port.to_string();
    let remote_spec = format!("{}@127.0.0.1", record.username);

    let mut args = Vec::new();
    if recursive {
        args.push("-r");
    }
    args.extend_from_slice(&[
        "-i",
        key_str,
        "-P",
        &port_str,
        "-o",
        "StrictHostKeyChecking=no",
        "-o",
        "UserKnownHostsFile=/dev/null",
    ]);

    let remote_path = format!("{}:{}", remote_spec, dest);
    let remote_src = format!("{}:{}", remote_spec, src);

    match direction {
        ScpDirection::LocalToVm => {
            args.push(src);
            args.push(&remote_path);
        }
        ScpDirection::VmToLocal => {
            args.push(&remote_src);
            args.push(dest);
        }
    }

    let status = Command::new("scp")
        .args(&args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .context("Failed to run scp")?;

    if status.success() {
        Ok(())
    } else {
        bail!("scp exited with status {}", status)
    }
}

async fn wait_for_ssh_port(pid: u32, port: u16, startup_timeout: Duration) -> Result<()> {
    let mut attempt: u32 = 0;
    let deadline = Instant::now() + startup_timeout;
    let start = Instant::now();
    while Instant::now() < deadline {
        attempt += 1;
        if !is_process_alive(pid) {
            bail!(
                "QEMU process {} exited before SSH became available; host forwarding may have failed.",
                pid
            );
        }

        if timeout(
            Duration::from_millis(250),
            TcpStream::connect(("127.0.0.1", port)),
        )
        .await
        .is_ok()
        {
            info!(
                "SSH port {} accepted connection after {} attempt(s) ({:?})",
                port,
                attempt,
                start.elapsed()
            );
            return Ok(());
        }

        if attempt.is_multiple_of(4) {
            info!(
                "Waiting for SSH on 127.0.0.1:{} (attempt {}, elapsed {:?})",
                port,
                attempt,
                start.elapsed()
            );
        }

        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    bail!(
        "SSH did not become reachable on 127.0.0.1:{} within {:?}; host forwarding likely failed.",
        port,
        startup_timeout
    )
}

fn log_progress(step: u8, total: u8, message: &str) {
    info!("[{}/{}] {}", step, total, message);
}

fn notify_progress(cb: &ProgressCallback, step: u8, total: u8, msg: &str) {
    if let Some(f) = cb {
        f(step, total, msg);
    }
}

/// Prepare the base cloud image, downloading if needed.
/// Returns the local path to the verified image.
async fn prepare_cloud_image(
    image_url: &str,
    images_dir: &std::path::Path,
    force_download: bool,
    sp: &mut Option<StepProgress>,
    on_progress: &ProgressCallback,
) -> Result<(PathBuf, bool)> {
    sp_start(sp, "Cloud image");
    notify_progress(on_progress, 3, 8, "Cloud image");
    log_progress(3, 8, "Preparing base cloud image");

    let image_filename = image_url.split('/').next_back().unwrap_or("ubuntu.img");
    let image_path = images_dir.join(image_filename);

    let downloaded = if force_download || !ImageDownloader::verify_image(&image_path)? {
        info!("Downloading Ubuntu Cloud image...");
        let downloader = ImageDownloader::new();
        let mp = StepProgress::multi_progress_opt(sp);
        downloader.download(image_url, &image_path, mp).await?;
        true
    } else {
        info!("Image already exists, skipping download");
        false
    };

    let status = if downloaded { "downloaded" } else { "cached" };
    sp_complete(sp, status);
    notify_progress(on_progress, 3, 8, &format!("Cloud image — {status}"));
    Ok((image_path, downloaded))
}

/// Wait for SSH to become reachable and verify the VM responds to commands.
/// Returns the hostname reported by the VM.
async fn verify_ssh_connection(
    record: &VmRecord,
    key_path: &std::path::Path,
    host_profile: &HostProfile,
    sp: &mut Option<StepProgress>,
    on_progress: &ProgressCallback,
) -> Result<String> {
    sp_start(sp, "SSH connected");
    notify_progress(on_progress, 8, 8, "SSH connected");
    log_progress(8, 8, "Waiting for SSH and verifying VM");

    let tcg_enabled = host_profile.machine_type.contains("accel=tcg");
    let ssh_wait_timeout = if tcg_enabled {
        Duration::from_secs(120)
    } else {
        Duration::from_secs(30)
    };
    let ssh_retry_attempts = if tcg_enabled { 30 } else { 12 };

    info!("Waiting for SSH to be available...");
    if let Err(err) = wait_for_ssh_port(
        record.pid.unwrap_or_default(),
        record.ssh_port,
        ssh_wait_timeout,
    )
    .await
    {
        sp_fail(sp, "port unreachable");
        return Err(err.context("QEMU failed to become reachable over forwarded SSH"));
    }

    let ssh_client = SshClient::new();
    let session = match ssh_client
        .connect_with_retry(
            "127.0.0.1",
            record.ssh_port,
            &record.username,
            key_path,
            ssh_retry_attempts,
            Duration::from_secs(2),
        )
        .await
    {
        Ok(s) => s,
        Err(err) => {
            sp_fail(sp, "connection failed");
            return Err(err.context("Failed to connect to VM via SSH"));
        }
    };

    let output = match ssh_client.execute_command(&session, "hostname").await {
        Ok(o) => o,
        Err(err) => {
            sp_fail(sp, "hostname check failed");
            return Err(err.context("Failed to verify VM startup"));
        }
    };
    info!("VM hostname: {}", output.trim());

    if record.runtime_status() != VmRuntimeStatus::Running {
        sp_fail(sp, "process not running");
        bail!("VM process is not running after startup");
    }

    sp_complete(sp, output.trim());
    notify_progress(
        on_progress,
        8,
        8,
        &format!("SSH connected — {}", output.trim()),
    );
    Ok(output)
}

async fn run_vm_inner(config: &Config, options: RunOptions) -> Result<VmRecord> {
    let username = options.username.unwrap_or_else(|| "ubuntu".to_string());
    let ssh_port = options.ssh_port.unwrap_or(2222);
    let memory = options.memory.unwrap_or_else(|| "4G".to_string());
    let cpus = options.cpus.unwrap_or(2);
    let disk_size = options.disk_size;
    let force_download = options.force_download;
    let verbose = options.verbose;
    let show_progress = options.show_progress;
    let on_progress = options.on_progress;
    let custom_image_url = options.image_url;

    let hostname = "vm".to_string();
    let mut sp = if verbose || !show_progress {
        None
    } else {
        Some(StepProgress::new(8))
    };

    // Step 1: Host profile
    sp_start(&mut sp, "Host profile");
    notify_progress(&on_progress, 1, 8, "Host profile");
    log_progress(1, 8, "Resolving host profile");
    info!("Running VM: {} with user: {}", hostname, username);

    let host_profile = HostProfile::detect()?;
    info!(
        "Using host profile: {}/{} ({}, {}, {})",
        host_profile.os,
        host_profile.arch,
        host_profile.qemu_binary,
        host_profile.machine_type,
        host_profile.cpu_type
    );
    let detail = format!(
        "{}/{} ({})",
        host_profile.os, host_profile.arch, host_profile.cpu_type
    );
    sp_complete(&mut sp, &detail);
    notify_progress(&on_progress, 1, 8, &format!("Host profile — {detail}"));

    // Step 2: Directories
    sp_start(&mut sp, "Directories");
    notify_progress(&on_progress, 2, 8, "Directories");
    let effective_image_url =
        custom_image_url.unwrap_or_else(|| host_profile.image_url.to_string());
    config.ensure_base_dir()?;
    let base_dir = config.base_dir.display();
    log_progress(
        2,
        8,
        &format!("Preparing directory layout and local configuration in {base_dir}"),
    );

    let images_dir = config.images_dir();
    let instances_dir = config.instances_dir();
    let keys_dir = config.keys_dir();

    std::fs::create_dir_all(&images_dir)?;
    std::fs::create_dir_all(&instances_dir)?;
    std::fs::create_dir_all(&keys_dir)?;
    sp_complete(&mut sp, &format!("ready ({base_dir})"));
    notify_progress(&on_progress, 2, 8, "Directories — ready");

    // Step 3: Cloud image
    let (image_path, _downloaded) = prepare_cloud_image(
        &effective_image_url,
        &images_dir,
        force_download,
        &mut sp,
        &on_progress,
    )
    .await?;

    // Step 4: SSH key pair
    sp_start(&mut sp, "SSH key pair");
    notify_progress(&on_progress, 4, 8, "SSH key pair");
    log_progress(4, 8, "Creating or reusing SSH key pair");
    info!("Generating SSH key pair...");
    let key_manager = crate::ssh::SshKeyManager::new(&keys_dir);
    let (private_key_path, public_key) = key_manager.generate_key_pair(&hostname)?;

    debug!("Private key: {}", private_key_path.display());
    debug!("Public key: {}", public_key);
    sp_complete(&mut sp, "ready");
    notify_progress(&on_progress, 4, 8, "SSH key pair — ready");

    // Steps 5-7 happen inside the port-retry loop
    let requested_ssh_port = ssh_port;
    let mut port_reservation = reserve_ssh_port(&instances_dir, requested_ssh_port)
        .context("Failed to reserve an SSH port for this VM")?;
    let mut selected_ssh_port = port_reservation.port();
    let mut port_attempts = 0u8;
    let vm_record = loop {
        let vm_id = next_vm_id(&instances_dir).context("Failed to generate VM id")?;
        let vm_dir = instances_dir.join(&vm_id);

        info!("VM ID: {}", vm_id);
        info!("VM directory: {}", vm_dir.display());

        match std::fs::create_dir(&vm_dir) {
            Ok(_) => {}
            Err(err) if err.kind() == ErrorKind::AlreadyExists => continue,
            Err(err) => {
                bail!(
                    "Failed to create VM directory {}: {}",
                    vm_dir.display(),
                    err
                );
            }
        }

        // Step 5: Cloud-init seed
        sp_start(&mut sp, "Cloud-init seed");
        notify_progress(&on_progress, 5, 8, "Cloud-init seed");
        info!("Creating cloud-init seed...");
        let seed =
            CloudInitSeed::with_config(hostname.clone(), username.clone(), public_key.clone());

        let metadata_path = vm_dir.join("meta-data");
        let userdata_path = vm_dir.join("user-data");

        seed.write_metadata(&metadata_path)?;
        seed.write_userdata(&userdata_path)?;

        info!("Creating NOCLOUD ISO...");
        let iso_creator = IsoCreator::new()?;
        let iso_path = vm_dir.join("seed.iso");
        iso_creator.create_nocloud_iso(&metadata_path, &userdata_path, &iso_path)?;
        sp_complete(&mut sp, "created");
        notify_progress(&on_progress, 5, 8, "Cloud-init seed — created");

        // Step 6: Disk image
        sp_start(&mut sp, "Disk image");
        notify_progress(&on_progress, 6, 8, "Disk image");
        info!("Creating VM disk image...");
        let vm_disk_path = vm_dir.join("disk.qcow2");
        copy_disk_image(&image_path, &vm_disk_path, disk_size.as_deref())?;
        sp_complete(&mut sp, "created");
        notify_progress(&on_progress, 6, 8, "Disk image — created");

        if selected_ssh_port != requested_ssh_port {
            info!(
                "Requested SSH port {} is unavailable; using next available port {}.",
                requested_ssh_port, selected_ssh_port
            );
        }

        // Step 7: QEMU start
        let qemu_label = if port_attempts > 0 {
            format!("Starting QEMU (retry {port_attempts}, port {selected_ssh_port})")
        } else {
            "Starting QEMU".to_string()
        };
        sp_start(&mut sp, &qemu_label);
        notify_progress(&on_progress, 7, 8, &qemu_label);
        info!("Building QEMU configuration...");
        let qemu_config = QemuConfig::from_host_profile(&host_profile)
            .memory(memory.clone())
            .cpus(cpus)
            .disk_image(&vm_disk_path)
            .cloud_init_iso(&iso_path)
            .ssh_port(selected_ssh_port)
            .pid_file(vm_dir.join("qemu.pid"))
            .name(vm_id.clone())
            .display(false)
            .daemonize(!verbose);

        info!("Starting QEMU VM...");
        let mut runner = QemuRunner::new();
        let pid = match runner.start(&qemu_config) {
            Ok(pid) => pid,
            Err(err) => {
                sp_fail(&mut sp, &err.to_string());
                return Err(err);
            }
        };
        info!("VM started with PID: {}", pid);

        let vm_record = VmRecord {
            id: vm_id.clone(),
            hostname: hostname.clone(),
            username: username.clone(),
            ssh_port: selected_ssh_port,
            private_key_path: private_key_path.to_string_lossy().to_string(),
            disk_path: vm_disk_path.to_string_lossy().to_string(),
            seed_iso_path: iso_path.to_string_lossy().to_string(),
            pid: Some(pid),
            status: VmStatus::Running,
            created_at: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .context("Failed to determine creation time")?
                .as_secs(),
            host_profile: format!(
                "{}/{} ({}, {})",
                host_profile.os,
                host_profile.arch,
                host_profile.machine_type,
                host_profile.cpu_type,
            ),
        };

        tokio::time::sleep(Duration::from_millis(500)).await;
        if vm_record.runtime_status() == VmRuntimeStatus::Running {
            let qemu_detail = format!("PID {pid}, port {selected_ssh_port}");
            sp_complete(&mut sp, &qemu_detail);
            notify_progress(
                &on_progress,
                7,
                8,
                &format!("Starting QEMU — {qemu_detail}"),
            );
            break vm_record;
        }

        if !crate::vm::port::is_port_free(selected_ssh_port) {
            port_attempts = port_attempts
                .checked_add(1)
                .context("Failed to track SSH port retry attempts")?;
            if port_attempts > 20 {
                sp_fail(&mut sp, "no available port");
                bail!(
                    "Could not find an available SSH port after {} attempts starting from {}.",
                    port_attempts,
                    requested_ssh_port
                );
            }

            if let Err(err) = stop_qemu_and_wait(pid, Duration::from_secs(3)) {
                info!("Unable to stop failed VM on pid {}: {}", pid, err);
            }

            let next_preferred = selected_ssh_port.checked_add(1).context(format!(
                "No available SSH ports found starting from {}.",
                selected_ssh_port
            ))?;
            if let Err(err) = std::fs::remove_dir_all(&vm_dir) {
                info!(
                    "Failed to remove failed VM directory {}: {}",
                    vm_dir.display(),
                    err
                );
            }
            drop(port_reservation);
            port_reservation = reserve_ssh_port(&instances_dir, next_preferred)?;
            selected_ssh_port = port_reservation.port();
            continue;
        }

        if let Err(err) = std::fs::remove_dir_all(&vm_dir) {
            info!(
                "Failed to remove failed VM directory {}: {}",
                vm_dir.display(),
                err
            );
        }

        sp_fail(&mut sp, "process exited");
        bail!(
            "QEMU failed to stay running after startup. SSH port {} may already be in use or QEMU failed to initialize.\
             Use --verbose for startup output.",
            selected_ssh_port
        );
    };

    // Persist record before SSH verification so cleanup tools (e.g. `vm rm`) can
    // find the VM even if SSH fails.  On SSH failure we kill QEMU and remove the
    // instance ourselves before propagating the error.
    write_vm_record(&instances_dir, &vm_record)?;

    // Step 8: SSH connection
    if let Err(ssh_err) = verify_ssh_connection(
        &vm_record,
        &private_key_path,
        &host_profile,
        &mut sp,
        &on_progress,
    )
    .await
    {
        info!("SSH verification failed; cleaning up QEMU process and VM instance");
        if let Some(pid) = vm_record.pid {
            if let Err(e) = stop_qemu_and_wait(pid, Duration::from_secs(5)) {
                info!("Failed to stop QEMU pid {}: {}", pid, e);
            }
        }
        let vm_dir = instances_dir.join(&vm_record.id);
        if let Err(e) = std::fs::remove_dir_all(&vm_dir) {
            info!("Failed to remove VM directory {}: {}", vm_dir.display(), e);
        }
        return Err(ssh_err);
    }

    Ok(vm_record)
}

pub(crate) fn run_interactive_ssh(config: &Config, id: &str) -> Result<()> {
    let (record, key_path) = require_running_vm(config, id)?;

    let status = Command::new("ssh")
        .args([
            "-i",
            key_path.to_str().context("Invalid key path")?,
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
            &format!("{}@{}", record.username, "127.0.0.1"),
        ])
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .context("Failed to run ssh")?;

    if status.success() {
        Ok(())
    } else {
        bail!("SSH exited with status {}", status)
    }
}

pub(crate) fn clear_vms(config: &Config) -> Result<()> {
    let instances_dir = config.instances_dir();
    let keys_dir = config.keys_dir();
    let records = list_vm_records(&instances_dir)?;
    if records.is_empty() {
        info!("No VMs found to clear");
        return Ok(());
    }

    let keep = keep_key_paths(&[]);
    let mut failures = Vec::new();
    for (id, record) in records {
        if let Err(err) = remove_vm_instance(&instances_dir, &id, &record, &keep) {
            info!("Failed to remove VM {}: {}", id, err);
            failures.push((id, err.to_string()));
        }
    }

    if !failures.is_empty() {
        let mut message = String::new();
        for (idx, (id, err)) in failures.iter().enumerate() {
            if idx > 0 {
                message.push('\n');
            }
            message.push_str(id);
            message.push_str(": ");
            message.push_str(err);
        }
        bail!("Failed to clear all VMs:\n{}", message);
    }

    if instances_dir.exists() {
        std::fs::remove_dir_all(&instances_dir).with_context(|| {
            format!(
                "Failed to remove instances directory {}",
                instances_dir.display()
            )
        })?;
    }
    if keys_dir.exists() {
        std::fs::remove_dir_all(&keys_dir)
            .with_context(|| format!("Failed to remove keys directory {}", keys_dir.display()))?;
    }
    let locks_dir = ssh_port_locks_dir(&instances_dir);
    if locks_dir.exists() {
        std::fs::remove_dir_all(&locks_dir).with_context(|| {
            format!(
                "Failed to remove SSH port lock directory {}",
                locks_dir.display()
            )
        })?;
    }

    info!("Removed all VM instances and keys");
    Ok(())
}

pub(crate) fn list_vms_inner(config: &Config) -> Result<String> {
    let instances_dir = config.instances_dir();
    let records = list_vm_records(&instances_dir)?;

    if records.is_empty() {
        return Ok("No VMs found\n".to_string());
    }

    let mut output = String::new();
    output.push_str(&format!(
        "{:<36} {:<14} {:<10} {:<5} {:<8} {:<10} {:<20}\n",
        "ID", "Hostname", "User", "Port", "PID", "Status", "Created"
    ));
    output.push_str(&format!("{}\n", "-".repeat(111)));

    for (id, record) in records {
        let pid = record
            .pid
            .map(|value| value.to_string())
            .unwrap_or_else(|| "-".to_string());
        let runtime_status = record.runtime_status();
        let runtime_status_label = record.runtime_status_label();

        if runtime_status == VmRuntimeStatus::Stale && record.status == VmStatus::Running {
            continue;
        }

        output.push_str(&format!(
            "{:<36} {:<14} {:<10} {:<5} {:<8} {:<10} {:<20}\n",
            id,
            record.hostname,
            record.username,
            record.ssh_port,
            pid,
            runtime_status_label,
            record.created_at,
        ));
    }

    Ok(output)
}

enum CpEndpoint {
    Local(String),
    Remote { id: String, path: String },
}

fn parse_cp_endpoint(config: &Config, spec: &str) -> Result<CpEndpoint> {
    if let Some((id, path)) = spec.split_once(':') {
        if !id.is_empty() {
            let instances_dir = config.instances_dir();
            if read_vm_record(&instances_dir, id).is_ok() {
                return Ok(CpEndpoint::Remote {
                    id: id.to_string(),
                    path: path.to_string(),
                });
            }
        }
    }

    Ok(CpEndpoint::Local(spec.to_string()))
}

pub(crate) fn cp_transfer(config: &Config, src: &str, dest: &str, recursive: bool) -> Result<()> {
    let source = parse_cp_endpoint(config, src)?;
    let destination = parse_cp_endpoint(config, dest)?;

    match (source, destination) {
        (CpEndpoint::Local(local), CpEndpoint::Remote { id, path }) => scp_transfer_inner(
            config,
            &id,
            &local,
            &path,
            recursive,
            ScpDirection::LocalToVm,
        ),
        (CpEndpoint::Remote { id, path }, CpEndpoint::Local(local)) => scp_transfer_inner(
            config,
            &id,
            &path,
            &local,
            recursive,
            ScpDirection::VmToLocal,
        ),
        (CpEndpoint::Remote { id: src_id, .. }, CpEndpoint::Remote { id: dest_id, .. }) => {
            bail!("cp requires one local path and one VM path (got remote paths for '{src_id}' and '{dest_id}')")
        }
        (CpEndpoint::Local(_), CpEndpoint::Local(_)) => {
            bail!("cp requires one VM path in the form <vm-id>:<path>")
        }
    }
}
