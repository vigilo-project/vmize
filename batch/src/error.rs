use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("Input path does not exist: {path}")]
    InputPathNotFound { path: PathBuf },

    #[error("Input path is not a directory: {path}")]
    InputPathNotDirectory { path: PathBuf },

    #[error("Output path is not valid UTF-8: {path}")]
    NonUtf8Path { path: PathBuf },

    #[error("Failed to discover scripts in {path}: {source}")]
    ScriptDiscovery {
        path: PathBuf,
        source: std::io::Error,
    },

    #[error("No files were found in {0}")]
    NoScripts(PathBuf),

    #[error("Failed to start VM: {message}")]
    VmStart { message: String },

    #[error("Failed to copy files to VM: {message}")]
    CopyToVm { message: String },

    #[error("Failed to run script '{script}': {message}")]
    ScriptFailed { script: String, message: String },

    #[error("Failed to collect VM output: {message}")]
    CopyFromVm { message: String },

    #[error("Failed to execute command in VM: {message}")]
    VmCommand { message: String },

    #[error("Failed to cleanup VM '{vm_id}': {message}")]
    CleanupFailed { vm_id: String, message: String },

    #[error("Blocking mode is not allowed from within an async runtime")]
    BlockingInAsyncContext,

    #[error("Runtime error: {message}")]
    Runtime { message: String },

    #[error(transparent)]
    Io(#[from] std::io::Error),
}
