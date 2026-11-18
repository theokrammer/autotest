use std::fs;
use std::path::{Path, PathBuf};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum StagingError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Staging verification failed: {0}")]
    VerificationFailed(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StagingEntry {
    pub target: String,
    pub scenario: String,
    pub recorded_at: String,
    pub mode: String,
    pub expected: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StagingDocument {
    pub entries: Vec<StagingEntry>,
    pub source_file: String,
}

/// Manages .autotest.staging.json lifecycle:
/// 1. Write staged outcomes from --record runs
/// 2. Verify staged outcomes by re-running in sandbox
/// 3. Merge verified staging back into the main .autotest.json
pub struct StagingManager;

impl StagingManager {
    /// Find all .autotest.staging.json files under a directory.
    pub fn find_staging_files(dir: &Path) -> Result<Vec<PathBuf>, StagingError> {
        let mut staging_files = Vec::new();
        if !dir.exists() {
            return Ok(staging_files);
        }
        for entry in walkdir::WalkDir::new(dir)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            let path = entry.path();
            if path.is_file() {
                let name = path.file_name().unwrap_or_default().to_string_lossy();
                if name.ends_with(".autotest.staging.json") || name.ends_with(".staging.json") {
                    staging_files.push(path.to_path_buf());
                }
            }
        }
        Ok(staging_files)
    }

    /// Write staging entries to a .autotest.staging.json file.
    pub fn write_staging(
        staging_path: &Path,
        entries: &[StagingEntry],
    ) -> Result<(), StagingError> {
        let content = serde_json::to_string_pretty(entries)?;
        if let Some(parent) = staging_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(staging_path, content)?;
        Ok(())
    }

    /// Read staging entries from a .autotest.staging.json file.
    pub fn read_staging(staging_path: &Path) -> Result<Vec<StagingEntry>, StagingError> {
        let content = fs::read_to_string(staging_path)?;
        let entries: Vec<StagingEntry> = serde_json::from_str(&content)?;
        Ok(entries)
    }

    /// Merge verified staging entries back into the main .autotest.json.
    /// Returns the number of scenarios merged.
    pub fn merge_verified_staging(staging_path: &Path) -> Result<usize, StagingError> {
        let parent = staging_path.parent().unwrap_or(Path::new("."));
        let filename = staging_path.file_name().unwrap_or_default().to_string_lossy();
        let main_filename = if filename.ends_with(".staging.json") {
            format!("{}{}", &filename[..filename.len() - ".staging.json".len()], ".json")
        } else {
            filename.replace(".staging.json", ".json")
        };
        let main_path = parent.join(main_filename);

        if !main_path.exists() {
            return Err(StagingError::VerificationFailed(format!(
                "Main scenario file not found: {}",
                main_path.display()
            )));
        }

        let entries = Self::read_staging(staging_path)?;
        let content = fs::read_to_string(&main_path)?;
        let mut doc: serde_json::Value = serde_json::from_str(&content)?;

        let mut merged_count = 0;

        if let Some(targets) = doc.get_mut("targets").and_then(|t| t.as_array_mut()) {
            for entry in &entries {
                if let Some(target) = targets.iter_mut().find(|t| {
                    t.get("id").and_then(|id| id.as_str()) == Some(&entry.target)
                }) {
                    let scenarios = target
                        .get_mut("scenarios")
                        .and_then(|s| s.as_array_mut());

                    if let Some(scenarios) = scenarios {
                        let mut expected_val = entry.expected.clone();
                        if let Some(obj) = expected_val.as_object_mut() {
                            if !obj.contains_key("outcome") {
                                if obj.contains_key("exception") || obj.contains_key("exception_type") {
                                    obj.insert("outcome".to_string(), serde_json::Value::String("exception".to_string()));
                                } else {
                                    obj.insert("outcome".to_string(), serde_json::Value::String("return".to_string()));
                                }
                            }
                        }

                        // Update existing or append new
                        if let Some(existing) = scenarios.iter_mut().find(|s| {
                            s.get("name").and_then(|n| n.as_str()) == Some(&entry.scenario)
                        }) {
                            existing["expected"] = expected_val;
                            existing["mode"] = serde_json::Value::String("recorded".to_string());
                            if let Some(obj) = existing.as_object_mut() {
                                obj.remove("stale");
                            }
                        } else {
                            let mut new_scenario = serde_json::Map::new();
                            new_scenario.insert("name".to_string(), serde_json::Value::String(entry.scenario.clone()));
                            new_scenario.insert("mode".to_string(), serde_json::Value::String("recorded".to_string()));
                            new_scenario.insert("locked".to_string(), serde_json::Value::Bool(false));
                            new_scenario.insert("stale".to_string(), serde_json::Value::Bool(false));
                            new_scenario.insert("inputs".to_string(), serde_json::Value::Array(vec![]));
                            new_scenario.insert("expected".to_string(), expected_val);
                            scenarios.push(serde_json::Value::Object(new_scenario));
                        }
                        merged_count += 1;
                    }
                }
            }
        }

        let output = serde_json::to_string_pretty(&doc)?;
        fs::write(&main_path, output)?;
        fs::remove_file(staging_path)?;

        Ok(merged_count)
    }

    /// Merge all staging files under a directory.
    /// Returns (files_merged, scenarios_merged).
    pub fn merge_all_staging(dir: &Path) -> Result<(usize, usize), StagingError> {
        let staging_files = Self::find_staging_files(dir)?;
        let mut total_files = 0;
        let mut total_scenarios = 0;
        for sf in staging_files {
            let count = Self::merge_verified_staging(&sf)?;
            total_files += 1;
            total_scenarios += count;
        }
        Ok((total_files, total_scenarios))
    }

    /// Compute the staging path for a given .autotest.json path.
    pub fn staging_path_for(autotest_path: &Path) -> PathBuf {
        let stem = autotest_path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .replace(".autotest.json", "");
        let dir = autotest_path.parent().unwrap_or(Path::new("."));
        dir.join(format!("{}.autotest.staging.json", stem))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_staging_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let staging_path = tmp.path().join("test.autotest.staging.json");

        let entries = vec![
            StagingEntry {
                target: "Calc.add".to_string(),
                scenario: "basic_add".to_string(),
                recorded_at: "2024-01-01T00:00:00Z".to_string(),
                mode: "recorded".to_string(),
                expected: serde_json::json!({"value": 3}),
            },
        ];

        StagingManager::write_staging(&staging_path, &entries).unwrap();
        let read_back = StagingManager::read_staging(&staging_path).unwrap();
        assert_eq!(read_back.len(), 1);
        assert_eq!(read_back[0].target, "Calc.add");
        assert_eq!(read_back[0].expected, serde_json::json!({"value": 3}));
    }

    #[test]
    fn test_merge_verified_staging() {
        let tmp = tempfile::tempdir().unwrap();
        let main_path = tmp.path().join("calc.autotest.json");
        let staging_path = tmp.path().join("calc.autotest.staging.json");

        let main_doc = serde_json::json!({
            "schema_version": "1.0",
            "source": {"file": "calc.ts", "content_hash": "sha256:abc", "analyzer": "test", "dependencies": {}},
            "fixtures": {},
            "targets": [{
                "id": "Calc.add",
                "kind": "method",
                "signature": {"constructor_args": [], "parameters": [], "return_type": "number"},
                "detected_side_effects": [],
                "scenarios": [
                    {"name": "basic_add", "mode": "symbolic_contract", "inputs": [1, 2], "expected": {"value": 3}}
                ]
            }]
        });
        fs::write(&main_path, serde_json::to_string_pretty(&main_doc).unwrap()).unwrap();

        let entries = vec![StagingEntry {
            target: "Calc.add".to_string(),
            scenario: "recorded_sum".to_string(),
            recorded_at: "2024-01-01T00:00:00Z".to_string(),
            mode: "recorded".to_string(),
            expected: serde_json::json!({"value": 7}),
        }];
        StagingManager::write_staging(&staging_path, &entries).unwrap();

        let merged = StagingManager::merge_verified_staging(&staging_path).unwrap();
        assert_eq!(merged, 1);
        assert!(!staging_path.exists()); // Staging file removed after merge

        let final_doc: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&main_path).unwrap()).unwrap();
        let scenarios = final_doc["targets"][0]["scenarios"].as_array().unwrap();
        assert_eq!(scenarios.len(), 2);
        let recorded = scenarios.iter().find(|s| s["name"] == "recorded_sum").unwrap();
        assert_eq!(recorded["mode"], "recorded");
        assert_eq!(recorded["expected"]["value"], 7);
    }

    #[test]
    fn test_find_staging_files() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("a.autotest.staging.json"), "[]").unwrap();
        fs::write(tmp.path().join("b.autotest.json"), "{}").unwrap();
        fs::create_dir_all(tmp.path().join("sub")).unwrap();
        fs::write(tmp.path().join("sub").join("c.autotest.staging.json"), "[]").unwrap();

        let files = StagingManager::find_staging_files(tmp.path()).unwrap();
        assert_eq!(files.len(), 2);
    }

    #[test]
    fn test_merge_all_staging() {
        let tmp = tempfile::tempdir().unwrap();
        let main_path = tmp.path().join("service.autotest.json");
        let staging_path = tmp.path().join("service.autotest.staging.json");

        let main_doc = serde_json::json!({
            "schema_version": "1.0",
            "source": {"file": "service.ts", "content_hash": "sha256:1", "analyzer": "test", "dependencies": {}},
            "fixtures": {},
            "targets": [{
                "id": "Service.test",
                "kind": "method",
                "signature": {"constructor_args": [], "parameters": [], "return_type": "string"},
                "detected_side_effects": [],
                "scenarios": []
            }]
        });
        fs::write(&main_path, serde_json::to_string(&main_doc).unwrap()).unwrap();

        let entries = vec![StagingEntry {
            target: "Service.test".to_string(),
            scenario: "test_run".to_string(),
            recorded_at: "2024-01-01T00:00:00Z".to_string(),
            mode: "recorded".to_string(),
            expected: serde_json::json!({"value": "ok"}),
        }];
        StagingManager::write_staging(&staging_path, &entries).unwrap();

        let (files, scenarios) = StagingManager::merge_all_staging(tmp.path()).unwrap();
        assert_eq!(files, 1);
        assert_eq!(scenarios, 1);
        assert!(!staging_path.exists());
    }
}
