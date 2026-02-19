use std::path::PathBuf;
use std::process;
use std::thread;

use clap::{Parser, Subcommand};
use task::load_task;

use worker::{MAX_BATCH_TASKS, TaskRunOptions, run_loaded_task_blocking};

/// VMize CLI — run VM tasks and manage the dashboard
#[derive(Debug, Parser)]
#[command(name = "vmize", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Run one or more VM tasks described by a task directory
    Task {
        /// Run all tasks in batch mode (max 4)
        #[arg(long)]
        batch: bool,

        /// Task directories containing task.json + input/
        #[arg(value_name = "TASK_DIR")]
        tasks: Vec<PathBuf>,
    },

    /// Start the web dashboard
    Dashboard {
        /// Port to listen on
        #[arg(long, default_value = "8080")]
        port: u16,
    },
}

fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::Task { batch, tasks } => {
            if tasks.is_empty() {
                eprintln!("Usage: vmize task <task-dir> [task-dir ...]");
                process::exit(1);
            }

            if batch {
                run_batch(&tasks);
            } else {
                run_sequential(&tasks);
            }
        }
        Commands::Dashboard { port } => {
            let rt = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
            rt.block_on(dashboard::start(port));
        }
    }
}

fn run_sequential(tasks: &[PathBuf]) {
    for (idx, task_path) in tasks.iter().enumerate() {
        let loaded = match load_task(task_path) {
            Ok(t) => t,
            Err(err) => {
                eprintln!("Failed to load task {}: {err}", task_path.display());
                process::exit(1);
            }
        };

        let task_name = loaded
            .definition
            .name
            .clone()
            .unwrap_or_else(|| format!("task-{}", idx + 1));
        if let Some(desc) = &loaded.definition.description {
            eprintln!("Running task: {task_name} — {desc}");
        } else {
            eprintln!("Running task: {task_name} ({})", task_path.display());
        }

        let options = TaskRunOptions {
            disk_size: loaded.definition.disk_size.clone(),
            ..Default::default()
        };
        if let Err(err) = run_loaded_task_blocking(&loaded, options) {
            eprintln!("{}", format_task_error(&task_name, err));
            process::exit(1);
        }
    }
}

fn run_batch(tasks: &[PathBuf]) {
    if tasks.len() > MAX_BATCH_TASKS {
        eprintln!(
            "--batch supports up to {MAX_BATCH_TASKS} tasks, but {} were provided",
            tasks.len()
        );
        process::exit(1);
    }

    let mut failed = 0usize;
    let mut handles = Vec::with_capacity(tasks.len());

    for (idx, task_path) in tasks.iter().cloned().enumerate() {
        let task_path_for_thread = task_path.clone();
        let thread_name = format!("vmize-task-{idx}");
        let handle = thread::Builder::new().name(thread_name).spawn(move || -> Result<(), String> {
            let loaded = load_task(&task_path_for_thread).map_err(|err| {
                format!(
                    "failed to load task {}: {err}",
                    task_path_for_thread.display()
                )
            })?;
            let name = loaded
                .definition
                .name
                .clone()
                .unwrap_or_else(|| task_path_for_thread.display().to_string());
            eprintln!("[start] {name}");
            run_loaded_task_blocking(
                &loaded,
                TaskRunOptions {
                    disk_size: loaded.definition.disk_size.clone(),
                    ..Default::default()
                },
            )
            .map_err(|e| format_task_error(&name, e))?;
            eprintln!("[done]  {name} -> {}", loaded.output_dir.display());
            Ok(())
        });

        match handle {
            Ok(handle) => handles.push(handle),
            Err(err) => {
                failed += 1;
                eprintln!("[error] failed to spawn worker thread for {}: {err}", task_path.display());
            }
        }
    }

    let failed = failed
        + handles
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

fn format_task_error(task_name: &str, err: worker::Error) -> String {
    format!("Failed to execute {task_name}: {err}")
}
