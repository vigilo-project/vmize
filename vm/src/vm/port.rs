use crate::process::is_process_alive;
use anyhow::{Context, Result, bail};
use std::fs::OpenOptions;
use std::io::{ErrorKind, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

/// Maximum number of VMs allowed
pub const MAX_VMS: usize = 10;

// ---------------------------------------------------------------------------
// VM creation lock (prevents race conditions in concurrent VM creation)
// ---------------------------------------------------------------------------

/// Path to the VM creation lock directory
pub fn vm_locks_dir(instances_dir: &Path) -> PathBuf {
    instances_dir.join(".vm-locks")
}

/// Path to the global VM creation lock
pub fn vm_creation_lock_path(instances_dir: &Path) -> PathBuf {
    vm_locks_dir(instances_dir).join("creation.lock")
}

/// RAII guard for VM creation lock.
/// The lock is released (file deleted) when this guard is dropped.
#[derive(Debug)]
pub struct VmCreationLock {
    lock_path: PathBuf,
}

impl Drop for VmCreationLock {
    fn drop(&mut self) {
        if let Err(err) = std::fs::remove_file(&self.lock_path)
            && err.kind() != ErrorKind::NotFound
        {
            eprintln!(
                "Failed to remove VM creation lock {}: {err}",
                self.lock_path.display()
            );
        }
    }
}

/// Check if a PID-based lock file is stale (the process that created it is no
/// longer alive).  Returns `true` when the lock can safely be reclaimed.
fn is_stale_pid_lock(lock_path: &Path) -> bool {
    let content = match std::fs::read_to_string(lock_path) {
        Ok(c) => c,
        Err(err) => return err.kind() == ErrorKind::NotFound,
    };

    match content.trim().parse::<u32>() {
        Ok(pid) => !is_process_alive(pid),
        Err(_) => true, // Invalid PID, consider stale
    }
}

/// Default timeout for VM creation lock acquisition (10 seconds).
const DEFAULT_LOCK_TIMEOUT: Duration = Duration::from_secs(10);

/// Interval between lock acquisition attempts.
const LOCK_RETRY_INTERVAL: Duration = Duration::from_millis(100);

/// Acquire global lock for VM creation with retry.
/// This ensures atomic VM ID allocation + port reservation + directory creation.
///
/// Returns a RAII guard that releases the lock when dropped.
/// Returns an error if the lock cannot be acquired within the timeout period.
pub fn acquire_vm_creation_lock(instances_dir: &Path) -> Result<VmCreationLock> {
    acquire_vm_creation_lock_with_timeout(instances_dir, DEFAULT_LOCK_TIMEOUT)
}

/// Acquire global lock for VM creation with a custom timeout.
/// This is the internal implementation that supports configurable timeouts.
fn acquire_vm_creation_lock_with_timeout(
    instances_dir: &Path,
    timeout: Duration,
) -> Result<VmCreationLock> {
    std::fs::create_dir_all(vm_locks_dir(instances_dir))?;

    let lock_path = vm_creation_lock_path(instances_dir);
    let start = std::time::Instant::now();

    loop {
        // Try atomic file creation
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lock_path)
        {
            Ok(mut file) => {
                file.write_all(std::process::id().to_string().as_bytes())?;
                file.flush()?;
                return Ok(VmCreationLock { lock_path });
            }
            Err(err) if err.kind() == ErrorKind::AlreadyExists => {
                // Check for stale lock
                if is_stale_pid_lock(&lock_path) {
                    // Remove stale lock and retry immediately
                    if let Err(e) = std::fs::remove_file(&lock_path) {
                        // Another process may have cleaned it up, just retry
                        if e.kind() != ErrorKind::NotFound {
                            bail!("Failed to remove stale VM creation lock: {}", e);
                        }
                    }
                    // Retry immediately after cleaning up stale lock
                    continue;
                }

                // Check timeout before waiting
                if start.elapsed() >= timeout {
                    bail!(
                        "Timed out waiting for VM creation lock after {:?}. \
                         Another VM creation may be in progress.",
                        timeout
                    );
                }

                // Wait and retry
                thread::sleep(LOCK_RETRY_INTERVAL);
            }
            Err(err) => return Err(err).context("Failed to acquire VM creation lock"),
        }
    }
}

/// Base SSH port for VMs (vm0 uses 2220, vm1 uses 2221, etc.)
pub const SSH_PORT_BASE: u16 = 2220;

