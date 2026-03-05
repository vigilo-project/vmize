use anyhow::{Result, bail};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MountMode {
    ReadOnly,
    ReadWrite,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MountSpec {
    pub host_path: PathBuf,
    pub guest_path: PathBuf,
    pub mode: MountMode,
}

pub fn parse_mount_spec(raw: &str) -> Result<MountSpec> {
    let raw = raw.trim();
    if raw.is_empty() {
        bail!("mount spec must not be empty");
    }

    let mut parts = raw.split(':');
    let host_path_str = parts.next().unwrap_or_default();
    let guest_path_str = parts.next().unwrap_or_default();
    let mode_str = parts.next();
    if parts.next().is_some() || host_path_str.is_empty() || guest_path_str.is_empty() {
        bail!("mount spec must be in form <host_path>:<guest_path>[:ro|rw]");
    }

    let mode = match mode_str.unwrap_or("ro") {
        "ro" => MountMode::ReadOnly,
        "rw" => MountMode::ReadWrite,
        _ => bail!("mount mode must be 'ro' or 'rw'"),
    };

    let host_path = PathBuf::from(host_path_str);
    if !host_path.is_absolute() {
        bail!("host path must be absolute");
    }
    if !host_path.exists() {
        bail!("host path does not exist: {}", host_path.display());
    }

    let guest_path = PathBuf::from(guest_path_str);
    if !guest_path.is_absolute() {
        bail!("guest path must be absolute");
    }

    Ok(MountSpec {
        host_path,
        guest_path,
        mode,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_mount_spec_defaults_to_read_only_mode() {
        let spec = parse_mount_spec("/tmp:/mnt/host").expect("mount spec should parse");
        assert_eq!(spec.host_path, PathBuf::from("/tmp"));
        assert_eq!(spec.guest_path, PathBuf::from("/mnt/host"));
        assert_eq!(spec.mode, MountMode::ReadOnly);
    }

    #[test]
    fn parse_mount_spec_accepts_read_write_mode() {
        let spec = parse_mount_spec("/tmp:/mnt/host:rw").expect("mount spec should parse");
        assert_eq!(spec.mode, MountMode::ReadWrite);
    }

    #[test]
    fn parse_mount_spec_rejects_non_absolute_host_path() {
        let err = parse_mount_spec("relative:/mnt/host").expect_err("spec must fail");
        assert!(err.to_string().contains("host path must be absolute"));
    }

    #[test]
    fn parse_mount_spec_rejects_non_absolute_guest_path() {
        let err = parse_mount_spec("/tmp:relative").expect_err("spec must fail");
        assert!(err.to_string().contains("guest path must be absolute"));
    }

    #[test]
    fn parse_mount_spec_rejects_unknown_mode() {
        let err = parse_mount_spec("/tmp:/mnt/host:bad").expect_err("spec must fail");
        assert!(err.to_string().contains("mount mode must be 'ro' or 'rw'"));
    }

    #[test]
    fn parse_mount_spec_rejects_nonexistent_host_path() {
        let err = parse_mount_spec("/definitely/missing/path:/mnt/host").expect_err("spec must fail");
        assert!(err.to_string().contains("host path does not exist"));
    }
}
