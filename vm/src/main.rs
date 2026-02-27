#![deny(warnings)]

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
use vm::{RunOptions, cp, ps, rm, run, ssh, ssh_stream};

/// VM CLI - Ubuntu Cloud Image VM Automation
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Run a new VM
    Run {
        /// VM username
        #[arg(long, default_value = "ubuntu")]
        username: String,

        /// Memory allocation (e.g., 2G, 4G)
        #[arg(long, default_value = "4G")]
        memory: String,

        /// Number of CPUs
        #[arg(long, default_value_t = 2)]
        cpus: u32,

        /// Optional virtual disk size (e.g. 20G, +20G)
        #[arg(long)]
        disk_size: Option<String>,

        /// Don't skip image download
        #[arg(long, default_value = "false")]
        force_download: bool,

        /// Custom image URL
        #[arg(long)]
        image_url: Option<String>,

        /// Custom kernel image path (direct kernel boot)
        #[arg(long)]
        kernel: Option<PathBuf>,

        /// Rootfs disk image for custom kernel boot
        #[arg(long)]
        rootfs: Option<PathBuf>,

        /// Show QEMU verbose output
        #[arg(long, default_value = "false")]
        verbose: bool,
    },

    /// Connect to a VM by ID
    Ssh {
        /// VM identifier
        id: String,

        /// Command to execute
        command: Option<String>,

        /// Stream command output
        #[arg(long, default_value = "false")]
        stream: bool,
    },

    /// List running VMs
    Ps,

    /// Remove a VM (or all VMs with --all)
    Rm {
        /// VM identifier
        id: Option<String>,

        /// Remove all VMs
        #[arg(long)]
        all: bool,
    },

    /// Copy files between local and VM using scp-style paths
    /// (`<vm-id>:<path>` for remote side).
    Cp {
        /// Source path (`<vm-id>:<path>` for remote)
        src: String,
        /// Destination path (`<vm-id>:<path>` for remote)
        dest: String,
        /// Recursive copy (directories)
        #[arg(short, long)]
        recursive: bool,
    },

    /// Print version information
    Version,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    let default_filter = match &cli.command {
        Commands::Run { verbose: false, .. } => "warn",
        _ => "info",
    };
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| default_filter.into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    match cli.command {
        Commands::Run {
            username,
            memory,
            cpus,
            disk_size,
            force_download,
            image_url,
            kernel,
            rootfs,
            verbose,
        } => {
            let options = RunOptions {
                username: Some(username),
                memory: Some(memory),
                cpus: Some(cpus),
                disk_size,
                force_download,
                image_url,
                kernel,
                rootfs,
                verbose,
                ..Default::default()
            };
            let record = run(options).await?;
            if verbose {
                println!("{}", record.id);
            } else {
                eprintln!();
                println!("vm ssh {}", record.id);
            }
        }
        Commands::Ssh {
            id,
            command,
            stream,
        } => match (command, stream) {
            (Some(cmd), true) => {
                ssh_stream(&id, &cmd)?;
            }
            (Some(cmd), false) => {
                let output = ssh(&id, Some(&cmd)).await?;
                println!("{}", output);
            }
            (None, _) => {
                ssh(&id, None).await?;
            }
        },
        Commands::Ps => {
            let output = ps()?;
            print!("{}", output);
        }
        Commands::Rm { id, all } => {
            if all {
                rm(None)?;
            } else {
                let id = id.context("VM id is required (or use --all to remove all VMs)")?;
                rm(Some(&id))?;
            }
        }
        Commands::Cp {
            src,
            dest,
            recursive,
        } => {
            cp(&src, &dest, recursive)?;
        }
        Commands::Version => {
            println!("vm {}", env!("CARGO_PKG_VERSION"));
        }
    }

    Ok(())
}
