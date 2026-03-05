#![deny(warnings)]

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
use vm::{MountSpec, RunOptions, cp, ps, rm, run, ssh, ssh_stream};

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

        /// Mount host path into VM: <host_path>:<guest_path>[:ro|rw]
        #[arg(long = "mount", value_parser = parse_mount_spec_arg)]
        mounts: Vec<MountSpec>,
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
            mounts,
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
                mounts,
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

fn parse_mount_spec_arg(value: &str) -> std::result::Result<MountSpec, String> {
    vm::parse_mount_spec(value).map_err(|err| err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_parses_single_mount_argument() {
        let cli = Cli::parse_from(["vm", "run", "--mount", "/tmp:/mnt/host"]);
        match cli.command {
            Commands::Run { mounts, .. } => {
                assert_eq!(mounts.len(), 1);
                assert_eq!(mounts[0].host_path.to_string_lossy(), "/tmp");
                assert_eq!(mounts[0].guest_path.to_string_lossy(), "/mnt/host");
            }
            _ => panic!("expected run command"),
        }
    }

    #[test]
    fn cli_parses_multiple_mount_arguments() {
        let cli = Cli::parse_from([
            "vm",
            "run",
            "--mount",
            "/tmp:/mnt/one",
            "--mount",
            "/var/tmp:/mnt/two:rw",
        ]);
        match cli.command {
            Commands::Run { mounts, .. } => {
                assert_eq!(mounts.len(), 2);
                assert_eq!(mounts[0].guest_path.to_string_lossy(), "/mnt/one");
                assert_eq!(mounts[1].guest_path.to_string_lossy(), "/mnt/two");
            }
            _ => panic!("expected run command"),
        }
    }
}
