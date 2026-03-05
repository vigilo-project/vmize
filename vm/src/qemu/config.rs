use crate::platform::HostProfile;
use anyhow::{Result, bail};
use std::path::{Path, PathBuf};
use std::process::Command;

/// QEMU configuration builder
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum DiskFormat {
    #[default]
    Qcow2,
    Raw,
}

impl DiskFormat {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Qcow2 => "qcow2",
            Self::Raw => "raw",
        }
    }
}

pub struct QemuConfig {
    qemu_binary: String,
    bios_path: Option<PathBuf>,
    machine_type: String,
    cpu_type: String,
    memory: String,
    cpus: u32,
    disk_image: Option<PathBuf>,
    kernel_path: Option<PathBuf>,
    append: Option<String>,
    disk_format: DiskFormat,
    cloud_init_iso: Option<PathBuf>,
    pid_file: Option<PathBuf>,
    qemu_name: Option<String>,
    ssh_port: Option<u16>,
    virtiofs_shares: Vec<VirtioFsShare>,
    display: bool,
    daemonize: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct VirtioFsShare {
    tag: String,
    socket_path: PathBuf,
}

impl Default for QemuConfig {
    fn default() -> Self {
        let (qemu_binary, machine_type, cpu_type) = match HostProfile::detect() {
            Ok(profile) => (
                profile.qemu_binary.to_string(),
                profile.machine_type.to_string(),
                profile.cpu_type.to_string(),
            ),
            Err(_) => (
                "qemu-system-x86_64".to_string(),
                "q35,accel=kvm".to_string(),
                "host".to_string(),
            ),
        };

        Self {
            qemu_binary,
            bios_path: None,
            machine_type,
            cpu_type,
            memory: "4G".to_string(),
            cpus: 2,
            disk_image: None,
            kernel_path: None,
            append: None,
            disk_format: DiskFormat::Qcow2,
            cloud_init_iso: None,
            pid_file: None,
            qemu_name: None,
            ssh_port: None,
            virtiofs_shares: Vec::new(),
            display: false,
            daemonize: true,
        }
    }
}

impl QemuConfig {
    /// Create a new QEMU config with defaults
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a QEMU config initialized from a host profile
    pub fn from_host_profile(profile: &HostProfile) -> Self {
        Self::new()
            .qemu_binary(profile.qemu_binary)
            .bios_path_opt(profile.bios_path.map(PathBuf::from))
            .machine_type(profile.machine_type)
            .cpu_type(profile.cpu_type)
    }

    /// Set the qemu binary name
    pub fn qemu_binary(mut self, qemu_binary: impl Into<String>) -> Self {
        self.qemu_binary = qemu_binary.into();
        self
    }

    /// Set optional UEFI BIOS path
    pub fn bios_path_opt(mut self, bios_path: Option<PathBuf>) -> Self {
        self.bios_path = bios_path;
        self
    }

    /// Set the machine type
    pub fn machine_type(mut self, machine: impl Into<String>) -> Self {
        self.machine_type = machine.into();
        self
    }

    /// Set the CPU type
    pub fn cpu_type(mut self, cpu: impl Into<String>) -> Self {
        self.cpu_type = cpu.into();
        self
    }

    /// Set memory allocation
    pub fn memory(mut self, memory: impl Into<String>) -> Self {
        self.memory = memory.into();
        self
    }

    /// Set number of CPUs
    pub fn cpus(mut self, cpus: u32) -> Self {
        self.cpus = cpus;
        self
    }

    /// Set the output disk format
    pub fn disk_format(mut self, disk_format: DiskFormat) -> Self {
        self.disk_format = disk_format;
        self
    }

    /// Set the disk image path
    pub fn disk_image<P: AsRef<Path>>(mut self, path: P) -> Self {
        self.disk_image = Some(path.as_ref().to_path_buf());
        self
    }

