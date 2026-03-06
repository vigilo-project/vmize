use crate::cloud_init::{CloudInitSeed, IsoCreator};
use crate::config::Config;
use crate::image::{ImageDownloader, copy_disk_image, detect_disk_format};
use crate::mount::{MountMode, MountSpec};
use crate::platform::HostProfile;
use crate::process::is_process_alive;
use crate::progress::{StepProgress, sp_complete, sp_fail, sp_start};
use crate::qemu::config::DiskFormat;
use crate::qemu::{QemuConfig, QemuRunner};
use crate::ssh::{SSH_STRICT_OPTIONS, SshClient};
use crate::vm::{
    VmRecord, VmRuntimeStatus, VmStatus, acquire_vm_creation_lock, keep_key_paths, list_vm_records,
    read_vm_record, remove_vm_instance, reserve_specific_ssh_port, ssh_port_for_vm_index,
    ssh_port_locks_dir, stop_qemu_and_wait, validate_vm_capacity, write_vm_record,
};
use anyhow::{Context, Result, bail};
use std::env;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::net::TcpStream;
use tokio::time::timeout;
use tracing::{debug, info};

pub type ProgressCallback = Option<Box<dyn Fn(u8, u8, &str) + Send>>;

pub struct RunOptions {
    pub username: Option<String>,
    pub memory: Option<String>,
    pub cpus: Option<u32>,
    pub disk_size: Option<String>,
    pub force_download: bool,
    pub image_url: Option<String>,
    pub kernel: Option<PathBuf>,
    pub rootfs: Option<PathBuf>,
    pub mounts: Vec<MountSpec>,
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
            memory: None,
            cpus: None,
            disk_size: None,
            force_download: false,
            image_url: None,
            kernel: None,
            rootfs: None,
            mounts: Vec::new(),
            verbose: false,
            show_progress: true,
            on_progress: None,
        }
    }
}

#[derive(Debug, Clone)]
struct VirtioFsShareRuntime {
    tag: String,
    socket_path: PathBuf,
    pid: u32,
}

#[derive(Debug, Default, Clone)]
struct VirtioFsRuntime {
    shares: Vec<VirtioFsShareRuntime>,
}

fn virtiofs_tag_for_index(index: usize) -> String {
    format!("vmizefs{index}")
}

