use anyhow::{bail, Context, Result};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use tracing::debug;

/// SSH key pair manager
pub struct SshKeyManager {
    base_dir: PathBuf,
}

impl SshKeyManager {
    /// Create a new SSH key manager
    pub fn new<P: AsRef<Path>>(base_dir: P) -> Self {
        Self {
            base_dir: base_dir.as_ref().to_path_buf(),
        }
    }

    /// Generate a new SSH key pair
    ///
    /// Returns the path to the private key file
    pub fn generate_key_pair(&self, name: &str) -> Result<(PathBuf, String)> {
        std::fs::create_dir_all(&self.base_dir).context("Failed to create keys directory")?;

        let key_name = format!("{}-{:x}", sanitize_key_name(name), stable_name_hash(name),);
        let private_key_path = self.base_dir.join(format!("{}.key", key_name));

        // Skip generation if key already exists
        if private_key_path.exists() {
            debug!("SSH key already exists at: {}", private_key_path.display());
            let public_key = self.read_public_key(&private_key_path)?;
            return Ok((private_key_path, public_key));
        }

        debug!("Generating SSH key pair: {}", private_key_path.display());

        let key_path_str = private_key_path.to_str().context("Non-UTF8 SSH key path")?;

        let output = std::process::Command::new("ssh-keygen")
            .args([
                "-t",
                "ed25519",
                "-f",
                key_path_str,
                "-N",
                "", // No passphrase
                "-C",
                &format!("vm@{}", name),
            ])
            .output()
            .context("Failed to run ssh-keygen")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("ssh-keygen failed: {}", stderr);
        }

        let public_key = self.read_public_key(&private_key_path)?;

        debug!("SSH key pair generated successfully");
        Ok((private_key_path, public_key))
    }

    /// Read the public key for a private key file
    fn read_public_key(&self, private_key_path: &Path) -> Result<String> {
        let public_key_path = PathBuf::from(format!("{}.pub", private_key_path.display()));

        let public_key =
            std::fs::read_to_string(&public_key_path).context("Failed to read public key file")?;

        Ok(public_key.trim().to_string())
    }
}

fn sanitize_key_name(name: &str) -> String {
    let mut sanitized = String::with_capacity(name.len());
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '.' {
            sanitized.push(ch);
        } else {
            sanitized.push('_');
        }
    }

    let sanitized = sanitized
        .trim_matches(|c: char| c == '.' || c == '_' || c == '-')
        .to_string();
    if sanitized.is_empty() {
        "vm-host".to_string()
    } else {
        sanitized
    }
}

fn stable_name_hash(name: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    name.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_generate_key_pair() {
        let temp_dir = TempDir::new().unwrap();
        let manager = SshKeyManager::new(temp_dir.path());

        let (private_key, public_key) = manager.generate_key_pair("test").unwrap();

        assert!(private_key.exists());
        assert!(!public_key.is_empty());
        assert!(public_key.starts_with("ssh-ed25519"));
    }
}
