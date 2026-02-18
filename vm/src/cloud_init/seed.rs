use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fmt;

/// Cloud-init metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Metadata {
    /// Instance identifier
    pub instance_id: String,
    /// Local hostname
    pub local_hostname: String,
}

impl Metadata {
    /// Create new metadata with UUID
    #[cfg(test)]
    pub fn new(instance_id: String, local_hostname: String) -> Self {
        Self {
            instance_id,
            local_hostname,
        }
    }

    /// Create metadata from hostname
    pub fn with_hostname(local_hostname: String) -> Self {
        Self {
            instance_id: format!("i-{local_hostname}"),
            local_hostname,
        }
    }
}

impl fmt::Display for Metadata {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "instance-id: {}\nlocal-hostname: {}",
            self.instance_id, self.local_hostname
        )
    }
}

/// Cloud-init user-data in cloud-config format
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserData {
    /// Hostname for the instance
    pub hostname: String,
    /// User configuration
    pub users: Vec<User>,
    /// Commands to run on first boot
    pub runcmd: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    pub name: String,
    pub sudo: String,
    pub shell: String,
    pub ssh_authorized_keys: Vec<String>,
}

impl UserData {
    /// Create new user-data with the specified configuration
    pub fn new(hostname: String, username: String, ssh_public_key: String) -> Self {
        Self {
            hostname,
            users: vec![User {
                name: username,
                sudo: "ALL=(ALL) NOPASSWD:ALL".to_string(),
                shell: "/bin/bash".to_string(),
                ssh_authorized_keys: vec![ssh_public_key],
            }],
            runcmd: vec![
                "systemctl enable ssh".to_string(),
                "systemctl start ssh".to_string(),
            ],
        }
    }

    /// Convert to cloud-config YAML format
    pub fn to_cloud_config(&self) -> Result<String> {
        let mut yaml = String::from("#cloud-config\n");
        yaml.push_str(&format!("hostname: {}\n", self.hostname));

        // Network configuration (DHCP on eth0)
        yaml.push_str("network:\n");
        yaml.push_str("  version: 2\n");
        yaml.push_str("  ethernets:\n");
        yaml.push_str("    id0:\n");
        yaml.push_str("      match:\n");
        yaml.push_str("        driver: virtio_net\n");
        yaml.push_str("      dhcp4: true\n");

        yaml.push_str("users:\n");
        for user in &self.users {
            yaml.push_str(&format!("  - name: {}\n", user.name));
            yaml.push_str(&format!("    sudo: {}\n", user.sudo));
            yaml.push_str(&format!("    shell: {}\n", user.shell));
            yaml.push_str("    ssh_authorized_keys:\n");
            for key in &user.ssh_authorized_keys {
                yaml.push_str(&format!("      - {}\n", key));
            }
        }

        if !self.runcmd.is_empty() {
            yaml.push_str("runcmd:\n");
            for cmd in &self.runcmd {
                yaml.push_str(&format!("  - {}\n", cmd));
            }
        }

        Ok(yaml)
    }
}

/// Cloud-init seed generator
pub struct CloudInitSeed {
    metadata: Metadata,
    userdata: UserData,
}

impl CloudInitSeed {
    /// Create seed with minimal configuration
    pub fn with_config(hostname: String, username: String, ssh_public_key: String) -> Self {
        Self {
            metadata: Metadata::with_hostname(hostname.clone()),
            userdata: UserData::new(hostname, username, ssh_public_key),
        }
    }

    /// Get metadata content
    #[cfg(test)]
    pub fn metadata(&self) -> &Metadata {
        &self.metadata
    }

    /// Get userdata content
    #[cfg(test)]
    pub fn userdata(&self) -> &UserData {
        &self.userdata
    }

    /// Get metadata as string
    pub fn metadata_string(&self) -> String {
        self.metadata.to_string()
    }

    /// Get userdata as cloud-config YAML
    pub fn userdata_string(&self) -> Result<String> {
        self.userdata.to_cloud_config()
    }

    /// Write metadata to file
    pub fn write_metadata<P: AsRef<std::path::Path>>(&self, path: P) -> Result<()> {
        std::fs::write(path.as_ref(), self.metadata_string())
            .context("Failed to write metadata file")?;
        Ok(())
    }

    /// Write userdata to file
    pub fn write_userdata<P: AsRef<std::path::Path>>(&self, path: P) -> Result<()> {
        let content = self.userdata_string()?;
        std::fs::write(path.as_ref(), content).context("Failed to write userdata file")?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_metadata_display() {
        let metadata = Metadata::new("test-instance".to_string(), "test-host".to_string());
        let display = format!("{}", metadata);
        assert!(display.contains("instance-id: test-instance"));
        assert!(display.contains("local-hostname: test-host"));
    }

    #[test]
    fn test_metadata_with_hostname() {
        let metadata = Metadata::with_hostname("test-host".to_string());
        assert_eq!(metadata.local_hostname, "test-host");
        assert_eq!(metadata.instance_id, "i-test-host");
    }

    #[test]
    fn test_userdata_new() {
        let userdata = UserData::new(
            "test-vm".to_string(),
            "testuser".to_string(),
            "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAA test".to_string(),
        );
        assert_eq!(userdata.hostname, "test-vm");
        assert_eq!(userdata.users.len(), 1);
        assert_eq!(userdata.users[0].name, "testuser");
        assert_eq!(userdata.users[0].sudo, "ALL=(ALL) NOPASSWD:ALL");
        assert_eq!(userdata.users[0].shell, "/bin/bash");
        assert_eq!(userdata.users[0].ssh_authorized_keys.len(), 1);
    }

    #[test]
    fn test_userdata_to_cloud_config() {
        let userdata = UserData::new(
            "test-vm".to_string(),
            "testuser".to_string(),
            "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAA test".to_string(),
        );
        let yaml = userdata.to_cloud_config().unwrap();
        assert!(yaml.contains("#cloud-config"));
        assert!(yaml.contains("hostname: test-vm"));
        assert!(yaml.contains("name: testuser"));
        assert!(yaml.contains("ssh-ed25519"));
        assert!(yaml.contains("network:"));
        assert!(yaml.contains("dhcp4: true"));
        assert!(yaml.contains("sudo:"));
        assert!(yaml.contains("runcmd:"));
    }

    #[test]
    fn test_cloud_init_seed_with_config() {
        let seed = CloudInitSeed::with_config(
            "test-vm".to_string(),
            "testuser".to_string(),
            "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAA test".to_string(),
        );
        let metadata = seed.metadata();
        assert_eq!(metadata.local_hostname, "test-vm");
        let userdata = seed.userdata();
        assert_eq!(userdata.hostname, "test-vm");
    }

    #[test]
    fn test_cloud_init_seed_write_files() {
        let temp_dir = TempDir::new().unwrap();
        let seed = CloudInitSeed::with_config(
            "test-vm".to_string(),
            "testuser".to_string(),
            "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAA test".to_string(),
        );

        let metadata_path = temp_dir.path().join("meta-data");
        let userdata_path = temp_dir.path().join("user-data");

        seed.write_metadata(&metadata_path).unwrap();
        seed.write_userdata(&userdata_path).unwrap();

        assert!(metadata_path.exists());
        assert!(userdata_path.exists());

        let metadata_content = std::fs::read_to_string(&metadata_path).unwrap();
        assert!(metadata_content.contains("local-hostname: test-vm"));

        let userdata_content = std::fs::read_to_string(&userdata_path).unwrap();
        assert!(userdata_content.contains("#cloud-config"));
    }
}
