use crate::config::Config;
use crate::ops;
use anyhow::Result;

pub use crate::ops::{ProgressCallback, RunOptions};
pub use crate::vm::VmRecord;

pub async fn run(options: RunOptions) -> Result<VmRecord> {
    ops::run(options).await
}

pub async fn ssh(id: &str, command: Option<&str>) -> Result<String> {
    let config = Config::default();
    match command {
        Some(cmd) => ops::ssh_with_config(&config, id, cmd).await,
        None => {
            ops::run_interactive_ssh(&config, id)?;
            Ok(String::new())
        }
    }
}

pub fn ssh_stream(id: &str, command: &str) -> Result<()> {
    let config = Config::default();
    ops::ssh_stream_with_config(&config, id, command)
}

pub fn cp(src: &str, dest: &str, recursive: bool) -> Result<()> {
    let config = Config::default();
    ops::cp_transfer(&config, src, dest, recursive)
}

pub fn cp_to(id: &str, local: &str, remote: &str, recursive: bool) -> Result<()> {
    let config = Config::default();
    ops::cp_to_with_config(&config, id, local, remote, recursive)
}

pub fn cp_from(id: &str, remote: &str, local: &str, recursive: bool) -> Result<()> {
    let config = Config::default();
    ops::cp_from_with_config(&config, id, remote, local, recursive)
}

pub fn rm(id: Option<&str>) -> Result<()> {
    let config = Config::default();
    match id {
        Some(id) => ops::rm_with_config(&config, id),
        None => ops::clear_vms(&config),
    }
}

pub fn ps() -> Result<String> {
    let config = Config::default();
    ops::list_vms_inner(&config)
}
