use anyhow::{Context, Result, bail};
use std::collections::hash_map::DefaultHasher;
use std::fs::OpenOptions;
use std::hash::{Hash, Hasher};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;
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

        // Acquire lock to prevent race condition when multiple threads try to generate
        // the same key simultaneously
        let lock_path = self.base_dir.join(format!("{}.lock", key_name));
        let _lock = acquire_file_lock(&lock_path)?;

        // Double-check after acquiring lock - another thread may have created it
        if private_key_path.exists() {
            debug!(
                "SSH key created by another thread at: {}",
                private_key_path.display()
            );
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

/// Acquire a file-based lock for synchronization.
/// Returns a guard that releases the lock when dropped.
fn acquire_file_lock(lock_path: &Path) -> Result<LockGuard> {
    // Ensure parent directory exists
    if let Some(parent) = lock_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create lock directory {}", parent.display()))?;
    }

    let max_attempts = 100; // 5 seconds max
    for _ in 0..max_attempts {
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(lock_path)
        {
            Ok(mut file) => {
                // Write our PID for debugging
                let _ = file.write_all(std::process::id().to_string().as_bytes());
                let _ = file.flush();
                return Ok(LockGuard {
                    path: lock_path.to_path_buf(),
                });
            }
            Err(e) if e.kind() == ErrorKind::AlreadyExists => {
                // Another process holds the lock, wait and retry
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => {
                return Err(e).context(format!(
                    "Failed to acquire lock file {}",
                    lock_path.display()
                ));
            }
        }
    }

    bail!("Timeout waiting for lock file {}", lock_path.display())
}

/// Guard that releases the file lock when dropped
struct LockGuard {
    path: PathBuf,
}

impl Drop for LockGuard {
    fn drop(&mut self) {
        if let Err(err) = std::fs::remove_file(&self.path)
            && err.kind() != ErrorKind::NotFound
        {
            eprintln!(
                "Failed to remove lock file {}: {}",
                self.path.display(),
                err
            );
        }
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
