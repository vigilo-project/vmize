use crate::platform::{HostProfile, UBUNTU_24_04_MINIMAL_AMD64_URL};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Application configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Base directory for VM storage
    pub base_dir: PathBuf,
    /// Ubuntu Cloud Image URL
    pub image_url: String,
    /// Default VM memory
    pub memory: String,
    /// Default CPU count
    pub cpus: u32,
}

impl Default for Config {
    fn default() -> Self {
        let image_url = HostProfile::detect()
            .map(|profile| profile.image_url.to_string())
            .unwrap_or_else(|_| UBUNTU_24_04_MINIMAL_AMD64_URL.to_string());

        Self {
            base_dir: Self::default_base_dir(),
            image_url,
            memory: "4G".to_string(),
            cpus: 2,
        }
    }
}

impl Config {
    /// Default base directory for VM artifacts.
    fn default_base_dir() -> PathBuf {
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .map(|home| home.join(".local").join("share").join("vm"))
            .unwrap_or_else(|| PathBuf::from(".").join(".local").join("share").join("vm"))
    }

    /// Ensure the base directory exists
    pub fn ensure_base_dir(&self) -> Result<()> {
        std::fs::create_dir_all(&self.base_dir)?;
        Ok(())
    }

    /// Get the path to store downloaded images
    pub fn images_dir(&self) -> PathBuf {
        self.base_dir.join("images")
    }

    /// Get the path to store VM instances
    pub fn instances_dir(&self) -> PathBuf {
        self.base_dir.join("instances")
    }

    /// Get the path to store SSH keys
    pub fn keys_dir(&self) -> PathBuf {
        self.base_dir.join("keys")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_default() {
        let config = Config::default();
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        assert_eq!(
            config.base_dir,
            PathBuf::from(home).join(".local").join("share").join("vm")
        );
        assert_eq!(config.memory, "4G");
        assert_eq!(config.cpus, 2);
        assert!(config.image_url.contains("ubuntu-24.04-minimal-cloudimg"));
    }

    #[test]
    fn test_config_images_dir() {
        let config = Config::default();
        assert_eq!(
            config.images_dir(),
            Config::default().base_dir.join("images")
        );
    }

    #[test]
    fn test_config_instances_dir() {
        let config = Config::default();
        assert_eq!(
            config.instances_dir(),
            Config::default().base_dir.join("instances")
        );
    }

    #[test]
    fn test_config_keys_dir() {
        let config = Config::default();
        assert_eq!(config.keys_dir(), Config::default().base_dir.join("keys"));
    }

    #[test]
    fn test_config_ensure_base_dir() {
        let config = Config::default();
        config.ensure_base_dir().unwrap();
        assert!(config.base_dir.exists());
    }
}
