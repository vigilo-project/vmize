use crate::process::is_process_alive;
use anyhow::{Context, Result};
use std::fs::OpenOptions;
use std::io::{ErrorKind, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub struct SshPortReservation {
    port: u16,
    lock_path: PathBuf,
}

impl SshPortReservation {
    pub fn port(&self) -> u16 {
        self.port
    }
}

impl Drop for SshPortReservation {
    fn drop(&mut self) {
        if let Err(err) = std::fs::remove_file(&self.lock_path) {
            if err.kind() != ErrorKind::NotFound {
                eprintln!(
                    "Failed to remove SSH port lock {}: {err}",
                    self.lock_path.display()
                );
            }
        }
    }
}

pub fn ssh_port_locks_dir(instances_dir: &Path) -> PathBuf {
    instances_dir.join(".ssh-port-locks")
}

pub fn ssh_port_lock_path(instances_dir: &Path, port: u16) -> PathBuf {
    ssh_port_locks_dir(instances_dir).join(format!("{}.lock", port))
}

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

fn is_stale_ssh_lock(lock_path: &Path) -> bool {
    let lock_pid = match std::fs::read_to_string(lock_path) {
        Ok(content) => match content.trim().parse::<u32>() {
            Ok(pid) => pid,
            Err(_) => return true,
        },
        Err(err) => {
            if err.kind() == ErrorKind::NotFound {
                return true;
            }
            return false;
        }
    };

    !is_process_alive(lock_pid)
}

pub fn reserve_ssh_port(instances_dir: &Path, preferred_port: u16) -> Result<SshPortReservation> {
    std::fs::create_dir_all(ssh_port_locks_dir(instances_dir))?;

    let mut port = preferred_port;
    loop {
        let lock_path = ssh_port_lock_path(instances_dir, port);

        if lock_path.exists() && is_stale_ssh_lock(&lock_path) {
            let _ = std::fs::remove_file(&lock_path);
        }

        if !is_port_free(port) {
            port = port.checked_add(1).context(format!(
                "No available SSH ports found starting from {}.",
                preferred_port
            ))?;
            continue;
        }

        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lock_path)
        {
            Ok(mut lock_file) => {
                lock_file.write_all(std::process::id().to_string().as_bytes())?;
                lock_file.flush()?;
                return Ok(SshPortReservation { port, lock_path });
            }
            Err(err) if err.kind() == ErrorKind::AlreadyExists => {
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