    /// Set the kernel image path
    pub fn kernel_path<P: AsRef<Path>>(mut self, path: P) -> Self {
        self.kernel_path = Some(path.as_ref().to_path_buf());
        self
    }

    /// Set additional kernel command line arguments
    pub fn append(mut self, append: impl Into<String>) -> Self {
        self.append = Some(append.into());
        self
    }

    /// Set the cloud-init ISO path
    pub fn cloud_init_iso<P: AsRef<Path>>(mut self, path: P) -> Self {
        self.cloud_init_iso = Some(path.as_ref().to_path_buf());
        self
    }

    /// Set the QEMU pidfile path.
    pub fn pid_file<P: AsRef<Path>>(mut self, path: P) -> Self {
        self.pid_file = Some(path.as_ref().to_path_buf());
        self
    }

    /// Set the VM name
    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.qemu_name = Some(name.into());
        self
    }

    /// Set SSH port forwarding
    pub fn ssh_port(mut self, port: u16) -> Self {
        self.ssh_port = Some(port);
        self
    }

    /// Add a virtiofs share backed by a vhost-user socket.
    pub fn virtiofs_share<P: AsRef<Path>>(
        mut self,
        tag: impl Into<String>,
        socket_path: P,
    ) -> Self {
        self.virtiofs_shares.push(VirtioFsShare {
            tag: tag.into(),
            socket_path: socket_path.as_ref().to_path_buf(),
        });
        self
    }

    /// Return the configured pidfile path, if set.
    pub fn pid_file_path(&self) -> Option<&Path> {
        self.pid_file.as_deref()
    }

    /// Enable/disable display
    pub fn display(mut self, enable: bool) -> Self {
        self.display = enable;
        self
    }

    /// Enable/disable daemonization
    pub fn daemonize(mut self, enable: bool) -> Self {
        self.daemonize = enable;
        self
    }

    /// Build the QEMU command
    pub fn build_command(&self) -> Result<Command> {
        let mut cmd = Command::new(&self.qemu_binary);

        if let Some(bios_path) = &self.bios_path {
            cmd.arg("-bios").arg(bios_path);
        }

        cmd.arg("-machine").arg(&self.machine_type);
        cmd.arg("-cpu").arg(&self.cpu_type);
        cmd.arg("-m").arg(&self.memory);
        cmd.arg("-smp").arg(self.cpus.to_string());

        // Add disk image
        if let Some(disk) = &self.disk_image {
            cmd.arg("-drive").arg(format!(
                "file={},format={},if=virtio",
                disk.display(),
                self.disk_format.as_str()
            ));
        }

        if let Some(kernel_path) = &self.kernel_path {
            cmd.arg("-kernel").arg(kernel_path);
        }

        if let Some(append) = &self.append {
            cmd.arg("-append").arg(append);
        }

        // Add cloud-init ISO
        if let Some(iso) = &self.cloud_init_iso {
            cmd.arg("-drive").arg(format!(
                "file={},format=raw,if=virtio,media=cdrom",
                iso.display()
            ));
        }

        if let Some(pid_file) = &self.pid_file {
            cmd.arg("-pidfile").arg(pid_file);
        }

        if let Some(name) = &self.qemu_name {
            cmd.arg("-name").arg(name);
        }

        // Configure networking with SSH port forwarding
        if let Some(port) = self.ssh_port {
            cmd.arg("-netdev")
                .arg(format!("user,id=net0,hostfwd=tcp::{}-:22", port));
            cmd.arg("-device").arg("virtio-net-pci,netdev=net0");
        }

        for (index, share) in self.virtiofs_shares.iter().enumerate() {
            let chardev_id = format!("virtiofsch{index}");
            cmd.arg("-chardev").arg(format!(
                "socket,id={},path={}",
                chardev_id,
                share.socket_path.display()
            ));
            cmd.arg("-device").arg(format!(
                "vhost-user-fs-pci,chardev={},tag={}",
                chardev_id, share.tag
            ));
        }

        // Configure display
        if self.display {
            cmd.arg("-display").arg("gtk");
        } else {
            cmd.arg("-display").arg("none");
        }

        // Daemonize if enabled
        if self.daemonize {
            // Keep the default serial device but discard its output.
            // `none` removes serial devices entirely, which can break
            // guests that rely on `console=ttyS0` / `console=ttyAMA0`.
            cmd.arg("-serial").arg("null");
            cmd.arg("-daemonize");
        } else {
            // Use stdio for serial output when not daemonizing
            cmd.arg("-serial").arg("stdio");
        }

        Ok(cmd)
    }

