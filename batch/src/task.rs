use std::path::{Path, PathBuf};

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct TaskDefinition {
    pub name: Option<String>,
    pub description: Option<String>,
    #[serde(default)]
    pub disk_size: Option<String>,
}

#[derive(Debug, Clone)]
pub struct LoadedTask {
    pub definition: TaskDefinition,
    pub input_dir: PathBuf,
    pub output_dir: PathBuf,
}

pub fn load_task(task_dir: &Path) -> Result<LoadedTask, String> {
    let json_path = task_dir.join("task.json");
    let contents = std::fs::read_to_string(&json_path)
        .map_err(|err| format!("Cannot read {}: {err}", json_path.display()))?;
    let definition: TaskDefinition = serde_json::from_str(&contents)
        .map_err(|err| format!("Invalid JSON in {}: {err}", json_path.display()))?;
    let input_dir = task_dir.join("scripts");
    let output_dir = task_dir.join("output");
    std::fs::create_dir_all(&output_dir)
        .map_err(|err| format!("Cannot create output dir {}: {err}", output_dir.display()))?;
    Ok(LoadedTask {
        definition,
        input_dir,
        output_dir,
    })
}
