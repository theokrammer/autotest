use regex::Regex;
use serde_json::{json, Map, Value};
use tracing::debug;
use crate::ir::{
    DetectedSideEffect, ExpectedOutcome, OutcomeKind, ParameterDefinition,
    ScenarioDefinition, ScenarioMode, TargetDefinition,
};

#[derive(Debug, Clone)]
pub struct SolverConfig {
    pub max_depth: usize,
    pub max_loop_unroll: usize,
}

impl Default for SolverConfig {
    fn default() -> Self {
        Self {
            max_depth: 8,
            max_loop_unroll: 2,
        }
    }
}

pub struct SymbolicSolver {
    pub config: SolverConfig,
}

impl SymbolicSolver {
    pub fn new(config: SolverConfig) -> Self {
        Self { config }
    }

    /// Evaluates if a target has unmockable side effects according to the 6 criteria
    pub fn has_unmockable_side_effect(effects: &[DetectedSideEffect]) -> Option<&DetectedSideEffect> {
        effects.iter().find(|e| !e.mockable)
    }

    /// Generates boundary and contract scenarios for a target
    pub fn solve_target(&self, target: &TargetDefinition) -> Vec<ScenarioDefinition> {
        debug!("Solving target {}", target.id);
        if let Some(_blocked) = Self::has_unmockable_side_effect(&target.detected_side_effects) {
            // Exclude from automatic generation
            return Vec::new();
        }

        let mut scenarios = Vec::new();

        // 1. Generate happy-path boundary scenario
        let target_prefix = target.id.replace("::", "_").replace('\\', "_").replace('.', "_");
        let (boundary_inputs, _boundary_name) = self.generate_boundary_inputs(&target.signature.parameters, false);
        scenarios.push(ScenarioDefinition {
            name: format!("{}_boundary_nominal", target.id.replace("::", "_").replace('\\', "_")),
            mode: ScenarioMode::SymbolicContract,
            locked: false,
            stale: false,
            tags: vec!["boundary".to_string(), "happy_path".to_string()],
            mocks: Vec::new(),
            inputs: Value::Object(boundary_inputs.clone()),
            expected: ExpectedOutcome {
                outcome: OutcomeKind::Return,
                expected_type: target.signature.return_type.clone(),
                value: target.signature.return_type.as_deref().map(default_return_for_type),
                exception_type: None,
                message_contains: None,
                expected_events: Vec::new(),
            },
        });

        // 1b. Generate contrasting boundary scenario (toggles bools, empty/non-empty strings & arrays, 0/positive numbers)
        let (alt_inputs, _) = self.generate_boundary_inputs(&target.signature.parameters, true);
        if !alt_inputs.is_empty() && alt_inputs != boundary_inputs {
            scenarios.push(ScenarioDefinition {
                name: format!("{}_boundary_alt", target_prefix),
                mode: ScenarioMode::SymbolicContract,
                locked: false,
                stale: false,
                tags: vec!["boundary".to_string(), "branch_coverage".to_string()],
                mocks: Vec::new(),
                inputs: Value::Object(alt_inputs),
                expected: ExpectedOutcome {
                    outcome: OutcomeKind::Return,
                    expected_type: target.signature.return_type.clone(),
                    value: target.signature.return_type.as_deref().map(default_return_for_type),
                    exception_type: None,
                    message_contains: None,
                    expected_events: Vec::new(),
                },
            });
        }

        // 2. Generate guard-clause / negative test scenarios for constrained parameters
        for param in &target.signature.parameters {
            for constraint in &param.constraints {
                if let Some((invalid_val, exc_name)) = generate_negative_sample(&param.param_type, constraint) {
                    let mut inputs = Map::new();
                    // Fill other parameters with valid boundary values
                    for other in &target.signature.parameters {
                        if other.name == param.name {
                            inputs.insert(param.name.clone(), invalid_val.clone());
                        } else {
                            inputs.insert(other.name.clone(), default_valid_for_param(other));
                        }
                    }

                    let target_prefix = target.id.replace("::", "_").replace('\\', "_").replace('.', "_");
                    scenarios.push(ScenarioDefinition {
                        name: format!("{}_guard_{}_{}_throws", target_prefix, param.name, sanitize_tag(constraint)),
                        mode: ScenarioMode::SymbolicContract,
                        locked: false,
                        stale: false,
                        tags: vec!["guard_clause".to_string(), "negative".to_string()],
                        mocks: Vec::new(),
                        inputs: Value::Object(inputs),
                        expected: ExpectedOutcome {
                            outcome: OutcomeKind::Exception,
                            expected_type: None,
                            value: None,
                            exception_type: Some(exc_name),
                            message_contains: Some(param.name.clone()),
                            expected_events: Vec::new(),
                        },
                    });
                }
            }
        }

        scenarios
    }

    fn generate_boundary_inputs(&self, params: &[ParameterDefinition], alt: bool) -> (Map<String, Value>, String) {
        let mut map = Map::new();
        for p in params {
            let val = if alt {
                alt_boundary_for_param(p)
            } else {
                default_valid_for_param(p)
            };
            map.insert(p.name.clone(), val);
        }
        (map, if alt { "alt" } else { "nominal" }.to_string())
    }
}

