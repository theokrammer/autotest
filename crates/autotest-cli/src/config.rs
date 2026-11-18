use std::collections::BTreeMap;
use std::path::Path;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutotestConfig {
    pub project: ProjectConfig,
    pub solver: SolverSection,
    #[serde(default)]
    pub languages: BTreeMap<String, LanguageConfig>,
    pub sandbox: SandboxSection,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectConfig {
    pub scenario_dir: String,
    pub hydrator_install: String,
    pub deduplicate_fixtures: bool,
    pub canonical_sort: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SolverSection {
    pub max_depth: usize,
    pub max_loop_unroll: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LanguageConfig {
    pub source_roots: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tsconfig: Option<String>,
    pub extractor: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fallback_extractor: Option<String>,
    pub test_framework: String,
    pub bridge_file: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxSection {
    pub mode: String,
    pub timeout_ms: u64,
    pub block_network: bool,
    pub allow_temp_fs_only: bool,
}

impl Default for AutotestConfig {
    fn default() -> Self {
        let mut languages = BTreeMap::new();
        languages.insert(
            "php".to_string(),
            LanguageConfig {
                source_roots: vec!["src".to_string()],
                tsconfig: None,
                extractor: "builtin:mago".to_string(),
                fallback_extractor: Some("php:phpstan".to_string()),
                test_framework: "phpunit".to_string(),
                bridge_file: "tests/AutotestBridgeTest.php".to_string(),
            },
        );
        languages.insert(
            "typescript".to_string(),
            LanguageConfig {
                source_roots: vec!["src".to_string()],
                tsconfig: Some("tsconfig.json".to_string()),
                extractor: "node:ts-compiler-api".to_string(),
                fallback_extractor: None,
                test_framework: "vitest".to_string(),
                bridge_file: "tests/autotest.bridge.test.ts".to_string(),
            },
        );

        Self {
            project: ProjectConfig {
                scenario_dir: "tests/scenarios".to_string(),
                hydrator_install: "package_manager".to_string(),
                deduplicate_fixtures: true,
                canonical_sort: true,
            },
            solver: SolverSection {
                max_depth: 8,
                max_loop_unroll: 2,
            },
            languages,
            sandbox: SandboxSection {
                mode: "os_native".to_string(),
                timeout_ms: 500,
                block_network: true,
                allow_temp_fs_only: true,
            },
        }
    }
}

impl AutotestConfig {
    pub fn load_from_dir(dir: &Path) -> anyhow::Result<Self> {
        let path = dir.join("autotest.toml");
        if !path.exists() {
            return Ok(Self::default());
        }
        let content = std::fs::read_to_string(path)?;
        let config: Self = toml::from_str(&content)?;
        Ok(config)
    }

    pub fn save_to_dir(&self, dir: &Path) -> anyhow::Result<()> {
        let path = dir.join("autotest.toml");
        let content = toml::to_string_pretty(self)?;
        std::fs::write(path, content)?;
        Ok(())
    }
}
