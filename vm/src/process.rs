use std::process::Command;

pub fn is_process_running(pid: u32) -> bool {
    Command::new("kill")
        .arg("-0")
        .arg(pid.to_string())
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false)
}

pub fn is_process_alive(pid: u32) -> bool {
    if !is_process_running(pid) {
        return false;
    }

    if let Ok(output) = Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "state="])
        .output()
        && output.status.success() {
            let state = String::from_utf8_lossy(&output.stdout)
                .trim()
                .to_ascii_uppercase();
            return !state.starts_with('Z');
        }

    true
}

pub fn is_qemu_process(pid: u32) -> bool {
    if !is_process_running(pid) {
        return false;
    }

    match Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "args="])
        .output()
    {
        Ok(output) if output.status.success() => {
            let output = String::from_utf8_lossy(&output.stdout).to_ascii_lowercase();
            output.contains("qemu-system-")
        }
        _ => false,
    }
}

pub fn is_process_absent_error(err: &str) -> bool {
    err.contains("No such process") || err.contains("does not exist")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_process_running_current() {
        assert!(is_process_running(std::process::id()));
    }

    #[test]
    fn test_is_process_running_invalid() {
        assert!(!is_process_running(999999));
    }

    #[test]
    fn test_is_process_alive_current() {
        assert!(is_process_alive(std::process::id()));
    }

    #[test]
    fn test_is_process_absent_error_matches() {
        assert!(is_process_absent_error("No such process"));
        assert!(is_process_absent_error("process does not exist"));
        assert!(!is_process_absent_error("some other error"));
    }
}
