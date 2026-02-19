mod error;
mod result;
mod runner;
mod task;
mod vm_ops;

pub const MAX_CONCURRENT_TASKS: usize = 4;

pub use error::Error;
pub use result::RunResult;
pub use runner::{
    RunPhase, RunProgress, TaskRunOptions, run_loaded_task, run_loaded_task_blocking,
    run_loaded_task_blocking_with_progress, run_loaded_task_with_progress,
};
pub use task::{LoadedTask, TaskDefinition, load_task};
pub use vm_ops::{RealVmOps, VmOps, VmOptions};

#[cfg(test)]
pub use vm_ops::MockVmOps;
