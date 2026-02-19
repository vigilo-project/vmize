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

// ═══════════════════════════════════════════════════════════════════════════════
// Unit Tests
// ═══════════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn create_task_dir_with_json(json_content: &str) -> TempDir {
        let temp = TempDir::new().expect("failed to create temp dir");
        let json_path = temp.path().join("task.json");
        fs::write(&json_path, json_content).expect("failed to write task.json");
        temp
    }

    #[test]
    fn load_task_parses_valid_json_with_all_fields() {
        let temp = create_task_dir_with_json(r#"{"name": "test-task", "description": "A test", "disk_size": "10G"}"#);

        let result = load_task(temp.path()).unwrap();

        assert_eq!(result.definition.name, Some("test-task".to_string()));
        assert_eq!(result.definition.description, Some("A test".to_string()));
        assert_eq!(result.definition.disk_size, Some("10G".to_string()));
        assert_eq!(result.input_dir, temp.path().join("scripts"));
        assert_eq!(result.output_dir, temp.path().join("output"));
    }

    #[test]
    fn load_task_parses_valid_json_with_optional_fields() {
        let temp = create_task_dir_with_json(r#"{"name": "minimal"}"#);

        let result = load_task(temp.path()).unwrap();

        assert_eq!(result.definition.name, Some("minimal".to_string()));
        assert!(result.definition.description.is_none());
        assert!(result.definition.disk_size.is_none());
    }

    #[test]
    fn load_task_parses_empty_json_object() {
        let temp = create_task_dir_with_json(r#"{}"#);

        let result = load_task(temp.path()).unwrap();

        assert!(result.definition.name.is_none());
        assert!(result.definition.description.is_none());
        assert!(result.definition.disk_size.is_none());
    }

    #[test]
    fn load_task_fails_for_missing_file() {
        let temp = TempDir::new().expect("failed to create temp dir");

        let result = load_task(temp.path());

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("Cannot read"));
        assert!(err.contains("task.json"));
    }

    #[test]
    fn load_task_fails_for_invalid_json() {
        let temp = create_task_dir_with_json(r#"not valid json"#);

        let result = load_task(temp.path());

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("Invalid JSON"));
    }

    #[test]
    fn load_task_fails_for_json_array_instead_of_object() {
        let temp = create_task_dir_with_json(r#"[1, 2, 3]"#);

        let result = load_task(temp.path());

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("Invalid JSON"));
    }

    #[test]
    fn load_task_creates_output_directory() {
        let temp = create_task_dir_with_json(r#"{"name": "test"}"#);
        let output_dir = temp.path().join("output");

        assert!(!output_dir.exists());

        let result = load_task(temp.path());

        assert!(result.is_ok());
        assert!(output_dir.exists());
        assert!(output_dir.is_dir());
    }

    #[test]
    fn load_task_output_directory_already_exists() {
        let temp = create_task_dir_with_json(r#"{"name": "test"}"#);
        let output_dir = temp.path().join("output");
        fs::create_dir(&output_dir).unwrap();
        fs::write(output_dir.join("existing.txt"), "data").unwrap();

        let result = load_task(temp.path());

        assert!(result.is_ok());
        // Existing files should not be deleted
        assert!(output_dir.join("existing.txt").exists());
    }

    #[test]
    fn load_task_sets_input_dir_to_scripts_subdirectory() {
        let temp = create_task_dir_with_json(r#"{"name": "test"}"#);
        let scripts_dir = temp.path().join("scripts");
        fs::create_dir(&scripts_dir).unwrap();
        fs::write(scripts_dir.join("script.sh"), "#!/bin/bash").unwrap();

        let result = load_task(temp.path()).unwrap();

        assert_eq!(result.input_dir, scripts_dir);
    }

    #[test]
    fn load_task_handles_unicode_in_name() {
        let temp = create_task_dir_with_json(r#"{"name": "테스트-タスク-🔥", "description": "한글 설명"}"#);

        let result = load_task(temp.path()).unwrap();

        assert_eq!(result.definition.name, Some("테스트-タスク-🔥".to_string()));
        assert_eq!(result.definition.description, Some("한글 설명".to_string()));
    }

    #[test]
    fn load_task_handles_multiline_description() {
        let temp = create_task_dir_with_json(
            r#"{"name": "test", "description": "Line 1\nLine 2\nLine 3"}"#,
        );

        let result = load_task(temp.path()).unwrap();

        assert_eq!(
            result.definition.description,
            Some("Line 1\nLine 2\nLine 3".to_string())
        );
    }
}