fn sanitize_tag(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .collect()
}

fn default_valid_for_param(param: &ParameterDefinition) -> Value {
    if let Some(ref def) = param.default_value {
        return def.clone();
    }

    let t = param.param_type.to_lowercase();
    let n = param.name.to_lowercase();
    let _has_finite = param.constraints.iter().any(|c| c == "finite");
    let has_non_neg = param.constraints.iter().any(|c| c.contains(">= 0") || c.contains("> 0"));
    let has_len_2 = param.constraints.iter().any(|c| c.contains("length == 2") || c.contains("len == 2"));

    if t.ends_with("[]") || t.starts_with("array<") || t == "array" || (t.starts_with("readonly ") && t.ends_with("[]")) {
        json!([])
    } else if t.contains("float") || t.contains("double") {
        if has_non_neg { json!(0.0) } else { json!(1.0) }
    } else if t.contains("int") || t.contains("number") {
        if n.contains("port") {
            json!(0)
        } else if has_non_neg {
            json!(0)
        } else {
            json!(1)
        }
    } else if t.contains("string") {
        if has_len_2 {
            json!("DE")
        } else if n.contains("alphabet") {
            json!("abcdef0123456789")
        } else {
            json!("test")
        }
    } else if t.contains("bool") {
        json!(true)
    } else if t == "any" {
        if n.contains("alphabet") {
            json!("abcdef0123456789")
        } else if n.contains("size") || n.contains("bytes") || n.contains("length") || n.contains("count") || n.contains("limit") {
            json!(16)
        } else if n.contains("port") {
            json!(0)
        } else if n.contains("name") || n.contains("path") || n.contains("url") || n.contains("key") || n.contains("pattern") || n.contains("prefix") || n.contains("host") {
            json!("test")
        } else {
            json!({})
        }
    } else if t == "null"
        || t == "undefined"
        || t == "void"
        || t == "mixed"
        || t.starts_with('?')
        || t.contains("| null")
        || t.contains("|null")
        || t.contains("| undefined")
        || t.contains("function")
        || t.contains("=>")
    {
        json!(null)
    } else {
        json!({})
    }
}

fn alt_boundary_for_param(param: &ParameterDefinition) -> Value {
    let t = param.param_type.to_lowercase();
    let n = param.name.to_lowercase();

    if t.ends_with("[]") || t.starts_with("array<") || t == "array" || (t.starts_with("readonly ") && t.ends_with("[]")) {
        if t.contains("number") || t.contains("int") {
            json!([1])
        } else {
            json!(["test"])
        }
    } else if t.contains("float") || t.contains("double") {
        json!(0.0)
    } else if t.contains("int") || t.contains("number") {
        if n.contains("port") {
            json!(0)
        } else {
            json!(0)
        }
    } else if t.contains("string") {
        if n.contains("alphabet") {
            json!("abcde")
        } else if n.contains("host") {
            json!("example.com/")
        } else if n.contains("pattern") || n.contains("path") {
            json!("/users/{id}")
        } else {
            json!("")
        }
    } else if t.contains("bool") {
        json!(false)
    } else if t == "any" {
        if n.contains("alphabet") {
            json!("abcde")
        } else if n.contains("size") || n.contains("bytes") || n.contains("length") || n.contains("count") || n.contains("limit") {
            json!(0)
        } else {
            default_valid_for_param(param)
        }
    } else {
        default_valid_for_param(param)
    }
}

fn generate_negative_sample(param_type: &str, constraint: &str) -> Option<(Value, String)> {
    let t = param_type.to_lowercase();
    if constraint.contains(">= 0") {
        if t.contains("float") || t.contains("double") {
            return Some((json!(-0.01), "InvalidArgumentException".to_string()));
        } else if t.contains("int") {
            return Some((json!(-1), "InvalidArgumentException".to_string()));
        }
    } else if constraint.contains("> 0") {
        if t.contains("float") || t.contains("double") {
            return Some((json!(0.0), "InvalidArgumentException".to_string()));
        } else if t.contains("int") {
            return Some((json!(0), "InvalidArgumentException".to_string()));
        }
    } else if constraint == "non-empty-string" {
        return Some((json!(""), "InvalidArgumentException".to_string()));
    } else if let Ok(re) = Regex::new(r"length\s*==\s*(\d+)") {
        if let Some(caps) = re.captures(constraint) {
            let len: usize = caps[1].parse().unwrap_or(2);
            let sample = "A".repeat(len.saturating_sub(1));
            return Some((json!(sample), "InvalidArgumentException".to_string()));
        }
    }
    None
}

fn default_return_for_type(t: &str) -> Value {
    if t.contains("Money") {
        json!({
            "$type": t,
            "$ctor": [0, "EUR"],
            "$factory": null,
            "$props": {}
        })
    } else if t.contains("bool") {
        json!(true)
    } else if t.contains("int") {
        json!(0)
    } else if t.contains("float") {
        json!(0.0)
    } else if t.contains("string") {
        json!("")
    } else {
        json!(null)
    }
}
