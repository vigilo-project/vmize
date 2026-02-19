use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use task::{LoadedTask, TaskDefinition, load_task};
use worker::{TaskRunOptions, run_loaded_task_blocking};

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

fn unique_output_dir(label: &str) -> PathBuf {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    std::env::temp_dir().join(format!(
        "worker-it-{}-{}-{}",
        label,
        std::process::id(),
        now.as_nanos()
    ))
}

#[test]
fn run_task_example_task1_collects_outputs() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let task_dir = manifest_dir.join("example/task1");

    let loaded = load_task(&task_dir).expect("load_task must succeed for example/task1");

    // Override output_dir to a temp path so we don't pollute the repo
    let output_dir = unique_output_dir("task1");
    let logs_dir = output_dir.join("logs");
    fs::create_dir_all(&logs_dir).unwrap();

    let task = LoadedTask {
        definition: loaded.definition,
        input_dir: loaded.input_dir,
        output_dir: output_dir.clone(),
        logs_dir: logs_dir.clone(),
    };

    let result = run_loaded_task_blocking(&task, TaskRunOptions::default())
        .expect("integration execution failed");

    let expected = vec!["00_print.sh".to_string(), "10_result.sh".to_string()];
    assert_eq!(result.exit_code, 0);
    assert_eq!(result.executed_commands, expected);
    assert_eq!(result.output_dir, output_dir);

    // Log files are in output/logs/
    assert!(logs_dir.join("00_print.sh.log").exists());
    assert!(logs_dir.join("10_result.sh.log").exists());

    // Script output files are in output/
    assert!(output_dir.join("hello.txt").exists());
    assert!(output_dir.join("result.txt").exists());

    assert!(
        fs::read_to_string(logs_dir.join("00_print.sh.log"))
            .unwrap()
            .contains("hello from vm")
    );
    assert!(
        fs::read_to_string(output_dir.join("hello.txt"))
            .unwrap()
            .trim()
            .contains("alpha")
    );
}

#[test]
fn run_task_ollama_prompt_collects_answer() {
    if std::env::var("BATCH_OLLAMA_IT").is_err() {
        eprintln!("Skipping Ollama integration test: set BATCH_OLLAMA_IT=1 to run.");
        return;
    }

    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let task_dir = manifest_dir.join("../example/ollama");

    let loaded = load_task(&task_dir).expect("load_task must succeed for example/ollama");

    let output_dir = unique_output_dir("ollama");
    let logs_dir = output_dir.join("logs");
    fs::create_dir_all(&logs_dir).unwrap();

    let task = LoadedTask {
        definition: TaskDefinition {
            disk_size: Some("20G".to_string()),
            ..loaded.definition
        },
        input_dir: loaded.input_dir,
        output_dir: output_dir.clone(),
        logs_dir: logs_dir.clone(),
    };

    let result = match run_loaded_task_blocking(&task, TaskRunOptions::default()) {
        Ok(r) => r,
        Err(err) => {
            let log_path = logs_dir.join("20_ollama_prompt.sh.log");
            if log_path.exists() {
                let log = fs::read_to_string(&log_path).unwrap_or_default();
                eprintln!("--- 20_ollama_prompt.sh.log ---\n{log}\n--- end ---");
            }
            panic!("ollama integration execution failed: {err:?}");
        }
    };

    assert_eq!(result.exit_code, 0);
    assert_eq!(result.output_dir, output_dir);

    assert!(logs_dir.join("20_ollama_prompt.sh.log").exists());
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
