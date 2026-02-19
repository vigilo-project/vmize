use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("Command '{command}' not found in input: {path}")]
    CommandNotFound { command: String, path: PathBuf },

    #[error("Artifact not found after task: {file}")]
    MissingArtifact { file: String },

    #[error("Output path is not valid UTF-8: {path}")]
    NonUtf8Path { path: PathBuf },

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
