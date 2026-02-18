use std::path::{Path, PathBuf};

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct JobDefinition {
    pub name: Option<String>,
    pub description: Option<String>,
    #[serde(default)]
    pub disk_size: Option<String>,
}

pub fn load_job(job_dir: &Path) -> Result<(JobDefinition, PathBuf, PathBuf), String> {
    let json_path = job_dir.join("job.json");
    let contents = std::fs::read_to_string(&json_path)
        .map_err(|err| format!("Cannot read {}: {err}", json_path.display()))?;
    let def: JobDefinition = serde_json::from_str(&contents)
        .map_err(|err| format!("Invalid JSON in {}: {err}", json_path.display()))?;
    let input = job_dir.join("scripts");
    let output = job_dir.join("output");
    std::fs::create_dir_all(&output)
        .map_err(|err| format!("Cannot create output dir {}: {err}", output.display()))?;
    Ok((def, input, output))
}
