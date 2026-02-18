use std::path::PathBuf;
use std::process;
use std::thread;

use clap::Parser;

use batch::job::load_job;
use batch::{run_in_out_blocking_with, Error, RunInOutOptions};

const MAX_CONCURRENT_JOBS: usize = 4;

#[derive(Debug, Parser)]
#[command(name = "batch")]
#[command(about = "Run one or more VM jobs described by a job directory")]
struct Cli {
    #[arg(long, help = "Run all jobs concurrently (max 4)")]
    concurrent: bool,

    #[arg(value_name = "JOB_DIR")]
    jobs: Vec<PathBuf>,
}

fn main() {
    let cli = Cli::parse();

    if cli.jobs.is_empty() {
        eprintln!("Usage: batch <job-dir> [job-dir ...]");
        process::exit(1);
    }

    if cli.concurrent {
        run_concurrent(&cli.jobs);
        return;
    }

    run_sequential(&cli.jobs);
}

fn run_sequential(jobs: &[PathBuf]) {
    for (idx, job_path) in jobs.iter().enumerate() {
        let (job, input, output) = match load_job(job_path) {
            Ok(t) => t,
            Err(err) => {
                eprintln!("Failed to load job {}: {err}", job_path.display());
                process::exit(1);
            }
        };

        let job_name = job
            .name
            .clone()
            .unwrap_or_else(|| format!("job-{}", idx + 1));
        if let Some(desc) = &job.description {
            eprintln!("Running job: {job_name} — {desc}");
        } else {
            eprintln!("Running job: {job_name} ({})", job_path.display());
        }

        let options = RunInOutOptions {
            disk_size: job.disk_size,
            ..Default::default()
        };
        if let Err(err) = run_in_out_blocking_with(&input, &output, options) {
            eprintln!("{}", format_job_error(&job_name, err));
            process::exit(1);
        }
    }
}

fn run_concurrent(jobs: &[PathBuf]) {
    if jobs.len() > MAX_CONCURRENT_JOBS {
        eprintln!(
            "--concurrent supports up to {MAX_CONCURRENT_JOBS} jobs, but {} were provided",
            jobs.len()
        );
        process::exit(1);
    }

    let handles: Vec<_> = jobs
        .iter()
        .cloned()
        .enumerate()
        .map(|(idx, job_path)| {
            thread::Builder::new()
                .name(format!("batch-job-{idx}"))
                .spawn(move || -> Result<(), String> {
                    let (job, input, output) = load_job(&job_path)?;
                    let name = job
                        .name
                        .unwrap_or_else(|| job_path.display().to_string());
                    eprintln!("[start] {name}");
                    run_in_out_blocking_with(
                        &input,
                        &output,
                        RunInOutOptions {
                            disk_size: job.disk_size,
                            ..Default::default()
                        },
                    )
                    .map_err(|e| format_job_error(&name, e))?;
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

fn format_job_error(job_name: &str, err: Error) -> String {
    format!("Failed to execute {job_name}: {err}")
}