/// Calculate SSH port for a VM index.
/// Returns None if index >= MAX_VMS.
pub fn ssh_port_for_vm_index(index: u32) -> Option<u16> {
    if (index as usize) < MAX_VMS {
        Some(SSH_PORT_BASE + index as u16)
    } else {
        None
    }
}

/// Validate that the system can create a new VM.
/// Returns the next VM index if capacity is available.
/// Returns an error if the maximum VM limit is reached.
pub fn validate_vm_capacity(instances_dir: &Path) -> Result<u32> {
    let next_id = crate::vm::next_vm_id(instances_dir)?;
    let index = crate::vm::vm_index_from_id(&next_id)
        .context("Failed to parse VM ID for capacity check")?;

    if (index as usize) >= MAX_VMS {
        bail!(
            "Maximum VM limit ({}) reached. Current VMs use ports {}-{}. \
             Remove an existing VM with 'vm rm <id>' before creating a new one.",
            MAX_VMS,
            SSH_PORT_BASE,
            SSH_PORT_BASE + MAX_VMS as u16 - 1
        );
    }

    Ok(index)
}

/// Check if a port is free (not bound by any process).
/// Used only in tests, kept for backward compatibility.
#[allow(dead_code)]
pub fn is_port_free(port: u16) -> bool {
    for bind_addr in [
        ("0.0.0.0", port),
        ("127.0.0.1", port),
        ("::", port),
        ("::1", port),
    ] {
        match TcpListener::bind(bind_addr) {
            Ok(listener) => {
                drop(listener);
            }
            Err(err) if err.kind() == std::io::ErrorKind::AddrInUse => return false,
            Err(err) => {
                if (bind_addr.0 == "::" || bind_addr.0 == "::1")
                    && (err.kind() == std::io::ErrorKind::Unsupported
                        || err.kind() == std::io::ErrorKind::AddrNotAvailable)
                {
                    continue;
                }
                return false;
            }
        }
    }

    true
}

// ---------------------------------------------------------------------------
// Legacy port reservation (kept for backward compatibility and tests)
// ---------------------------------------------------------------------------

#[derive(Debug)]
#[allow(dead_code)]
pub struct SshPortReservation {
    port: u16,
    lock_path: PathBuf,
}

#[allow(dead_code)]
impl SshPortReservation {
    pub fn port(&self) -> u16 {
        self.port
    }
}

impl Drop for SshPortReservation {
    fn drop(&mut self) {
        if let Err(err) = std::fs::remove_file(&self.lock_path)
            && err.kind() != ErrorKind::NotFound
        {
            eprintln!(
                "Failed to remove SSH port lock {}: {err}",
                self.lock_path.display()
            );
        }
    }
}

pub fn ssh_port_locks_dir(instances_dir: &Path) -> PathBuf {
    instances_dir.join(".ssh-port-locks")
}

pub fn ssh_port_lock_path(instances_dir: &Path, port: u16) -> PathBuf {
    ssh_port_locks_dir(instances_dir).join(format!("{}.lock", port))
}


/// Reserve a specific SSH port with a file lock.
/// This is used for fixed port allocation where the port is predetermined.
/// Returns an error if the port is already reserved by another active process.
pub fn reserve_specific_ssh_port(instances_dir: &Path, port: u16) -> Result<SshPortReservation> {
    std::fs::create_dir_all(ssh_port_locks_dir(instances_dir))?;

    let lock_path = ssh_port_lock_path(instances_dir, port);

    // Clean up stale lock if exists
    if lock_path.exists() && is_stale_pid_lock(&lock_path) {
        let _ = std::fs::remove_file(&lock_path);
    }

    // Try to create lock file (atomic operation)
    match OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&lock_path)
    {
        Ok(mut lock_file) => {
            // Write our PID and return
            lock_file.write_all(std::process::id().to_string().as_bytes())?;
            lock_file.flush()?;
            Ok(SshPortReservation { port, lock_path })
        }
        Err(err) if err.kind() == ErrorKind::AlreadyExists => {
            // Lock file exists and is active - another VM is using this port
            bail!(
                "SSH port {} is already reserved by another VM. \
                 Use 'vm ps' to see running VMs or 'vm rm <id>' to remove a VM.",
                port
            );
        }
        Err(err) => Err(err).context(format!(
            "Failed to reserve SSH port {} with lock {}",
            port,
            lock_path.display()
        )),
    }
}