    /// Build the QEMU command as a vector of strings
    #[cfg(test)]
    pub fn build_args(&self) -> Result<Vec<String>> {
        let cmd = self.build_command()?;
        let args: Vec<String> = std::iter::once(self.qemu_binary.clone())
            .chain(cmd.get_args().map(|s| s.to_string_lossy().to_string()))
            .collect();

        Ok(args)
    }

    /// Validate the configuration
    pub fn validate(&self) -> Result<()> {
        if self.disk_image.is_none() {
            bail!("Disk image is required");
        }

        if self.cloud_init_iso.is_none() {
            bail!("Cloud-init ISO is required");
        }

        if let Some(disk) = &self.disk_image
            && !disk.exists()
        {
            bail!("Disk image does not exist: {}", disk.display());
        }

        if let Some(iso) = &self.cloud_init_iso
            && !iso.exists()
        {
            bail!("Cloud-init ISO does not exist: {}", iso.display());
        }

        if let Some(kernel) = &self.kernel_path
            && !kernel.exists()
        {
            bail!("Kernel image does not exist: {}", kernel.display());
        }

        for share in &self.virtiofs_shares {
            if share.tag.trim().is_empty() {
                bail!("virtiofs tag must not be empty");
            }
            if !share.socket_path.is_absolute() {
                bail!(
                    "virtiofs socket path must be absolute: {}",
                    share.socket_path.display()
                );
            }
            if !share.socket_path.exists() {
                bail!(
                    "virtiofs socket does not exist: {}",
                    share.socket_path.display()
                );
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::HostProfile;
    use tempfile::NamedTempFile;
    use tempfile::TempDir;

    #[test]
    fn test_qemu_config_default() {
        let config = QemuConfig::default();
        assert!(config.qemu_binary.starts_with("qemu-system-"));
        assert!(!config.machine_type.is_empty());

        let expected_cpu = HostProfile::detect().map(|p| p.cpu_type).unwrap_or("host");
        assert_eq!(config.cpu_type, expected_cpu);

        assert_eq!(config.memory, "4G");
        assert_eq!(config.cpus, 2);
        assert!(config.disk_image.is_none());
        assert!(config.cloud_init_iso.is_none());
        assert!(config.ssh_port.is_none());
        assert!(!config.display);
        assert!(config.daemonize);
        assert!(matches!(config.disk_format, DiskFormat::Qcow2));
        assert!(config.kernel_path.is_none());
        assert!(config.append.is_none());
    }

    #[test]
    fn test_qemu_config_builder() {
        let temp_file = NamedTempFile::new().unwrap();
        let temp_file2 = NamedTempFile::new().unwrap();
        let temp_file3 = NamedTempFile::new().unwrap();

        let config = QemuConfig::new()
            .qemu_binary("qemu-system-x86_64".to_string())
            .machine_type("q35,accel=kvm".to_string())
            .memory("4G".to_string())
            .cpus(4)
            .disk_image(temp_file.path())
            .disk_format(DiskFormat::Raw)
            .kernel_path(temp_file3.path())
            .append("console=ttyS0")
            .cloud_init_iso(temp_file2.path())
            .ssh_port(2222)
            .pid_file(temp_file.path())
            .display(true)
            .daemonize(false);

        assert_eq!(config.memory, "4G");
        assert_eq!(config.cpus, 4);
        assert_eq!(config.ssh_port, Some(2222));
        assert!(config.display);
        assert!(!config.daemonize);
        assert_eq!(config.qemu_binary, "qemu-system-x86_64");
        assert!(config.bios_path.is_none());
        assert_eq!(config.pid_file, Some(temp_file.path().to_path_buf()));
        assert_eq!(config.disk_format, DiskFormat::Raw);
        assert_eq!(config.append.as_deref(), Some("console=ttyS0"));
    }

    #[test]
    fn test_qemu_config_build_args() {
        let temp_file = NamedTempFile::new().unwrap();
        let temp_file2 = NamedTempFile::new().unwrap();

        let config = QemuConfig::new()
            .qemu_binary("qemu-system-x86_64".to_string())
            .machine_type("q35,accel=kvm".to_string())
            .disk_image(temp_file.path())
            .disk_format(DiskFormat::Qcow2)
            .cloud_init_iso(temp_file2.path())
            .ssh_port(2222)
            .pid_file(temp_file.path());

        let args = config.build_args().unwrap();
        assert!(args.contains(&"qemu-system-x86_64".to_string()));
        assert!(args.contains(&"-machine".to_string()));
        assert!(args.contains(&"q35,accel=kvm".to_string()));
        assert!(args.contains(&"-m".to_string()));
        assert!(args.contains(&"4G".to_string()));
        assert!(args.contains(&"-netdev".to_string()));
        assert!(args.iter().any(|arg| arg.contains("hostfwd=tcp::2222-:22")));
        assert!(args.iter().any(|arg| arg.contains("-pidfile")));
        assert!(
            args.iter()
                .any(|arg| arg.contains(temp_file.path().to_string_lossy().as_ref()))
        );
        assert!(args.iter().any(|arg| arg.contains("format=qcow2")));
    }

    #[test]
    fn test_qemu_config_build_args_with_kernel() {
        let temp_file = NamedTempFile::new().unwrap();
        let temp_file2 = NamedTempFile::new().unwrap();
        let temp_file3 = NamedTempFile::new().unwrap();

        let config = QemuConfig::new()
            .qemu_binary("qemu-system-x86_64".to_string())
            .machine_type("q35,accel=kvm".to_string())
            .disk_image(temp_file.path())
            .disk_format(DiskFormat::Raw)
            .kernel_path(temp_file3.path())
            .append("console=ttyS0")
            .cloud_init_iso(temp_file2.path())
            .ssh_port(2222)
            .pid_file(temp_file.path());

        let args = config.build_args().unwrap();
        assert!(args.iter().any(|arg| arg.contains("-kernel")));
        assert!(args.iter().any(|arg| arg.contains("console=ttyS0")));
        assert!(args.iter().any(|arg| arg.contains("format=raw")));
    }

    #[test]
    fn test_qemu_config_from_linux_profile() {
        let profile = HostProfile::for_target("linux", "x86_64").unwrap();
        let temp_file = NamedTempFile::new().unwrap();
        let temp_file2 = NamedTempFile::new().unwrap();
        let config = QemuConfig::from_host_profile(&profile)
            .disk_image(temp_file.path())
            .cloud_init_iso(temp_file2.path());

        let args = config.build_args().unwrap();
        assert!(args.contains(&"qemu-system-x86_64".to_string()));
        assert!(args.contains(&"q35,accel=kvm".to_string()));
        assert!(!args.contains(&"-bios".to_string()));
    }

    #[test]
    fn test_qemu_config_from_macos_arm64_profile() {
        let profile = HostProfile::for_target("macos", "aarch64").unwrap();
        let temp_file = NamedTempFile::new().unwrap();
        let temp_file2 = NamedTempFile::new().unwrap();
        let config = QemuConfig::from_host_profile(&profile)
            .disk_image(temp_file.path())
            .cloud_init_iso(temp_file2.path());

        let args = config.build_args().unwrap();
        assert!(args.contains(&"qemu-system-aarch64".to_string()));
        assert!(args.contains(&"virt,accel=hvf".to_string()));
        assert!(args.contains(&"-bios".to_string()));
        assert!(args.iter().any(|arg| arg.contains("edk2-aarch64-code.fd")));
    }

    #[test]
    fn test_qemu_config_validate_missing_disk() {
        let config = QemuConfig::new();
        let result = config.validate();
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Disk image is required")
        );
    }

    #[test]
    fn test_qemu_config_validate_missing_iso() {
        let temp_file = NamedTempFile::new().unwrap();
        let config = QemuConfig::new().disk_image(temp_file.path());
        let result = config.validate();
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Cloud-init ISO is required")
        );
    }

    #[test]
    fn test_qemu_config_validate_nonexistent_disk() {
        let temp_file2 = NamedTempFile::new().unwrap();
        let fake_path = PathBuf::from("/nonexistent/disk.qcow2");
        let config = QemuConfig::new()
            .disk_image(&fake_path)
            .cloud_init_iso(temp_file2.path());
        let result = config.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("does not exist"));
    }

