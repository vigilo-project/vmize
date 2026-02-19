use anyhow::{Result, bail};

pub const UBUNTU_24_04_MINIMAL_AMD64_URL: &str = "https://cloud-images.ubuntu.com/minimal/releases/noble/release/ubuntu-24.04-minimal-cloudimg-amd64.img";
pub const UBUNTU_24_04_MINIMAL_ARM64_URL: &str = "https://cloud-images.ubuntu.com/minimal/releases/noble/release/ubuntu-24.04-minimal-cloudimg-arm64.img";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostProfile {
    pub os: &'static str,
    pub arch: &'static str,
    pub qemu_binary: &'static str,
    pub machine_type: &'static str,
    pub cpu_type: &'static str,
    pub image_url: &'static str,
    pub bios_path: Option<&'static str>,
}

impl HostProfile {
    pub fn detect() -> Result<Self> {
        let profile = Self::for_target(std::env::consts::OS, std::env::consts::ARCH)?;
        let accel_override = std::env::var("VM_QEMU_ACCEL").ok();
        Ok(Self::apply_accel_override(
            profile,
            accel_override.as_deref(),
        ))
    }

    pub fn for_target(os: &str, arch: &str) -> Result<Self> {
        match (os, arch) {
            ("linux", "x86_64") => Ok(Self {
                os: "linux",
                arch: "x86_64",
                qemu_binary: "qemu-system-x86_64",
                machine_type: "q35,accel=kvm",
                cpu_type: "host",
                image_url: UBUNTU_24_04_MINIMAL_AMD64_URL,
                bios_path: None,
            }),
            ("macos", "aarch64") => Ok(Self {
                os: "macos",
                arch: "aarch64",
                qemu_binary: "qemu-system-aarch64",
                machine_type: "virt,accel=hvf",
                cpu_type: "host",
                image_url: UBUNTU_24_04_MINIMAL_ARM64_URL,
                bios_path: Some("/opt/homebrew/share/qemu/edk2-aarch64-code.fd"),
            }),
            _ => bail!(
                "Unsupported host platform: {}/{}. Supported platforms: linux/x86_64, macos/aarch64",
                os,
                arch
            ),
        }
    }

    fn apply_accel_override(mut profile: Self, accel_override: Option<&str>) -> Self {
        if accel_override == Some("tcg") {
            match profile.os {
                "macos" => {
                    profile.machine_type = "virt,accel=tcg";
                    profile.cpu_type = "max";
                }
                "linux" => {
                    profile.machine_type = "q35,accel=tcg";
                    profile.cpu_type = "max";
                }
                _ => {}
            }
        }

        profile
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_host_profile_linux_x86_64() {
        let profile = HostProfile::for_target("linux", "x86_64").unwrap();
        assert_eq!(profile.qemu_binary, "qemu-system-x86_64");
        assert_eq!(profile.machine_type, "q35,accel=kvm");
        assert_eq!(profile.cpu_type, "host");
        assert!(profile.image_url.contains("amd64"));
        assert_eq!(profile.bios_path, None);
    }

    #[test]
    fn test_host_profile_macos_aarch64() {
        let profile = HostProfile::for_target("macos", "aarch64").unwrap();
        assert_eq!(profile.qemu_binary, "qemu-system-aarch64");
        assert_eq!(profile.machine_type, "virt,accel=hvf");
        assert_eq!(profile.cpu_type, "host");
        assert!(profile.image_url.contains("arm64"));
        assert_eq!(
            profile.bios_path,
            Some("/opt/homebrew/share/qemu/edk2-aarch64-code.fd")
        );
    }

    #[test]
    fn test_host_profile_unsupported() {
        let err = HostProfile::for_target("linux", "aarch64").unwrap_err();
        assert!(err.to_string().contains("Unsupported host platform"));
    }

    #[test]
    fn test_macos_profile_without_override_uses_hvf() {
        let profile = HostProfile::for_target("macos", "aarch64").unwrap();
        let profile = HostProfile::apply_accel_override(profile, None);
        assert_eq!(profile.machine_type, "virt,accel=hvf");
        assert_eq!(profile.cpu_type, "host");
    }

    #[test]
    fn test_macos_profile_with_tcg_override() {
        let profile = HostProfile::for_target("macos", "aarch64").unwrap();
        let profile = HostProfile::apply_accel_override(profile, Some("tcg"));
        assert_eq!(profile.machine_type, "virt,accel=tcg");
        assert_eq!(profile.cpu_type, "max");
    }

    #[test]
    fn test_linux_profile_with_tcg_override() {
        let profile = HostProfile::for_target("linux", "x86_64").unwrap();
        let profile = HostProfile::apply_accel_override(profile, Some("tcg"));
        assert_eq!(profile.machine_type, "q35,accel=tcg");
        assert_eq!(profile.cpu_type, "max");
    }
}
