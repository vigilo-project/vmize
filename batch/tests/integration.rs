use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use batch::{run_task_blocking, run_task_blocking_with_options, TaskRunOptions};

fn fixture_input_dir(scripts: &[&str], scripts_dir_rel: &str) -> PathBuf {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let fixture_dir: PathBuf = manifest_dir.join(scripts_dir_rel);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_else(|_| Default::default());
    let input_dir = std::env::temp_dir().join(format!(
        "batch-it-input-{}-{}",
        std::process::id(),
        now.as_nanos()
    ));

    fs::create_dir_all(&input_dir).unwrap();
    for &script in scripts {
        fs::copy(fixture_dir.join(script), input_dir.join(script)).unwrap();
    }

    input_dir
}

fn collect_shell_scripts(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = fs::read_dir(dir)
        .unwrap_or_else(|err| panic!("failed to read directory {}: {err}", dir.display()));

    for entry in entries {
        let entry = entry.unwrap_or_else(|err| {
            panic!("failed to read directory entry in {}: {err}", dir.display())
        });
        let path = entry.path();
        let file_type = entry
            .file_type()
            .unwrap_or_else(|err| panic!("failed to read file type for {}: {err}", path.display()));

        if file_type.is_dir() {
            collect_shell_scripts(&path, out);
            continue;
        }

        if path.extension().is_some_and(|ext| ext == "sh") {
            out.push(path);
        }
    }
}

#[test]
fn run_task_with_options_example_scripts_collects_outputs() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let input_dir = manifest_dir.join("example/task1/scripts");
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_else(|_| Default::default());
    let output_dir = std::env::temp_dir().join(format!(
        "batch-it-output-examples-{}-{}",
        std::process::id(),
        now.as_nanos()
    ));

    if output_dir.exists() {
        fs::remove_dir_all(&output_dir).unwrap();
    }
    fs::create_dir_all(&output_dir).unwrap();

    let result = run_task_blocking(&input_dir, &output_dir).expect("integration execution failed");

    let expected = vec!["00_print.sh".to_string(), "10_result.sh".to_string()];
    assert_eq!(result.exit_code, 0);
    assert_eq!(result.executed_scripts, expected);
    assert_eq!(result.output_dir, output_dir);

    assert!(output_dir.join("00_print.sh.log").exists());
    assert!(output_dir.join("10_result.sh.log").exists());
    assert!(output_dir.join("hello.txt").exists());
    assert!(output_dir.join("result.txt").exists());
    assert!(fs::read_to_string(output_dir.join("00_print.sh.log"))
        .unwrap()
        .contains("hello from vm"));
    assert!(fs::read_to_string(output_dir.join("hello.txt"))
        .unwrap()
        .trim()
        .contains("alpha"));
}

#[test]
fn run_task_with_options_ollama_prompt_collects_answer() {
    if std::env::var("BATCH_OLLAMA_IT").is_err() {
        eprintln!("Skipping Ollama integration test: set BATCH_OLLAMA_IT=1 to run.");
        return;
    }

    let input_dir = fixture_input_dir(&["20_ollama_prompt.sh"], "../tasks/ollama/scripts");
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_else(|_| Default::default());
    let output_dir = std::env::temp_dir().join(format!(
        "batch-it-output-ollama-{}-{}",
        std::process::id(),
        now.as_nanos()
    ));

    if output_dir.exists() {
        fs::remove_dir_all(&output_dir).unwrap();
    }
    fs::create_dir_all(&output_dir).unwrap();

    let options = TaskRunOptions {
        disk_size: Some("20G".to_string()),
        ..Default::default()
    };
    let result = match run_task_blocking_with_options(&input_dir, &output_dir, options) {
        Ok(r) => r,
        Err(err) => {
            // Print any logs that were copied back before panicking
            let log_path = output_dir.join("20_ollama_prompt.sh.log");
            if log_path.exists() {
                let log = fs::read_to_string(&log_path).unwrap_or_default();
                eprintln!("--- 20_ollama_prompt.sh.log ---\n{log}\n--- end ---");
            }
            let err_path = output_dir.join("ollama-error.txt");
            if err_path.exists() {
                let err_log = fs::read_to_string(&err_path).unwrap_or_default();
                eprintln!("--- ollama-error.txt ---\n{err_log}\n--- end ---");
            }
            panic!("ollama integration execution failed: {err:?}");
        }
    };

    assert_eq!(result.exit_code, 0);
    assert_eq!(
        result.executed_scripts,
        vec!["20_ollama_prompt.sh".to_string()]
    );
    assert_eq!(result.output_dir, output_dir);

    assert!(output_dir.join("20_ollama_prompt.sh.log").exists());
    let answer = fs::read_to_string(output_dir.join("ollama-answer.txt")).unwrap();
    assert!(
        !answer.trim().is_empty(),
        "ollama-answer.txt must not be empty"
    );
}

#[test]
fn all_example_shell_scripts_pass_bash_n() {
    let scripts_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("example");
    let mut scripts = Vec::new();
    collect_shell_scripts(&scripts_root, &mut scripts);
    scripts.sort();

    assert!(
        !scripts.is_empty(),
        "expected shell scripts under {}",
        scripts_root.display()
    );

    for script in scripts {
        let output = Command::new("bash")
            .arg("-n")
            .arg(&script)
            .output()
            .unwrap_or_else(|err| panic!("failed to run bash -n on {}: {err}", script.display()));

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            panic!(
                "bash -n failed for {} with status {:?}: {}",
                script.display(),
                output.status,
                stderr
            );
        }
    }
}
