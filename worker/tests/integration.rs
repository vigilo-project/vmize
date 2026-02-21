use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use task::{LoadedTask, TaskDefinition, load_task};
use worker::{TaskRunOptions, run_loaded_task_blocking, run_task_chain_blocking};

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

fn vm_test_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
}

fn copy_dir_recursive(src: &Path, dst: &Path) {
    fs::create_dir_all(dst)
        .unwrap_or_else(|err| panic!("failed to create directory {}: {err}", dst.display()));

    for entry in fs::read_dir(src)
        .unwrap_or_else(|err| panic!("failed to read directory {}: {err}", src.display()))
    {
        let entry = entry.unwrap_or_else(|err| {
            panic!("failed to read directory entry in {}: {err}", src.display())
        });
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        let file_type = entry.file_type().unwrap_or_else(|err| {
            panic!("failed to read file type for {}: {err}", src_path.display())
        });

        if file_type.is_dir() {
            copy_dir_recursive(&src_path, &dst_path);
        } else {
            fs::copy(&src_path, &dst_path).unwrap_or_else(|err| {
                panic!(
                    "failed to copy fixture file {} -> {}: {err}",
                    src_path.display(),
                    dst_path.display()
                )
            });
        }
    }
}

fn create_task_fixture(
    root: &Path,
    task_dir_name: &str,
    task_json: &str,
    scripts: &[(&str, &str)],
) -> PathBuf {
    let task_dir = root.join(task_dir_name);
    let input_dir = task_dir.join("input");
    fs::create_dir_all(&input_dir)
        .unwrap_or_else(|err| panic!("failed to create input dir {}: {err}", input_dir.display()));
    fs::write(task_dir.join("task.json"), task_json).unwrap_or_else(|err| {
        panic!(
            "failed to write task.json for {}: {err}",
            task_dir.display()
        )
    });

    for (name, body) in scripts {
        fs::write(input_dir.join(name), body).unwrap_or_else(|err| {
            panic!(
                "failed to write script {} in {}: {err}",
                name,
                task_dir.display()
            )
        });
    }

    task_dir
}

#[test]
fn run_task_example_task1_collects_outputs() {
    let _guard = vm_test_lock();

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
    let _guard = vm_test_lock();

    if std::env::var("BATCH_OLLAMA_IT").is_err() {
        eprintln!("Skipping Ollama integration test: set BATCH_OLLAMA_IT=1 to run.");
        return;
    }

    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let task_dir = manifest_dir.join("example/ollama");

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
fn run_task_chain_example_passes_artifacts_to_next_task() {
    let _guard = vm_test_lock();

    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let fixture_root = manifest_dir.join("example");

    let temp_root = unique_output_dir("chain-fixture");
    copy_dir_recursive(
        &fixture_root.join("chain-task1"),
        &temp_root.join("chain-task1"),
    );
    copy_dir_recursive(
        &fixture_root.join("chain-task2"),
        &temp_root.join("chain-task2"),
    );

    let chain_start = temp_root.join("chain-task1");
    let chain = run_task_chain_blocking(&chain_start, TaskRunOptions::default())
        .expect("chain execution failed");

    assert_eq!(chain.steps.len(), 2);
    assert_eq!(
        chain.steps[0].handoff_artifacts,
        vec!["handoff.txt".to_string()]
    );

    let first_output = chain.steps[0].run_result.output_dir.clone();
    let second_output = chain.steps[1].run_result.output_dir.clone();

    assert!(first_output.join("handoff.txt").exists());
    assert!(second_output.join("final.txt").exists());

    let final_output = fs::read_to_string(second_output.join("final.txt")).unwrap();
    assert_eq!(final_output.trim(), "VMIZE-CHAIN");

    let _ = fs::remove_dir_all(&temp_root);
}

#[test]
fn run_task_chain_directory_artifact_handoff_succeeds() {
    let _guard = vm_test_lock();

    let temp_root = unique_output_dir("chain-dir-artifact");
    fs::create_dir_all(&temp_root).unwrap();

    let task1_json = r#"{
  "name": "chain-dir-producer",
  "commands": ["00_make_rootfs.sh"],
  "artifacts": ["rootfs"],
  "next_task_dir": "../task2"
}"#;
    let task2_json = r#"{
  "name": "chain-dir-consumer",
  "commands": ["00_consume_rootfs.sh"],
  "artifacts": ["final.txt"]
}"#;

    create_task_fixture(
        &temp_root,
        "task1",
        task1_json,
        &[(
            "00_make_rootfs.sh",
            "#!/usr/bin/env bash\nset -euo pipefail\nmkdir -p /tmp/vmize-worker/out/rootfs/bin\necho \"chain-rootfs-ok\" > /tmp/vmize-worker/out/rootfs/bin/llama-cli\n",
        )],
    );
    create_task_fixture(
        &temp_root,
        "task2",
        task2_json,
        &[(
            "00_consume_rootfs.sh",
            "#!/usr/bin/env bash\nset -euo pipefail\nif [[ ! -f /tmp/vmize-worker/work/rootfs/bin/llama-cli ]]; then\n  echo \"missing rootfs handoff\" >&2\n  exit 1\nfi\ncat /tmp/vmize-worker/work/rootfs/bin/llama-cli > /tmp/vmize-worker/out/final.txt\n",
        )],
    );

    let chain = run_task_chain_blocking(&temp_root.join("task1"), TaskRunOptions::default())
        .expect("directory artifact chain execution failed");

    assert_eq!(chain.steps.len(), 2);
    assert_eq!(chain.steps[0].handoff_artifacts, vec!["rootfs".to_string()]);
    let final_output = chain.steps[1].run_result.output_dir.join("final.txt");
    assert!(final_output.exists());
    assert_eq!(
        fs::read_to_string(final_output).unwrap().trim(),
        "chain-rootfs-ok"
    );

    let _ = fs::remove_dir_all(&temp_root);
}

