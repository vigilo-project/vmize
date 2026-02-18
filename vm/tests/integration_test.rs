use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, Instant};
use tokio::time::sleep;

fn project_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn vm_bin_path() -> String {
    std::env::var("CARGO_BIN_EXE_vm").unwrap_or_else(|_| {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("target")
            .join("debug")
            .join("vm")
            .to_string_lossy()
            .into_owned()
    })
}

fn default_data_dir() -> PathBuf {
    let home = std::env::var("HOME").expect("HOME not set");
    PathBuf::from(home).join(".local").join("share").join("vm")
}

/// Kill any QEMU processes left over from a previous test run,
/// and remove stale SSH mux sockets.
fn kill_stale_test_qemu() {
    // Remove stale SSH mux sockets that openssh may have left behind.
    let mux_dir = Path::new("/tmp/.ssh-mux-ubuntu");
    if mux_dir.is_dir() {
        eprintln!("Removing stale SSH mux directory: {}", mux_dir.display());
        let _ = std::fs::remove_dir_all(mux_dir);
    }
}

#[tokio::test]
async fn test_vm_run_ssh_apt() {
    let username = "ubuntu";
    log_progress("Starting integration test: run → ssh → cp → cleanup");
    kill_stale_test_qemu();
    let data_dir = default_data_dir();
    let mut cleanup = VmCleanupGuard::new();
    log_progress(&format!("Using data dir: {}", data_dir.display()));

    log_progress("Running vm run...");
    let vm_bin = vm_bin_path();
    let run_output = Command::new(&vm_bin)
        .args(["run", "--username", username, "--ssh-port", "4445"])
        .current_dir(project_dir())
        .output()
        .expect("Failed to run vm");

    assert!(
        run_output.status.success(),
        "vm run failed:\nstdout:{}\nstderr:{}",
        String::from_utf8_lossy(&run_output.stdout),
        String::from_utf8_lossy(&run_output.stderr)
    );
    log_progress("vm run finished");

    let last_line = String::from_utf8_lossy(&run_output.stdout)
        .lines()
        .last()
        .unwrap_or("")
        .trim()
        .to_string();
    // run outputs "vm ssh vm0" in non-verbose mode; extract the last token as VM ID
    let vm_id = last_line
        .split_whitespace()
        .last()
        .unwrap_or("")
        .to_string();

    assert!(!vm_id.is_empty(), "VM ID was not returned from run");
    assert!(
        vm_id.starts_with("vm"),
        "Expected sequential VM id (vm0/vm1...), got: {vm_id}"
    );

    cleanup.vm_id = Some(vm_id.clone());

    let key_path = read_vm_key_path(&vm_id, &data_dir);
    let ssh_port = read_vm_ssh_port(&vm_id, &data_dir);
    log_progress(&format!("Waiting for SSH on port {ssh_port}..."));

    wait_for_ssh(
        &key_path,
        ssh_port,
        username,
        Duration::from_secs(120),
        Duration::from_secs(5),
    )
    .await;

    let test_commands: Vec<(&str, Option<&str>)> = vec![
        ("hostname", Some("vm")),
        ("whoami", Some(username)),
        ("getent hosts example.com", Some("example.com")),
        ("sudo apt-get update", None),
        ("sudo apt-get install -y jq", None),
        ("jq --version", None),
    ];

    for (cmd, expected_output) in test_commands {
        log_progress(&format!("Running SSH command: {cmd}"));
        let output = run_vm_ssh_command(&vm_id, cmd);

        assert!(
            output.status.success(),
            "Command '{}' failed.\nstdout:\n{}\nstderr:\n{}",
            cmd,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );

        let stdout = String::from_utf8_lossy(&output.stdout);

        if let Some(expected) = expected_output {
            assert!(
                stdout.contains(expected),
                "Command '{}' output should contain '{}', got: {}",
                cmd,
                expected,
                stdout
            );
        }

        log_progress(&format!("✓ Command '{cmd}' passed"));
    }

    // exit code propagation: verify that a failing command reports failure
    log_progress("Verifying non-zero exit code propagation...");
    let fail_output = run_vm_ssh_command(&vm_id, "false");
    assert!(
        !fail_output.status.success(),
        "Command 'false' should have failed but reported success"
    );
    log_progress("✓ Non-zero exit code propagation verified");

    // Test cp: local -> VM
    let tmp_dir = std::env::temp_dir().join("vm-test");
    std::fs::create_dir_all(&tmp_dir).expect("Failed to create temp dir");
    let local_file = tmp_dir.join("transfer_test.txt");
    std::fs::write(&local_file, "hello from vm cp").expect("Failed to write test file");
    log_progress("Testing cp local->vm...");

    let cp_output = Command::new(&vm_bin)
        .args([
            "cp",
            local_file.to_str().unwrap(),
            &format!("{vm_id}:/tmp/transfer_test.txt"),
        ])
        .current_dir(project_dir())
        .output()
        .expect("Failed to run cp local->vm");

    assert!(
        cp_output.status.success(),
        "cp local->vm failed:\nstdout:{}\nstderr:{}",
        String::from_utf8_lossy(&cp_output.stdout),
        String::from_utf8_lossy(&cp_output.stderr)
    );
    log_progress("✓ cp local->vm passed");

    // Verify the file arrived on the VM
    let cat_output = run_vm_ssh_command(&vm_id, "cat /tmp/transfer_test.txt");
    assert!(cat_output.status.success(), "cat copied file failed");
    let cat_stdout = String::from_utf8_lossy(&cat_output.stdout);
    assert!(
        cat_stdout.contains("hello from vm cp"),
        "Transferred file content mismatch: {}",
        cat_stdout
    );
    log_progress("✓ cp local->vm content verified");

    // Test cp: VM -> local
    let pulled_file = tmp_dir.join("transferred_test.txt");
    let cp_from_vm_output = Command::new(&vm_bin)
        .args([
            "cp",
            &format!("{vm_id}:/tmp/transfer_test.txt"),
            pulled_file.to_str().unwrap(),
        ])
        .current_dir(project_dir())
        .output()
        .expect("Failed to run cp vm->local");

    assert!(
        cp_from_vm_output.status.success(),
        "cp vm->local failed:\nstdout:{}\nstderr:{}",
        String::from_utf8_lossy(&cp_from_vm_output.stdout),
        String::from_utf8_lossy(&cp_from_vm_output.stderr)
    );

    let pulled_content = std::fs::read_to_string(&pulled_file).expect("Failed to read pulled file");
    assert!(
        pulled_content.contains("hello from vm cp"),
        "Pulled file content mismatch: {}",
        pulled_content
    );
    log_progress("✓ cp vm->local passed");

    // Test cp -r: recursive directory copy local -> VM
    let local_dir = tmp_dir.join("transfer_dir");
    std::fs::create_dir_all(local_dir.join("subdir")).expect("Failed to create test subdir");
    std::fs::write(local_dir.join("a.txt"), "file a").expect("Failed to write a.txt");
    std::fs::write(local_dir.join("subdir/b.txt"), "file b").expect("Failed to write b.txt");
    log_progress("Testing cp -r local->vm...");

    let cp_r_output = Command::new(&vm_bin)
        .args([
            "cp",
            "-r",
            local_dir.to_str().unwrap(),
            &format!("{vm_id}:/tmp/transfer_dir"),
        ])
        .current_dir(project_dir())
        .output()
        .expect("Failed to run cp -r local->vm");

    assert!(
        cp_r_output.status.success(),
        "cp -r local->vm failed:\nstdout:{}\nstderr:{}",
        String::from_utf8_lossy(&cp_r_output.stdout),
        String::from_utf8_lossy(&cp_r_output.stderr)
    );

    let cat_r_output = run_vm_ssh_command(&vm_id, "cat /tmp/transfer_dir/subdir/b.txt");
    assert!(cat_r_output.status.success(), "cat recursive file failed");
    let cat_r_stdout = String::from_utf8_lossy(&cat_r_output.stdout);
    assert!(
        cat_r_stdout.contains("file b"),
        "Recursive copied file content mismatch: {}",
        cat_r_stdout
    );
    log_progress("✓ cp -r local->vm passed");

    // Cleanup temp dir
    let _ = std::fs::remove_dir_all(&tmp_dir);

    cleanup.stop_with_assert();

    log_progress("✓ Integration test passed");
}

