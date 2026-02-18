use std::path::PathBuf;
use std::process;
use std::thread;

use clap::Parser;

use batch::task::load_task;
use batch::{run_in_out_blocking_with, Error, RunInOutOptions};

const MAX_CONCURRENT_TASKS: usize = 4;

#[derive(Debug, Parser)]
#[command(name = "batch")]
#[command(about = "Run one or more VM tasks described by a task directory")]
struct Cli {
    #[arg(long, help = "Run all tasks concurrently (max 4)")]
    concurrent: bool,

    #[arg(value_name = "TASK_DIR")]
    tasks: Vec<PathBuf>,
}

fn main() {
    let cli = Cli::parse();

    if cli.tasks.is_empty() {
        eprintln!("Usage: batch <task-dir> [task-dir ...]");
        process::exit(1);
    }

    if cli.concurrent {
        run_concurrent(&cli.tasks);
        return;
    }

    run_sequential(&cli.tasks);
}

fn run_sequential(tasks: &[PathBuf]) {
    for (idx, task_path) in tasks.iter().enumerate() {
        let (task, input, output) = match load_task(task_path) {
            Ok(t) => t,
            Err(err) => {
                eprintln!("Failed to load task {}: {err}", task_path.display());
                process::exit(1);
            }
        };

        let task_name = task
            .name
            .clone()
            .unwrap_or_else(|| format!("task-{}", idx + 1));
        if let Some(desc) = &task.description {
            eprintln!("Running task: {task_name} — {desc}");
        } else {
            eprintln!("Running task: {task_name} ({})", task_path.display());
        }

        let options = RunInOutOptions {
            disk_size: task.disk_size,
            ..Default::default()
        };
        if let Err(err) = run_in_out_blocking_with(&input, &output, options) {
            eprintln!("{}", format_task_error(&task_name, err));
            process::exit(1);
        }
    }
}

fn run_concurrent(tasks: &[PathBuf]) {
    if tasks.len() > MAX_CONCURRENT_TASKS {
        eprintln!(
            "--concurrent supports up to {MAX_CONCURRENT_TASKS} tasks, but {} were provided",
            tasks.len()
        );
        process::exit(1);
    }

    let handles: Vec<_> = tasks
        .iter()
        .cloned()
        .enumerate()
        .map(|(idx, task_path)| {
            thread::Builder::new()
                .name(format!("batch-task-{idx}"))
                .spawn(move || -> Result<(), String> {
                    let (task, input, output) = load_task(&task_path)?;
                    let name = task.name.unwrap_or_else(|| task_path.display().to_string());
                    eprintln!("[start] {name}");
                    run_in_out_blocking_with(
                        &input,
                        &output,
                        RunInOutOptions {
                            disk_size: task.disk_size,
                            ..Default::default()
                        },
                    )
                    .map_err(|e| format_task_error(&name, e))?;
                    eprintln!("[done]  {name} -> {}", output.display());
                    Ok(())
                })
                .expect("failed to spawn worker thread")
        })
        .collect();

    let failed = handles
        .into_iter()
        .filter_map(|h| {
            h.join()
                .unwrap_or_else(|_| Err("thread panicked".into()))
                .err()
        })
        .inspect(|e| eprintln!("{e}"))
        .count();

    if failed > 0 {
        process::exit(1);
    }
}

fn format_task_error(task_name: &str, err: Error) -> String {
    format!("Failed to execute {task_name}: {err}")
}