#[test]
fn run_task_chain_fails_on_handoff_conflict() {
    let _guard = vm_test_lock();

    let temp_root = unique_output_dir("chain-handoff-conflict");
    fs::create_dir_all(&temp_root).unwrap();

    let task1_json = r#"{
  "name": "chain-conflict-producer",
  "commands": ["00_make_rootfs.sh"],
  "artifacts": ["rootfs"],
  "next_task_dir": "../task2"
}"#;
    let task2_json = r#"{
  "name": "chain-conflict-consumer",
  "commands": []
}"#;

    create_task_fixture(
        &temp_root,
        "task1",
        task1_json,
        &[(
            "00_make_rootfs.sh",
            "#!/usr/bin/env bash\nset -euo pipefail\nmkdir -p /tmp/vmize-worker/out/rootfs/bin\necho \"ok\" > /tmp/vmize-worker/out/rootfs/bin/tool\n",
        )],
    );
    let task2_dir = create_task_fixture(&temp_root, "task2", task2_json, &[]);
    fs::create_dir_all(task2_dir.join("input").join("rootfs")).unwrap();

    let err = run_task_chain_blocking(&temp_root.join("task1"), TaskRunOptions::default())
        .expect_err("expected handoff conflict");
    match err {
        worker::Error::Runtime { message } => assert!(message.contains("handoff conflict")),
        other => panic!("expected Runtime error for handoff conflict, got: {other}"),
    }

    let _ = fs::remove_dir_all(&temp_root);
}

#[test]
fn run_task_chain_fails_when_handoff_artifact_is_missing() {
    let _guard = vm_test_lock();

    let temp_root = unique_output_dir("chain-missing-artifact");
    fs::create_dir_all(&temp_root).unwrap();

    let task1_json = r#"{
  "name": "chain-missing-producer",
  "commands": ["00_noop.sh"],
  "artifacts": ["missing.txt"],
  "next_task_dir": "../task2"
}"#;
    let task2_json = r#"{
  "name": "chain-missing-consumer",
  "commands": ["00_consume.sh"],
  "artifacts": ["done.txt"]
}"#;

    create_task_fixture(
        &temp_root,
        "task1",
        task1_json,
        &[(
            "00_noop.sh",
            "#!/usr/bin/env bash\nset -euo pipefail\necho \"noop\" > /tmp/vmize-worker/out/unrelated.txt\n",
        )],
    );
    create_task_fixture(
        &temp_root,
        "task2",
        task2_json,
        &[(
            "00_consume.sh",
            "#!/usr/bin/env bash\nset -euo pipefail\necho \"done\" > /tmp/vmize-worker/out/done.txt\n",
        )],
    );

    let err = run_task_chain_blocking(&temp_root.join("task1"), TaskRunOptions::default())
        .expect_err("expected missing handoff artifact error");
    match err {
        worker::Error::Runtime { message } => {
            assert!(message.contains("Failed to collect VM output"));
            assert!(
                message.contains("scp exited with status")
                    || message.contains("No such file or directory")
            );
        }
        other => panic!("expected Runtime error for missing artifact, got: {other}"),
    }

    let _ = fs::remove_dir_all(&temp_root);
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
