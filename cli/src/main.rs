use std::path::PathBuf;
use std::process;
use std::thread;

use clap::{Parser, Subcommand};

use worker::{MAX_BATCH_TASKS, TaskRunOptions, run_task_chain_blocking};

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
    for task_path in tasks {
        eprintln!("[start] {}", task_path.display());

        let chain = match run_task_chain_blocking(task_path, TaskRunOptions::default()) {
            Ok(result) => result,
            Err(err) => {
                eprintln!(
                    "{}",
                    format_task_error(&task_path.display().to_string(), err)
                );
                process::exit(1);
            }
        };

        print_chain_steps(&chain);

        if let Some(last_step) = chain.steps.last() {
            eprintln!(
                "[done]  {} -> {}",
                task_path.display(),
                last_step.run_result.output_dir.display()
            );
        } else {
            eprintln!("[done]  {}", task_path.display());
        }
    }
}

fn print_chain_steps(chain: &worker::ChainRunResult) {
    for (idx, step) in chain.steps.iter().enumerate() {
        let name = step
            .task_name
            .clone()
            .unwrap_or_else(|| step.task_dir.display().to_string());
        eprintln!("  step {}: {} ({})", idx + 1, name, step.task_dir.display());
        if !step.handoff_artifacts.is_empty() {
            eprintln!(
                "    handoff artifacts: {}",
                step.handoff_artifacts.join(", ")
            );
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
        let handle =
            thread::Builder::new()
                .name(thread_name)
                .spawn(move || -> Result<(), String> {
                    let name = task_path_for_thread.display().to_string();
                    eprintln!("[start] {name}");
                    let chain =
                        run_task_chain_blocking(&task_path_for_thread, TaskRunOptions::default())
                            .map_err(|e| format_task_error(&name, e))?;
                    print_chain_steps(&chain);
                    if let Some(last_step) = chain.steps.last() {
                        eprintln!(
                            "[done]  {name} -> {}",
                            last_step.run_result.output_dir.display()
                        );
                    } else {
                        eprintln!("[done]  {name}");
                    }
                    Ok(())
                });

        match handle {
            Ok(handle) => handles.push(handle),
            Err(err) => {
                failed += 1;
                eprintln!(
                    "[error] failed to spawn worker thread for {}: {err}",
                    task_path.display()
                );
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