/// Reserve an SSH port starting from a preferred port.
/// This is the legacy dynamic allocation function, kept for tests and backward compatibility.
#[allow(dead_code)]
pub fn reserve_ssh_port(instances_dir: &Path, preferred_port: u16) -> Result<SshPortReservation> {
    std::fs::create_dir_all(ssh_port_locks_dir(instances_dir))?;

    let mut port = preferred_port;
    loop {
        let lock_path = ssh_port_lock_path(instances_dir, port);

        // Clean up stale lock if exists
        if lock_path.exists() && is_stale_pid_lock(&lock_path) {
            let _ = std::fs::remove_file(&lock_path);
        }

        // Try to create lock file first (atomic operation)
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lock_path)
        {
            Ok(mut lock_file) => {
                // We have the lock, now verify the port is actually free
                if !is_port_free(port) {
                    // Port is occupied, release lock and try next port
                    drop(lock_file);
                    let _ = std::fs::remove_file(&lock_path);
                    port = port.checked_add(1).context(format!(
                        "No available SSH ports found starting from {}.",
                        preferred_port
                    ))?;
                    continue;
                }

                // Port is free, write our PID and return
                lock_file.write_all(std::process::id().to_string().as_bytes())?;
                lock_file.flush()?;
                return Ok(SshPortReservation { port, lock_path });
            }
            Err(err) if err.kind() == ErrorKind::AlreadyExists => {
                // Lock file exists, try next port
                port = port.checked_add(1).context(format!(
                    "No available SSH ports found starting from {}.",
                    preferred_port
                ))?;
            }
            Err(err) => {
                return Err(err).context(format!(
                    "Failed to reserve SSH port {} with lock {}",
                    port,
                    lock_path.display()
                ));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn temp_instances_dir() -> TempDir {
        TempDir::new().expect("failed to create temp instances dir")
    }

    fn find_free_port_hint() -> u16 {
        let mut port = 45_000u16;
        loop {
            if is_port_free(port) {
                return port;
            }
            port = port
                .checked_add(1)
                .expect("failed to locate a free test port");
        }
    }

    // -------------------------------------------------------------------------
    // VM creation lock tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_acquire_vm_creation_lock_success() {
        let instances_dir = temp_instances_dir();

        let lock = acquire_vm_creation_lock(instances_dir.path());
        assert!(lock.is_ok());

        // Lock file should exist
        let lock_path = vm_creation_lock_path(instances_dir.path());
        assert!(lock_path.exists());

        // Lock file should contain our PID
        let content = std::fs::read_to_string(&lock_path).unwrap();
        assert_eq!(content.trim().parse::<u32>().unwrap(), std::process::id());
    }

    #[test]
    fn test_acquire_vm_creation_lock_releases_on_drop() {
        let instances_dir = temp_instances_dir();
        let lock_path = vm_creation_lock_path(instances_dir.path());

        // Acquire and immediately drop
        {
            let _lock = acquire_vm_creation_lock(instances_dir.path()).unwrap();
            assert!(lock_path.exists());
        }

        // Lock file should be removed after drop
        assert!(!lock_path.exists());
    }

    #[test]
    fn test_acquire_vm_creation_lock_times_out_when_held() {
        let instances_dir = temp_instances_dir();

        // First lock succeeds
        let _first = acquire_vm_creation_lock(instances_dir.path()).unwrap();

        // Second lock attempt should time out with very short timeout
        let result =
            acquire_vm_creation_lock_with_timeout(instances_dir.path(), Duration::from_millis(50));
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Timed out waiting for VM creation lock"));
    }

    #[test]
    fn test_acquire_vm_creation_lock_succeeds_after_lock_released() {
        let instances_dir = temp_instances_dir();
        let instances_dir_path = instances_dir.path().to_path_buf();

        // First lock succeeds
        let first = acquire_vm_creation_lock(instances_dir.path()).unwrap();

        // Spawn a thread that will release the lock after a short delay
        let lock_path = vm_creation_lock_path(&instances_dir_path);
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            // Drop the lock to release it
            drop(first);
        });

        // Second lock attempt should succeed with a timeout that's longer than the release delay
        let result =
            acquire_vm_creation_lock_with_timeout(&instances_dir_path, Duration::from_millis(500));
        assert!(
            result.is_ok(),
            "Should have acquired lock after first was released"
        );

        // Verify the lock file contains our PID
        let content = std::fs::read_to_string(&lock_path).unwrap();
        assert_eq!(content.trim().parse::<u32>().unwrap(), std::process::id());
    }

    #[test]
    fn test_acquire_vm_creation_lock_reuses_stale_lock() {
        let instances_dir = temp_instances_dir();
        let lock_path = vm_creation_lock_path(instances_dir.path());

        // Create a stale lock file with a non-existent PID
        std::fs::create_dir_all(vm_locks_dir(instances_dir.path())).unwrap();
        std::fs::write(&lock_path, "99999999").unwrap();

        // Should succeed because the lock is stale
        let lock = acquire_vm_creation_lock(instances_dir.path());
        assert!(lock.is_ok());

        // Lock file should now contain our PID
        let content = std::fs::read_to_string(&lock_path).unwrap();
        assert_eq!(content.trim().parse::<u32>().unwrap(), std::process::id());
    }

    #[test]
    fn test_is_stale_pid_lock_with_dead_pid() {
        let instances_dir = temp_instances_dir();
        let lock_path = vm_creation_lock_path(instances_dir.path());

        std::fs::create_dir_all(vm_locks_dir(instances_dir.path())).unwrap();

        // Non-existent PID should be stale
        std::fs::write(&lock_path, "99999999").unwrap();
        assert!(is_stale_pid_lock(&lock_path));

        // Current process PID should not be stale
        std::fs::write(&lock_path, std::process::id().to_string()).unwrap();
        assert!(!is_stale_pid_lock(&lock_path));
    }

    // -------------------------------------------------------------------------
    // Fixed port allocation tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_ssh_port_for_vm_index_valid() {
        assert_eq!(ssh_port_for_vm_index(0), Some(2220));
        assert_eq!(ssh_port_for_vm_index(1), Some(2221));
        assert_eq!(ssh_port_for_vm_index(5), Some(2225));
        assert_eq!(ssh_port_for_vm_index(9), Some(2229));
    }

    #[test]
    fn test_ssh_port_for_vm_index_exceeds_limit() {
        assert_eq!(ssh_port_for_vm_index(10), None);
        assert_eq!(ssh_port_for_vm_index(11), None);
        assert_eq!(ssh_port_for_vm_index(100), None);
    }

    #[test]
    fn test_validate_vm_capacity_empty_dir() {
        let instances_dir = temp_instances_dir();
        let index = validate_vm_capacity(instances_dir.path()).unwrap();
        assert_eq!(index, 0);
    }

    #[test]
    fn test_validate_vm_capacity_with_existing_vms() {
        let instances_dir = temp_instances_dir();
        let base = instances_dir.path();

        // Create vm0, vm1, vm2 directories
        std::fs::create_dir_all(base.join("vm0")).unwrap();
        std::fs::create_dir_all(base.join("vm1")).unwrap();
        std::fs::create_dir_all(base.join("vm2")).unwrap();

        let index = validate_vm_capacity(base).unwrap();
        assert_eq!(index, 3); // Next should be vm3
    }

    #[test]
    fn test_validate_vm_capacity_at_limit() {
        let instances_dir = temp_instances_dir();
        let base = instances_dir.path();

        // Create vm0 through vm9 (10 VMs)
        for i in 0..10 {
            std::fs::create_dir_all(base.join(format!("vm{}", i))).unwrap();
        }

        let result = validate_vm_capacity(base);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Maximum VM limit (10) reached"));
    }

    #[test]
    fn test_reserve_specific_ssh_port_success() {
        let instances_dir = temp_instances_dir();
        let port = find_free_port_hint();

        let reservation = reserve_specific_ssh_port(instances_dir.path(), port);
        assert!(reservation.is_ok());
        assert_eq!(reservation.unwrap().port(), port);
    }

    #[test]
    fn test_reserve_specific_ssh_port_already_reserved() {
        let instances_dir = temp_instances_dir();
        let port = find_free_port_hint();

        // First reservation should succeed
        let _first = reserve_specific_ssh_port(instances_dir.path(), port).unwrap();

        // Second reservation of the same port should fail
        let second = reserve_specific_ssh_port(instances_dir.path(), port);
        assert!(second.is_err());
        let err = second.unwrap_err().to_string();
        assert!(err.contains("already reserved"));
    }

    #[test]
    fn test_reserve_specific_ssh_port_reuses_stale_lock() {
        let instances_dir = temp_instances_dir();
        let port = find_free_port_hint();

        // Create a stale lock file with a non-existent PID
        std::fs::create_dir_all(ssh_port_locks_dir(instances_dir.path())).unwrap();
        let lock_path = ssh_port_lock_path(instances_dir.path(), port);
        std::fs::write(&lock_path, "99999999").unwrap();

        // Should succeed because the lock is stale
        let reservation = reserve_specific_ssh_port(instances_dir.path(), port);
        assert!(reservation.is_ok());
    }

    // -------------------------------------------------------------------------
    // Legacy dynamic allocation tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_reserve_ssh_port_prefers_preferred_free_port() {
        let instances_dir = temp_instances_dir();
        let instances_dir = instances_dir.path();

        let preferred = find_free_port_hint();
        let reservation =
            reserve_ssh_port(instances_dir, preferred).expect("failed to reserve preferred port");

        assert!(reservation.port() >= preferred);
    }

    #[test]
    fn test_reserve_ssh_port_skips_busy_port() {
        let instances_dir = temp_instances_dir();
        let instances_dir = instances_dir.path();

        let busy_listener =
            TcpListener::bind(("127.0.0.1", 0)).expect("failed to bind occupied port");
        let occupied_port = busy_listener
            .local_addr()
            .expect("failed to read occupied port")
            .port();

        let reservation = reserve_ssh_port(instances_dir, occupied_port)
            .expect("failed to reserve alternate port");

        assert!(reservation.port() > occupied_port);
    }

    #[test]
    fn test_reserve_ssh_port_skips_busy_ipv6_port() {
        let instances_dir = temp_instances_dir();
        let instances_dir = instances_dir.path();

        let busy_listener = match TcpListener::bind(("::1", 0)) {
            Ok(listener) => listener,
            Err(err)
                if err.kind() == std::io::ErrorKind::AddrNotAvailable
                    || err.kind() == std::io::ErrorKind::Unsupported =>
            {
                return;
            }
            Err(err) => panic!("failed to bind IPv6 listener: {err}"),
        };

        let occupied_port = busy_listener
            .local_addr()
            .expect("failed to read occupied ipv6 port")
            .port();

        let reservation = reserve_ssh_port(instances_dir, occupied_port)
            .expect("failed to reserve alternate port");

        assert!(reservation.port() > occupied_port);
    }

    #[test]
    fn test_reserve_ssh_port_avoids_existing_lock() {
        let instances_dir = temp_instances_dir();
        let instances_dir = instances_dir.path();

        let preferred = find_free_port_hint();
        let first =
            reserve_ssh_port(instances_dir, preferred).expect("failed to reserve first port");

        let second =
            reserve_ssh_port(instances_dir, preferred).expect("failed to reserve second port");

        assert_ne!(first.port(), second.port());
        assert!(second.port() >= preferred);
    }

    #[test]
    fn test_reserve_ssh_port_reuses_port_with_stale_lock() {
        let instances_dir = temp_instances_dir();
        let instances_dir = instances_dir.path();
        let stale_lock_port = find_free_port_hint();
        let stale_lock = ssh_port_lock_path(instances_dir, stale_lock_port);

        std::fs::create_dir_all(ssh_port_locks_dir(instances_dir))
            .expect("failed to create lock directory");
        std::fs::write(&stale_lock, "99999999").expect("failed to write stale lock");

        let reservation = reserve_ssh_port(instances_dir, stale_lock_port)
            .expect("failed to reserve from stale lock");

        assert!(reservation.port() >= stale_lock_port);
    }

    #[test]
    fn test_reserve_ssh_port_avoids_active_lock() {
        let instances_dir = temp_instances_dir();
        let instances_dir = instances_dir.path();
        let preferred = find_free_port_hint();

        std::fs::create_dir_all(ssh_port_locks_dir(instances_dir))
            .expect("failed to create lock directory");
        let active_lock = ssh_port_lock_path(instances_dir, preferred);
        std::fs::write(&active_lock, std::process::id().to_string())
            .expect("failed to write active lock");

        let reservation =
            reserve_ssh_port(instances_dir, preferred).expect("failed to skip active lock");

        assert!(reservation.port() > preferred);
    }
}
