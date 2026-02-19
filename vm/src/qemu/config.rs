use crate::platform::HostProfile;
use anyhow::{bail, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

/// QEMU configuration builder
pub struct QemuConfig {
    qemu_binary: String,
    bios_path: Option<PathBuf>,
    machine_type: String,
    cpu_type: String,
    memory: String,
    cpus: u32,
    disk_image: Option<PathBuf>,
    cloud_init_iso: Option<PathBuf>,
    pid_file: Option<PathBuf>,
    qemu_name: Option<String>,
    ssh_port: Option<u16>,
    display: bool,
    daemonize: bool,
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
            cloud_init_iso: None,
            pid_file: None,
            qemu_name: None,
            ssh_port: None,
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

    /// Set the disk image path
    pub fn disk_image<P: AsRef<Path>>(mut self, path: P) -> Self {
        self.disk_image = Some(path.as_ref().to_path_buf());
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
            cmd.arg("-drive")
                .arg(format!("file={},format=qcow2,if=virtio", disk.display()));
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

        // Configure display
        if self.display {
            cmd.arg("-display").arg("gtk");
        } else {
            cmd.arg("-display").arg("none");
        }

        // Daemonize if enabled
        if self.daemonize {
            // Redirect serial to /dev/null when daemonizing
            cmd.arg("-serial").arg("none");
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
            && !disk.exists() {
                bail!("Disk image does not exist: {}", disk.display());
            }

        if let Some(iso) = &self.cloud_init_iso
            && !iso.exists() {
                bail!("Cloud-init ISO does not exist: {}", iso.display());
            }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::HostProfile;
    use tempfile::NamedTempFile;

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
    }

    #[test]
    fn test_qemu_config_builder() {
        let temp_file = NamedTempFile::new().unwrap();
        let temp_file2 = NamedTempFile::new().unwrap();

        let config = QemuConfig::new()
            .qemu_binary("qemu-system-x86_64".to_string())
            .machine_type("q35,accel=kvm".to_string())
            .memory("4G".to_string())
            .cpus(4)
            .disk_image(temp_file.path())
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
    }

    #[test]
    fn test_qemu_config_build_args() {
        let temp_file = NamedTempFile::new().unwrap();
        let temp_file2 = NamedTempFile::new().unwrap();

        let config = QemuConfig::new()
            .qemu_binary("qemu-system-x86_64".to_string())
            .machine_type("q35,accel=kvm".to_string())
            .disk_image(temp_file.path())
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
        assert!(args
            .iter()
            .any(|arg| arg.contains(temp_file.path().to_string_lossy().as_ref())));
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
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Disk image is required"));
    }

    #[test]
    fn test_qemu_config_validate_missing_iso() {
        let temp_file = NamedTempFile::new().unwrap();
        let config = QemuConfig::new().disk_image(temp_file.path());
        let result = config.validate();
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Cloud-init ISO is required"));
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
    fn test_qemu_config_daemonize_serial_none() {
        let temp_file = NamedTempFile::new().unwrap();
        let temp_file2 = NamedTempFile::new().unwrap();

        let config = QemuConfig::new()
            .disk_image(temp_file.path())
            .cloud_init_iso(temp_file2.path())
            .daemonize(true);

        let args = config.build_args().unwrap();
        assert!(args.contains(&"-daemonize".to_string()));
        assert!(args.contains(&"-serial".to_string()));
        assert!(args.contains(&"none".to_string()));
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
}
