mod error;
mod result;
mod runner;
mod vm_ops;

pub const MAX_BATCH_TASKS: usize = 4;

pub use error::Error;
pub use result::{ChainRunResult, ChainStepResult, RunResult};
pub use runner::{
    RunPhase, RunProgress, TaskRunOptions, run_loaded_task, run_loaded_task_blocking,
    run_loaded_task_blocking_with_progress, run_loaded_task_with_progress, run_task_chain_blocking,
};
pub use vm_ops::{RealVmOps, VmOps, VmOptions};

#[cfg(test)]
pub use vm_ops::MockVmOps;