fn read_vm_key_path(vm_id: &str, data_dir: &Path) -> PathBuf {
    let record_path = data_dir.join("instances").join(vm_id).join("vm.json");
    let data = std::fs::read_to_string(record_path).expect("Failed to read VM record");
    let record: serde_json::Value = serde_json::from_str(&data).expect("Failed to parse VM record");
    PathBuf::from(
        record["private_key_path"]
            .as_str()
            .expect("Missing private_key_path"),
    )
}

fn read_vm_ssh_port(vm_id: &str, data_dir: &Path) -> u16 {
    let record_path = data_dir.join("instances").join(vm_id).join("vm.json");
    let data = std::fs::read_to_string(record_path).expect("Failed to read VM record");
    let record: serde_json::Value = serde_json::from_str(&data).expect("Failed to parse VM record");
    record["ssh_port"]
        .as_u64()
        .and_then(|port| u16::try_from(port).ok())
        .expect("Missing ssh_port")
}

async fn wait_for_ssh(
    key_path: &Path,
    ssh_port: u16,
    username: &str,
    timeout: Duration,
    retry_interval: Duration,
) {
    let deadline = Instant::now() + timeout;
    let start = Instant::now();
    let mut attempt: u32 = 0;

    loop {
        attempt += 1;
        let output = run_ssh_command(key_path, ssh_port, username, "true");
        if output.status.success() {
            log_progress(&format!(
                "SSH ready after {} attempts ({:?})",
                attempt,
                start.elapsed()
            ));
            return;
        }

        let last_error = format!(
            "stdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );

        if Instant::now() >= deadline {
            panic!(
                "SSH did not become ready within {:?}. Last error:\n{}",
                timeout, last_error
            );
        }

        if attempt.is_multiple_of(5) {
            log_progress(&format!(
                "Waiting for SSH on {} (attempt {}, elapsed {:?}): {}",
                ssh_port,
                attempt,
                start.elapsed(),
                last_error.replace('\n', " ")
            ));
        }

        sleep(retry_interval).await;
    }
}

fn log_progress(message: &str) {
    eprintln!("{}", message);
    let _ = io::stdout().flush();
    let _ = io::stderr().flush();
}

fn run_ssh_command(key_path: &Path, ssh_port: u16, username: &str, cmd: &str) -> Output {
    Command::new("ssh")
        .args([
            "-i",
            key_path.to_str().expect("Invalid key path"),
            "-p",
            &ssh_port.to_string(),
            "-o",
            "BatchMode=yes",
            "-o",
            "StrictHostKeyChecking=no",
            "-o",
            "UserKnownHostsFile=/dev/null",
            "-o",
            "ConnectTimeout=10",
            &format!("{}@127.0.0.1", username),
            cmd,
        ])
        .output()
        .unwrap_or_else(|e| panic!("Failed to execute SSH command '{}': {}", cmd, e))
}

fn run_vm_ssh_command(vm_id: &str, cmd: &str) -> Output {
    Command::new(vm_bin_path())
        .args(["ssh", vm_id, cmd])
        .current_dir(project_dir())
        .output()
        .unwrap_or_else(|e| panic!("Failed to run vm ssh '{}': {}", cmd, e))
}

struct VmCleanupGuard {
    vm_id: Option<String>,
}

impl VmCleanupGuard {
    fn new() -> Self {
        Self { vm_id: None }
    }

    fn stop_with_assert(&mut self) {
        if let Some(vm_id) = self.vm_id.as_deref() {
            if let Err(err) = self.try_stop(vm_id) {
                panic!("Failed to stop VM during cleanup: {}", err);
            }
            self.vm_id = None;
        }
    }

    fn try_stop(&self, vm_id: &str) -> Result<(), String> {
        let output = Command::new(vm_bin_path())
            .args(["rm", vm_id])
            .current_dir(project_dir())
            .output()
            .map_err(|e| format!("Failed to execute stop command: {}", e))?;

        if !output.status.success() {
            return Err(format!(
                "Stop command failed.\nstdout:\n{}\nstderr:\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        Ok(())
    }
}

impl Drop for VmCleanupGuard {
    fn drop(&mut self) {
        if let Some(vm_id) = self.vm_id.as_deref() {
            if let Err(err) = self.try_stop(vm_id) {
                eprintln!("Cleanup warning: {}", err);
            }
        }
    }
}
