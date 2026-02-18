use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use batch::{run_in_out_blocking, run_in_out_blocking_with, RunInOutOptions};

#[derive(Debug)]
struct JobExpectation {
    name: &'static str,
    output_dir: PathBuf,
    job_file: PathBuf,
    required_outputs: Vec<&'static str>,
}

fn expect_job_outputs(expectation: &JobExpectation) {
    let missing: Vec<&'static str> = expectation
        .required_outputs
        .iter()
        .copied()
        .filter(|item| !expectation.output_dir.join(item).exists())
        .collect();

    assert!(
        missing.is_empty(),
        "job '{}' did not produce expected outputs in {:?}: {:?}",
        expectation.name,
        expectation.output_dir,
        missing
    );
}

fn failed_job_from_output(output: &str, jobs: &[JobExpectation]) -> Option<String> {
    for job in jobs {
        if output.contains(&format!("Failed to execute {}", job.name)) {
            return Some(job.name.to_string());
        }
        if output.contains(&format!("Failed to load job {}", job.job_file.display())) {
            return Some(job.name.to_string());
        }
    }
    None
}

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

fn fixture_simple_output_dir(label: &str) -> PathBuf {
    let output_dir = Path::new("/tmp").join("batch").join(label);
    if output_dir.exists() {
        fs::remove_dir_all(&output_dir).unwrap();
    }
    fs::create_dir_all(&output_dir).unwrap();
    output_dir
}

fn fixture_job_path(filename: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("example")
        .join(filename)
}

fn batch_bin_path() -> String {
    std::env::var("CARGO_BIN_EXE_batch").unwrap_or_else(|_| {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("target")
            .join("debug")
            .join("batch")
            .to_string_lossy()
            .into_owned()
    })
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
fn run_in_out_with_example_scripts_collects_outputs() {
    let input_dir = fixture_input_dir(&["00_print.sh", "10_result.sh"], "example/job1/scripts");
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

    let result =
        run_in_out_blocking(&input_dir, &output_dir).expect("integration execution failed");

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
fn run_in_out_with_ollama_prompt_collects_answer() {
    if std::env::var("BATCH_OLLAMA_IT").is_err() {
        eprintln!("Skipping Ollama integration test: set BATCH_OLLAMA_IT=1 to run.");
        return;
    }

    let input_dir = fixture_input_dir(&["20_ollama_prompt.sh"], "../jobs/ollama/scripts");
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

    let options = RunInOutOptions {
        disk_size: Some("20G".to_string()),
        ..Default::default()
    };
    let result = match run_in_out_blocking_with(&input_dir, &output_dir, options) {
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
fn run_batch_cli_with_two_jobs() {
    let job1 = fixture_job_path("job1");
    let job2 = fixture_job_path("job2");
    let batch_bin = batch_bin_path();

    let result = Command::new(batch_bin)
        .arg(job1.as_os_str())
        .arg(job2.as_os_str())
        .output()
        .expect("failed to execute batch binary");

    let io = String::from_utf8_lossy(&result.stdout);
    let err = String::from_utf8_lossy(&result.stderr);
    let combined = format!("{io}{err}");
    let jobs = [
        JobExpectation {
            name: "job1-print-result",
            output_dir: job1.join("output"),
            job_file: job1.clone(),
            required_outputs: vec![
                "00_print.sh.log",
                "10_result.sh.log",
                "hello.txt",
                "result.txt",
            ],
        },
        JobExpectation {
            name: "job2-print-only",
            output_dir: job2.join("output"),
            job_file: job2.clone(),
            required_outputs: vec!["00_print.sh.log", "hello.txt"],
        },
    ];

    if !result.status.success() {
        let failed_job = failed_job_from_output(&combined, &jobs)
            .or_else(|| {
                jobs.iter()
                    .find(|job| !job.output_dir.join("00_print.sh.log").exists())
                    .map(|job| job.name.to_string())
            })
            .unwrap_or_else(|| "unknown job".to_string());
        panic!(
            "batch failed at: {failed_job}\nstatus: {:?}\nstdout: {io}\nstderr: {err}",
            result.status
        );
    }

    assert!(
        combined.contains("Running job: job1-print-result")
            && combined.contains("Running job: job2-print-only")
    );

    jobs.iter().for_each(expect_job_outputs);
}

#[test]
fn run_batch_concurrent_rejects_more_than_four_jobs() {
    let batch_bin = batch_bin_path();
    let job = fixture_job_path("job1");

    let result = Command::new(batch_bin)
        .arg("--concurrent")
        .arg(job.as_os_str())
        .arg(job.as_os_str())
        .arg(job.as_os_str())
        .arg(job.as_os_str())
        .arg(job.as_os_str())
        .output()
        .expect("failed to execute batch binary");

    assert!(
        !result.status.success(),
        "--concurrent with >4 jobs must fail"
    );
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(
        stderr.contains("--concurrent supports up to 4 jobs"),
        "stderr did not contain max-jobs error: {stderr}"
    );
}

#[test]
fn run_batch_no_args_prints_usage_and_exits_nonzero() {
    let batch_bin = batch_bin_path();

    let result = Command::new(batch_bin)
        .output()
        .expect("failed to execute batch binary");

    assert!(
        !result.status.success(),
        "batch with no args must exit non-zero"
    );
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(
        stderr.contains("Usage:"),
        "stderr must contain usage hint: {stderr}"
    );
}

#[test]
fn all_example_shell_scripts_pass_bash_n() {
    let scripts_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("example");
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