    #[test]
    fn test_qemu_config_validate_nonexistent_kernel() {
        let temp_file = NamedTempFile::new().unwrap();
        let temp_kernel = PathBuf::from("/nonexistent/bzImage");
        let config = QemuConfig::new()
            .disk_image(temp_file.path())
            .cloud_init_iso(temp_file.path())
            .kernel_path(temp_kernel)
            .append("console=ttyS0");
        let result = config.validate();
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Kernel image does not exist")
        );
    }

    #[test]
    fn test_qemu_config_validate_valid() {
        let temp_file = NamedTempFile::new().unwrap();
        let temp_file2 = NamedTempFile::new().unwrap();
        let config = QemuConfig::new()
            .disk_image(temp_file.path())
            .cloud_init_iso(temp_file2.path());
        let result = config.validate();
        assert!(result.is_ok());
    }

    #[test]
    fn test_qemu_config_daemonize_serial_null() {
        let temp_file = NamedTempFile::new().unwrap();
        let temp_file2 = NamedTempFile::new().unwrap();

        let config = QemuConfig::new()
            .disk_image(temp_file.path())
            .cloud_init_iso(temp_file2.path())
            .daemonize(true);

        let args = config.build_args().unwrap();
        assert!(args.contains(&"-daemonize".to_string()));
        assert!(args.contains(&"-serial".to_string()));
        assert!(args.contains(&"null".to_string()));
    }

    #[test]
    fn test_qemu_config_no_daemonize_serial_stdio() {
        let temp_file = NamedTempFile::new().unwrap();
        let temp_file2 = NamedTempFile::new().unwrap();

        let config = QemuConfig::new()
            .disk_image(temp_file.path())
            .cloud_init_iso(temp_file2.path())
            .daemonize(false);

        let args = config.build_args().unwrap();
        assert!(!args.contains(&"-daemonize".to_string()));
        assert!(args.contains(&"-serial".to_string()));
        assert!(args.contains(&"stdio".to_string()));
    }

    #[test]
    fn test_qemu_config_build_args_with_virtiofs_share() {
        let temp_file = NamedTempFile::new().unwrap();
        let temp_file2 = NamedTempFile::new().unwrap();
        let temp_dir = TempDir::new().unwrap();
        let socket_path = temp_dir.path().join("virtiofs.sock");

        let config = QemuConfig::new()
            .disk_image(temp_file.path())
            .cloud_init_iso(temp_file2.path())
            .virtiofs_share("vmizefs0", &socket_path);

        let args = config.build_args().unwrap();
        assert!(args.iter().any(|arg| {
            arg == "-chardev"
                || arg.contains(&format!(
                    "socket,id=virtiofsch0,path={}",
                    socket_path.display()
                ))
        }));
        assert!(args.iter().any(|arg| {
            arg == "-device" || arg.contains("vhost-user-fs-pci,chardev=virtiofsch0,tag=vmizefs0")
        }));
    }
}
