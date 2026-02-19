mod error;
mod result;
mod runner;
mod task;
mod vm_ops;

pub const MAX_CONCURRENT_TASKS: usize = 4;

pub use error::Error;
pub use result::RunResult;
pub use runner::{
    run_task, run_task_blocking, run_task_blocking_with_options, run_task_blocking_with_progress,
    run_task_with_options, run_task_with_progress, RunPhase, RunProgress, TaskRunOptions,
};
pub use task::{load_task, LoadedTask, TaskDefinition};
pub use vm_ops::{RealVmOps, VmOps, VmOptions};

#[cfg(test)]
pub use vm_ops::MockVmOps;