fn shell_quote(value: &str) -> String {
    if value.is_empty() {
        return "''".to_string();
    }
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MountTransport {
    VirtioFs,
    Virtio9p,
}

fn mount_runcmd_for_spec(index: usize, mount: &MountSpec, transport: MountTransport) -> Vec<String> {
    let tag = virtiofs_tag_for_index(index);
    let guest_path = mount.guest_path.to_string_lossy().to_string();
    let quoted_guest = shell_quote(&guest_path);
    let quoted_tag = shell_quote(&tag);

    let mount_cmd = match (transport, mount.mode) {
        (MountTransport::VirtioFs, MountMode::ReadOnly) => {
            format!("mount -t virtiofs -o ro {} {}", quoted_tag, quoted_guest)
        }
        (MountTransport::VirtioFs, MountMode::ReadWrite) => {
            format!("mount -t virtiofs {} {}", quoted_tag, quoted_guest)
        }
        (MountTransport::Virtio9p, MountMode::ReadOnly) => {
            format!(
                "mount -t 9p -o trans=virtio,version=9p2000.L,ro {} {}",
                quoted_tag, quoted_guest
            )
        }
        (MountTransport::Virtio9p, MountMode::ReadWrite) => {
            format!(
                "mount -t 9p -o trans=virtio,version=9p2000.L {} {}",
                quoted_tag, quoted_guest
            )
        }
    };

    vec![format!("mkdir -p {}", quoted_guest), mount_cmd]
}

impl VirtioFsRuntime {
    fn pids(&self) -> Vec<u32> {
        self.shares.iter().map(|share| share.pid).collect()
    }
}

fn find_executable(name: &str) -> Option<PathBuf> {
    let path_var = env::var_os("PATH")?;
    for dir in env::split_paths(&path_var) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

fn resolve_virtiofsd_binary() -> Option<PathBuf> {
    find_executable("virtiofsd")
        .or_else(|| find_executable("qemu-virtiofsd"))
        .or_else(|| {
            let homebrew = PathBuf::from("/opt/homebrew/libexec/virtiofsd");
            homebrew.is_file().then_some(homebrew)
        })
}

fn wait_for_socket_or_exit(pid: u32, socket_path: &Path, timeout: Duration) -> Result<()> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if socket_path.exists() {
            return Ok(());
        }
        if !is_process_alive(pid) {
            bail!(
                "virtiofsd process {} exited before creating socket {}",
                pid,
                socket_path.display()
            );
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    bail!(
        "Timed out waiting for virtiofs socket {}",
        socket_path.display()
    )
}

fn stop_sidecar_processes(pids: &[u32]) {
    for pid in pids {
        let _ = Command::new("kill")
            .arg("-TERM")
            .arg(pid.to_string())
            .output();
    }

    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        if pids.iter().all(|pid| !is_process_alive(*pid)) {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    for pid in pids {
        if is_process_alive(*pid) {
            let _ = Command::new("kill")
                .arg("-KILL")
                .arg(pid.to_string())
                .output();
        }
    }
}

fn start_virtiofs_runtime(vm_dir: &Path, mounts: &[MountSpec]) -> Result<VirtioFsRuntime> {
    if mounts.is_empty() {
        return Ok(VirtioFsRuntime::default());
    }

    let virtiofsd = resolve_virtiofsd_binary()
        .context("virtiofsd not found (expected 'virtiofsd' or 'qemu-virtiofsd' in PATH)")?;
    let mut shares = Vec::with_capacity(mounts.len());
    let mut started_pids = Vec::with_capacity(mounts.len());

    for (index, mount) in mounts.iter().enumerate() {
        let tag = virtiofs_tag_for_index(index);
        let socket_path = vm_dir.join(format!("virtiofs-{index}.sock"));
        let _ = fs::remove_file(&socket_path);

        let child = Command::new(&virtiofsd)
            .arg("--socket-path")
            .arg(&socket_path)
            .arg("--shared-dir")
            .arg(&mount.host_path)
            .arg("--sandbox")
            .arg("none")
            .arg("--cache")
            .arg("auto")
            .arg("--inode-file-handles")
            .arg("never")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .with_context(|| {
                format!(
                    "Failed to start virtiofsd for host path {}",
                    mount.host_path.display()
                )
            })?;
        let pid = child.id();
        drop(child);

        if let Err(err) = wait_for_socket_or_exit(pid, &socket_path, Duration::from_secs(5)) {
            stop_sidecar_processes(&started_pids);
            return Err(err);
        }

        started_pids.push(pid);
        shares.push(VirtioFsShareRuntime {
            tag,
            socket_path,
            pid,
        });
    }

    Ok(VirtioFsRuntime { shares })
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
    ssh_client.execute_command(&session, command).await
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
    args.push("-O"); // Use legacy SCP protocol to avoid "unexpected filename" errors
    if recursive {
        args.push("-r");
    }
    args.extend_from_slice(&["-i", key_str, "-P", &port_str]);
    // SCP uses the same options but without BatchMode (scp handles that
    // differently) and without the -o prefix already present in the constant.
    args.extend_from_slice(&[
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
    let memory = options.memory.unwrap_or_else(|| "4G".to_string());
    let cpus = options.cpus.unwrap_or(2);
    let disk_size = options.disk_size;
    let force_download = options.force_download;
    let kernel = options.kernel;
    let rootfs = options.rootfs;
    let mounts = options.mounts;
    let verbose = options.verbose;
    let show_progress = options.show_progress;
    let on_progress = options.on_progress;
    let custom_image_url = options.image_url;

    let use_custom_kernel = match (&kernel, &rootfs) {
        (Some(_), Some(_)) => true,
        (None, None) => false,
        _ => bail!(
            "kernel and rootfs must be provided together, or neither one to use the default cloud image flow"
        ),
    };

    if use_custom_kernel {
        if custom_image_url.is_some() {
            bail!("image-url must not be used with custom kernel/rootfs mode");
        }
        if disk_size.is_some() {
            bail!("disk-size must not be used with custom kernel/rootfs mode");
        }
    }

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

    // Step 3: Cloud image or custom rootfs
    let (image_path, _downloaded) = if use_custom_kernel {
        let rootfs_path = rootfs.expect("Validated rootfs");
        sp_start(&mut sp, "Cloud image");
        notify_progress(&on_progress, 3, 8, "Cloud image");
        log_progress(3, 8, "Using custom rootfs for boot");
        sp_complete(&mut sp, "ready");
        notify_progress(&on_progress, 3, 8, "Cloud image — ready");
        (rootfs_path, false)
    } else {
        let effective_image_url =
            custom_image_url.unwrap_or_else(|| host_profile.image_url.to_string());
        prepare_cloud_image(
            &effective_image_url,
            &images_dir,
            force_download,
            &mut sp,
            &on_progress,
        )
        .await?
    };

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

    // Step 5: Validate capacity, allocate VM ID, reserve port, create directory
    // All under a global lock to prevent race conditions during concurrent VM creation
    let vm_creation_lock =
        acquire_vm_creation_lock(&instances_dir).context("Failed to acquire VM creation lock")?;

    // VM ID determines SSH port: vm0 -> 2220, vm1 -> 2221, ..., vm9 -> 2229
    let vm_index =
        validate_vm_capacity(&instances_dir).context("Failed to validate VM capacity")?;
    let vm_id = format!("vm{}", vm_index);
    let ssh_port =
        ssh_port_for_vm_index(vm_index).expect("validate_vm_capacity should ensure valid index");

    info!("VM ID: {} (SSH port: {})", vm_id, ssh_port);

    // Reserve the fixed SSH port for this VM
    let _port_reservation = reserve_specific_ssh_port(&instances_dir, ssh_port)
        .context("Failed to reserve SSH port for VM")?;

    let vm_dir = instances_dir.join(&vm_id);
    info!("VM directory: {}", vm_dir.display());

    match std::fs::create_dir(&vm_dir) {
        Ok(_) => {}
        Err(err) if err.kind() == ErrorKind::AlreadyExists => {
            bail!(
                "VM directory {} already exists - this should not happen with sequential IDs",
                vm_dir.display()
            );
        }
        Err(err) => {
            bail!(
                "Failed to create VM directory {}: {}",
                vm_dir.display(),
                err
            );
        }
    }

    // VM directory created successfully, release the global lock
    // Port reservation will be released when _port_reservation drops
    drop(vm_creation_lock);

    // Step 6: Cloud-init seed
    sp_start(&mut sp, "Cloud-init seed");
    notify_progress(&on_progress, 5, 8, "Cloud-init seed");
    info!("Creating cloud-init seed...");
    let mut seed = CloudInitSeed::with_config(hostname.clone(), username.clone(), public_key);

    let mount_transport = if host_profile.os == "macos" {
        MountTransport::Virtio9p
    } else {
        MountTransport::VirtioFs
    };

    if !mounts.is_empty() {
        let mount_commands: Vec<String> = mounts
            .iter()
            .enumerate()
            .flat_map(|(index, mount)| mount_runcmd_for_spec(index, mount, mount_transport))
            .collect();
        seed.extend_runcmd(mount_commands);
    }

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

    // Step 7: Disk image
    sp_start(&mut sp, "Disk image");
    notify_progress(&on_progress, 6, 8, "Disk image");
    let (vm_disk_path, disk_format) = if use_custom_kernel {
        info!("Using provided rootfs as VM disk image...");
        let disk_format = detect_disk_format(&image_path)?;
        (image_path.clone(), disk_format)
    } else {
        info!("Creating VM disk image...");
        let vm_disk_path = vm_dir.join("disk.qcow2");
        copy_disk_image(&image_path, &vm_disk_path, disk_size.as_deref())?;
        (vm_disk_path, DiskFormat::Qcow2)
    };
    sp_complete(&mut sp, "created");
    notify_progress(&on_progress, 6, 8, "Disk image — created");

    // Step 8: QEMU start
    sp_start(&mut sp, "Starting QEMU");
    notify_progress(&on_progress, 7, 8, "Starting QEMU");
    info!("Building QEMU configuration...");

    // Start virtiofsd on Linux; on macOS use virtio-9p (QEMU built-in, no daemon).
    let virtiofs_runtime = if mount_transport == MountTransport::VirtioFs {
        match start_virtiofs_runtime(&vm_dir, &mounts) {
            Ok(runtime) => runtime,
            Err(err) => {
                if let Err(remove_err) = fs::remove_dir_all(&vm_dir) {
                    info!(
                        "Failed to remove VM directory after virtiofs setup error {}: {}",
                        vm_dir.display(),
                        remove_err
                    );
                }
                return Err(err);
            }
        }
    } else {
        VirtioFsRuntime::default()
    };

    // Custom kernel boot: direct-boot with kernel+rootfs, no EFI BIOS.
    // Standard boot: use the host profile's BIOS path (if any).
    let bios_path = if use_custom_kernel {
        None
    } else {
        host_profile.bios_path.map(PathBuf::from)
    };

    let mut qemu_config = QemuConfig::from_host_profile(&host_profile)
        .bios_path_opt(bios_path)
        .memory(memory)
        .cpus(cpus)
        .disk_image(&vm_disk_path)
        .disk_format(disk_format)
        .cloud_init_iso(&iso_path)
        .ssh_port(ssh_port)
        .pid_file(vm_dir.join("qemu.pid"))
        .name(vm_id.clone())
        .display(false)
        .daemonize(!verbose);

    if use_custom_kernel {
        let kernel_path = kernel.expect("Validated custom kernel path");
        let kernel_append = match host_profile.arch {
            "x86_64" => "root=/dev/vda rw rootfstype=ext4 console=ttyS0",
            "aarch64" => "root=/dev/vda rw rootfstype=ext4 console=ttyAMA0",
            _ => bail!(
                "Unsupported host architecture for kernel boot: {}",
                host_profile.arch
            ),
        };
        qemu_config = qemu_config.kernel_path(kernel_path).append(kernel_append);
    }

    for share in &virtiofs_runtime.shares {
        qemu_config = qemu_config.virtiofs_share(&share.tag, &share.socket_path);
    }

    if mount_transport == MountTransport::Virtio9p {
        for (index, mount) in mounts.iter().enumerate() {
            let tag = virtiofs_tag_for_index(index);
            qemu_config = qemu_config.virtio_9p_share(&tag, &mount.host_path);
        }
    }

    info!("Starting QEMU VM...");
    let mut runner = QemuRunner::new();
    let pid = match runner.start(&qemu_config) {
        Ok(pid) => pid,
        Err(err) => {
            sp_fail(&mut sp, &err.to_string());
            stop_sidecar_processes(&virtiofs_runtime.pids());
            return Err(err);
        }
    };
    info!("VM started with PID: {}", pid);

    let vm_record = VmRecord {
        id: vm_id,
        hostname,
        username,
        ssh_port,
        private_key_path: private_key_path.to_string_lossy().to_string(),
        disk_path: vm_disk_path.to_string_lossy().to_string(),
        seed_iso_path: iso_path.to_string_lossy().to_string(),
        pid: Some(pid),
        sidecar_pids: virtiofs_runtime.pids(),
        status: VmStatus::Running,
        created_at: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .context("Failed to determine creation time")?
            .as_secs(),
        host_profile: format!(
            "{}/{} ({}, {})",
            host_profile.os, host_profile.arch, host_profile.machine_type, host_profile.cpu_type,
        ),
    };

    tokio::time::sleep(Duration::from_millis(500)).await;
    if vm_record.runtime_status() != VmRuntimeStatus::Running {
        // Clean up on failure
        if let Err(err) = stop_qemu_and_wait(pid, Duration::from_secs(3)) {
            info!("Unable to stop failed VM on pid {}: {}", pid, err);
        }
        stop_sidecar_processes(&vm_record.sidecar_pids);
        if let Err(err) = std::fs::remove_dir_all(&vm_dir) {
            info!(
                "Failed to remove failed VM directory {}: {}",
                vm_dir.display(),
                err
            );
        }

        sp_fail(&mut sp, "process exited");
        bail!(
            "QEMU failed to stay running after startup. \
             Use --verbose for startup output."
        );
    }

    let qemu_detail = format!("PID {pid}, port {ssh_port}");
    sp_complete(&mut sp, &qemu_detail);
    notify_progress(
        &on_progress,
        7,
        8,
        &format!("Starting QEMU — {qemu_detail}"),
    );

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
        if let Some(pid) = vm_record.pid
            && let Err(e) = stop_qemu_and_wait(pid, Duration::from_secs(5))
        {
            info!("Failed to stop QEMU pid {}: {}", pid, e);
        }
        stop_sidecar_processes(&vm_record.sidecar_pids);
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

    let key_str = key_path.to_str().context("Invalid key path")?;
    let port_str = record.ssh_port.to_string();
    let user_host = format!("{}@127.0.0.1", record.username);
    let mut args: Vec<&str> = vec!["-i", key_str, "-p", &port_str];
    args.extend_from_slice(&SSH_STRICT_OPTIONS);
    args.extend_from_slice(&["-o", "ConnectTimeout=10", &user_host]);

    let status = Command::new("ssh")
        .args(&args)
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
    if let Some((id, path)) = spec.split_once(':')
        && !id.is_empty()
    {
        let instances_dir = config.instances_dir();
        if read_vm_record(&instances_dir, id).is_ok() {
            return Ok(CpEndpoint::Remote {
                id: id.to_string(),
                path: path.to_string(),
            });
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
            bail!(
                "cp requires one local path and one VM path (got remote paths for '{src_id}' and '{dest_id}')"
            )
        }
        (CpEndpoint::Local(_), CpEndpoint::Local(_)) => {
            bail!("cp requires one VM path in the form <vm-id>:<path>")
        }
    }
}
