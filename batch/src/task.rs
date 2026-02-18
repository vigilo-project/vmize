use std::path::{Path, PathBuf};

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct TaskDefinition {
    pub name: Option<String>,
    pub description: Option<String>,
    #[serde(default)]
    pub disk_size: Option<String>,
}

pub fn load_task(task_dir: &Path) -> Result<(TaskDefinition, PathBuf, PathBuf), String> {
    let json_path = task_dir.join("task.json");
    let contents = std::fs::read_to_string(&json_path)
        .map_err(|err| format!("Cannot read {}: {err}", json_path.display()))?;
    let def: TaskDefinition = serde_json::from_str(&contents)
        .map_err(|err| format!("Invalid JSON in {}: {err}", json_path.display()))?;
    let input = task_dir.join("scripts");
    let output = task_dir.join("output");
    std::fs::create_dir_all(&output)
        .map_err(|err| format!("Cannot create output dir {}: {err}", output.display()))?;
    Ok((def, input, output))
}
