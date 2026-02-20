use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct RunResult {
    pub vm_id: String,
    pub output_dir: PathBuf,
    pub logs_dir: PathBuf,
    pub executed_commands: Vec<String>,
    pub collected_artifacts: Vec<String>,
    pub exit_code: i32,
    pub elapsed_ms: u64,
}

impl RunResult {
    pub fn new(
        vm_id: impl Into<String>,
        output_dir: impl Into<PathBuf>,
        logs_dir: impl Into<PathBuf>,
    ) -> Self {
        Self {
            vm_id: vm_id.into(),
            output_dir: output_dir.into(),
            logs_dir: logs_dir.into(),
            executed_commands: Vec::new(),
            collected_artifacts: Vec::new(),
            exit_code: 0,
            elapsed_ms: 0,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ChainStepResult {
    pub task_dir: PathBuf,
    pub task_name: Option<String>,
    pub handoff_artifacts: Vec<String>,
    pub run_result: RunResult,
}

#[derive(Debug, Clone, Default)]
pub struct ChainRunResult {
    pub steps: Vec<ChainStepResult>,
}
