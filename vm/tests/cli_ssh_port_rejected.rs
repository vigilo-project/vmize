use std::path::PathBuf;
use std::process::Command;

fn vm_bin_path() -> String {
    std::env::var("CARGO_BIN_EXE_vm").unwrap_or_else(|_| {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("target")
            .join("debug")
            .join("vm")
            .to_string_lossy()
            .into_owned()
    })
}

#[test]
fn vm_run_rejects_ssh_port_flag() {
    let vm_bin = vm_bin_path();
    let output = Command::new(&vm_bin)
        .args(["run", "--ssh-port", "2222"])
        .output()
        .expect("failed to execute vm binary");

    assert!(
        !output.status.success(),
        "vm run should reject --ssh-port flag after API cleanup"
    );

    let stderr = String::from_utf8_lossy(&output.stderr).to_lowercase();
    assert!(
        stderr.contains("unexpected argument")
            || stderr.contains("unrecognized option")
            || stderr.contains("unknown argument"),
        "expected parse error, stderr: {stderr}"
    );
}
