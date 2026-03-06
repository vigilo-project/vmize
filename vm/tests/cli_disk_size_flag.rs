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
fn vm_run_help_includes_disk_size_flag() {
    let vm_bin = vm_bin_path();
    let output = Command::new(&vm_bin)
        .args(["run", "--help"])
        .output()
        .expect("failed to execute vm binary");

    assert!(output.status.success(), "expected help to succeed");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("--disk-size"),
        "expected run help to include --disk-size, got:\n{stdout}"
    );
}
