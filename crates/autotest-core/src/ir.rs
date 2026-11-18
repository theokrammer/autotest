use std::collections::BTreeMap;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ScenarioDocument {
    #[serde(rename = "$schema", skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    pub schema_version: String,
    pub source: SourceMetadata,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub fixtures: BTreeMap<String, FixtureDefinition>,
    pub targets: Vec<TargetDefinition>,
}

impl Default for ScenarioDocument {
    fn default() -> Self {
        Self {
            schema: Some("https://autotest.dev/schema/scenario.v1.schema.json".to_string()),
            schema_version: "1.0".to_string(),
            source: SourceMetadata::default(),
            fixtures: BTreeMap::new(),
            targets: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default, JsonSchema)]
pub struct SourceMetadata {
    pub file: String,
    pub content_hash: String,
    pub analyzer: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub dependencies: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct FixtureDefinition {
    pub target: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub when_args: Vec<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub returns: Vec<FixtureReturn>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub mutate_args: BTreeMap<String, serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expect_calls: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct FixtureReturn {
    #[serde(rename = "return", skip_serializing_if = "Option::is_none")]
    pub return_value: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exception: Option<FixtureException>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FixtureException {
    #[serde(rename = "type")]
    pub exception_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TargetDefinition {
    pub id: String,
    pub kind: TargetKind,
    pub signature: SignatureDefinition,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub detected_side_effects: Vec<DetectedSideEffect>,
    #[serde(default)]
    pub scenarios: Vec<ScenarioDefinition>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TargetKind {
    Method,
    Function,
    HttpEndpoint,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default, JsonSchema)]
pub struct SignatureDefinition {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub constructor_args: Vec<ParameterDefinition>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parameters: Vec<ParameterDefinition>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub return_type: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ParameterDefinition {
    pub name: String,
    #[serde(rename = "type")]
    pub param_type: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub constraints: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_value: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DetectedSideEffect {
    #[serde(rename = "type")]
    pub effect_type: String,
    pub target: String,
    pub mockable: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ScenarioDefinition {
    pub name: String,
    pub mode: ScenarioMode,
    #[serde(default)]
    pub locked: bool,
    #[serde(default)]
    pub stale: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mocks: Vec<MockReferenceOrInline>,
    pub inputs: serde_json::Value,
    pub expected: ExpectedOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ScenarioMode {
    SymbolicContract,
    Recorded,
    Manual,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum MockReferenceOrInline {
    Reference {
        #[serde(rename = "$ref")]
        reference: String,
    },
    Inline(FixtureDefinition),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ExpectedOutcome {
    pub outcome: OutcomeKind,
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub expected_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exception_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_contains: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub expected_events: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OutcomeKind {
    Return,
    Exception,
    NoCrash,
}
