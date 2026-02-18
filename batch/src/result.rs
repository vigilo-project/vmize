use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct RunResult {
    pub vm_id: String,
    pub output_dir: PathBuf,
    pub executed_scripts: Vec<String>,
    pub exit_code: i32,
    pub elapsed_ms: u64,
}

impl RunResult {
    pub fn new(vm_id: impl Into<String>, output_dir: impl Into<PathBuf>) -> Self {
        Self {
            vm_id: vm_id.into(),
            output_dir: output_dir.into(),
            executed_scripts: Vec::new(),
            exit_code: 0,
            elapsed_ms: 0,
        }
    }
}
