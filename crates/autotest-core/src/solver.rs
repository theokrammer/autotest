use std::collections::BTreeMap;
use regex::Regex;
use serde_json::{json, Map, Value};
use tracing::debug;
use crate::ir::{
    DetectedSideEffect, ExpectedOutcome, FixtureDefinition, FixtureReturn, MockReferenceOrInline,
    OutcomeKind, ParameterDefinition, ScenarioDefinition, ScenarioDocument, ScenarioMode,
    SetupCall, TargetDefinition, TargetKind,
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
            setup_calls: Vec::new(),
            invoke_returned_args: None,
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
                setup_calls: Vec::new(),
                invoke_returned_args: None,
                inputs: Value::Object(alt_inputs.clone()),
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

        // 1c. Higher-order / callable return invocation (executes returned closure/factory body)
        if is_higher_order_target(target) {
            scenarios.push(ScenarioDefinition {
                name: format!("{}_invoke_returned", target_prefix),
                mode: ScenarioMode::SymbolicContract,
                locked: false,
                stale: false,
                tags: vec!["higher_order".to_string(), "branch_coverage".to_string()],
                mocks: Vec::new(),
                setup_calls: Vec::new(),
                invoke_returned_args: Some(Vec::new()),
                inputs: Value::Object(boundary_inputs.clone()),
                expected: ExpectedOutcome {
                    outcome: OutcomeKind::Return,
                    expected_type: None,
                    value: None,
                    exception_type: None,
                    message_contains: None,
                    expected_events: Vec::new(),
                },
            });
            if !alt_inputs.is_empty() && alt_inputs != boundary_inputs {
                scenarios.push(ScenarioDefinition {
                    name: format!("{}_invoke_returned_alt", target_prefix),
                    mode: ScenarioMode::SymbolicContract,
                    locked: false,
                    stale: false,
                    tags: vec!["higher_order".to_string(), "branch_coverage".to_string()],
                    mocks: Vec::new(),
                    setup_calls: Vec::new(),
                    invoke_returned_args: Some(vec![json!(10)]),
                    inputs: Value::Object(alt_inputs),
                    expected: ExpectedOutcome {
                        outcome: OutcomeKind::Return,
                        expected_type: None,
                        value: None,
                        exception_type: None,
                        message_contains: None,
                        expected_events: Vec::new(),
                    },
                });
            }
        }

        // 1d. Branch-guided input synthesis from extracted AST literals (`eq:<lit>`, `branch_num:<n>`)
        let mut branch_count = 0usize;
        for param in &target.signature.parameters {
            for constraint in &param.constraints {
                if branch_count >= 4 {
                    break;
                }
                let branch_val = if let Some(lit) = constraint.strip_prefix("eq:") {
                    Some((json!(lit), sanitize_tag(lit)))
                } else if let Some(num_str) = constraint.strip_prefix("branch_num:") {
                    num_str.parse::<i64>().ok().map(|n| (json!(n), sanitize_tag(num_str)))
                } else {
                    None
                };
                if let Some((val, tag)) = branch_val {
                    let mut inputs = Map::new();
                    for other in &target.signature.parameters {
                        if other.name == param.name {
                            inputs.insert(param.name.clone(), val.clone());
                        } else {
                            inputs.insert(other.name.clone(), default_valid_for_param(other));
                        }
                    }
                    let sc_name = format!("{}_branch_{}_{}", target_prefix, param.name, tag);
                    if !scenarios.iter().any(|s| s.name == sc_name) {
                        scenarios.push(ScenarioDefinition {
                            name: sc_name,
                            mode: ScenarioMode::SymbolicContract,
                            locked: false,
                            stale: false,
                            tags: vec!["branch_guided".to_string(), "branch_coverage".to_string()],
                            mocks: Vec::new(),
                            setup_calls: Vec::new(),
                            invoke_returned_args: None,
                            inputs: Value::Object(inputs),
                            expected: ExpectedOutcome {
                                outcome: OutcomeKind::Return,
                                expected_type: target.signature.return_type.clone(),
                                value: target.signature.return_type.as_deref().map(default_return_for_type),
                                exception_type: None,
                                message_contains: None,
                                expected_events: Vec::new(),
                            },
                        });
                        branch_count += 1;
                    }
                }
            }
        }

        // 1e. Non-empty mock returns for injected dependencies (`interface_call` / `service_call`)
        if !target.signature.constructor_args.is_empty() {
            let mut inline_mocks = Vec::new();
            let mut seen_targets = std::collections::HashSet::new();
            for eff in &target.detected_side_effects {
                if eff.mockable
                    && (eff.effect_type == "interface_call" || eff.effect_type == "service_call")
                    && seen_targets.insert(eff.target.clone())
                {
                    let mock_ret = synthesize_mock_return_for_target(&eff.target);
                    inline_mocks.push(MockReferenceOrInline::Inline(FixtureDefinition {
                        target: eff.target.clone(),
                        when_args: Vec::new(),
                        returns: vec![FixtureReturn {
                            return_value: Some(mock_ret),
                            exception: None,
                        }],
                        mutate_args: BTreeMap::new(),
                        expect_calls: None,
                    }));
                }
            }
            if !inline_mocks.is_empty() {
                scenarios.push(ScenarioDefinition {
                    name: format!("{}_with_mocked_dep", target_prefix),
                    mode: ScenarioMode::SymbolicContract,
                    locked: false,
                    stale: false,
                    tags: vec!["mocked_dependency".to_string(), "branch_coverage".to_string()],
                    mocks: inline_mocks,
                    setup_calls: Vec::new(),
                    invoke_returned_args: None,
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
            }
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

                    scenarios.push(ScenarioDefinition {
                        name: format!("{}_guard_{}_{}_throws", target_prefix, param.name, sanitize_tag(constraint)),
                        mode: ScenarioMode::SymbolicContract,
                        locked: false,
                        stale: false,
                        tags: vec!["guard_clause".to_string(), "negative".to_string()],
                        mocks: Vec::new(),
                        setup_calls: Vec::new(),
                        invoke_returned_args: None,
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

    /// Solves all targets in a document and synthesizes stateful call chains (`setup_calls`)
    /// pairing mutator/builder methods with reader/getter methods on the same class.
    pub fn solve_document(&self, doc: &mut ScenarioDocument) {
        // First solve each target individually
        for target in &mut doc.targets {
            let new_scenarios = self.solve_target(target);
            if !new_scenarios.is_empty() {
                target.scenarios = new_scenarios;
            }
        }

        // Collect a representative mutator/builder method per class
        let mut class_mutators: BTreeMap<String, (String, Vec<Value>)> = BTreeMap::new();
        for target in &doc.targets {
            if target.kind != TargetKind::Method {
                continue;
            }
            if Self::has_unmockable_side_effect(&target.detected_side_effects).is_some() {
                continue;
            }
            if let Some((cls, method)) = split_class_method(&target.id) {
                if is_mutator_method(method) && !class_mutators.contains_key(cls) {
                    let args: Vec<Value> = target
                        .signature
                        .parameters
                        .iter()
                        .map(default_valid_for_param)
                        .collect();
                    class_mutators.insert(cls.to_string(), (method.to_string(), args));
                }
            }
        }

        // Add stateful 2-step scenarios to reader/getter methods
        for target in &mut doc.targets {
            if target.kind != TargetKind::Method || target.scenarios.is_empty() {
                continue;
            }
            if let Some((cls, method)) = split_class_method(&target.id) {
                if is_reader_method(method) {
                    if let Some((mutator_name, mutator_args)) = class_mutators.get(cls) {
                        if mutator_name != method {
                            let target_prefix = target.id.replace("::", "_").replace('\\', "_").replace('.', "_");
                            let sc_name = format!("{}_after_{}", target_prefix, mutator_name);
                            if !target.scenarios.iter().any(|s| s.name == sc_name) {
                                let (boundary_inputs, _) =
                                    self.generate_boundary_inputs(&target.signature.parameters, false);
                                target.scenarios.push(ScenarioDefinition {
                                    name: sc_name,
                                    mode: ScenarioMode::SymbolicContract,
                                    locked: false,
                                    stale: false,
                                    tags: vec!["stateful_chain".to_string(), "branch_coverage".to_string()],
                                    mocks: Vec::new(),
                                    setup_calls: vec![SetupCall {
                                        method: mutator_name.clone(),
                                        args: mutator_args.clone(),
                                    }],
                                    invoke_returned_args: None,
                                    inputs: Value::Object(boundary_inputs),
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
                        }
                    }
                }
            }
        }
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
    } else if t.contains("callable")
        || t.contains("closure")
        || t.contains("function")
        || t.contains("=>")
        || (t == "any" && (n == "getrandom" || n == "callback" || n == "cb" || n == "fn" || n == "handler" || n == "predicate" || n == "next"))
    {
        if n == "getrandom" {
            json!({
                "$type": "Callback",
                "returns": {
                    "$type": "Uint8Array",
                    "data": [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32]
                }
            })
        } else if n == "predicate" {
            json!({ "$type": "Callback", "returns": true })
        } else {
            json!({ "$type": "Callback", "returns": null })
        }
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
    {
        json!(null)
    } else {
        json!({})
    }
}

fn is_higher_order_target(target: &TargetDefinition) -> bool {
    if let Some(ref rt) = target.signature.return_type {
        let r = rt.to_lowercase();
        if r.contains("=>") || r.contains("function") || r.contains("callable") || r.contains("closure") || r.contains("decorator") {
            return true;
        }
    }
    let id_lower = target.id.to_lowercase();
    id_lower == "customalphabet"
        || id_lower == "customrandom"
        || id_lower.ends_with("decorator")
        || id_lower.ends_with("interceptor")
        || id_lower.ends_with("factory")
}

fn synthesize_mock_return_for_target(effect_target: &str) -> Value {
    let method = effect_target
        .rsplit(|c| c == '.' || c == ':')
        .next()
        .unwrap_or("")
        .to_lowercase();
    if method.starts_with("is") || method.starts_with("has") || method.starts_with("can") {
        json!(true)
    } else if method.ends_with('s') || method.contains("list") || method.contains("all") || method.contains("items") {
        json!(["mock_item"])
    } else if method.contains("count") || method.contains("size") || method.contains("length") {
        json!(1)
    } else if method.starts_with("get") || method.starts_with("find") {
        json!("mocked")
    } else {
        json!({})
    }
}

fn split_class_method(target_id: &str) -> Option<(&str, &str)> {
    if let Some(idx) = target_id.rfind("::") {
        Some((&target_id[..idx], &target_id[idx + 2..]))
    } else if let Some(idx) = target_id.rfind('.') {
        Some((&target_id[..idx], &target_id[idx + 1..]))
    } else {
        None
    }
}

fn is_mutator_method(method: &str) -> bool {
    let m = method.to_lowercase();
    m.starts_with("add")
        || m.starts_with("set")
        || m.starts_with("register")
        || m.starts_with("push")
        || m.starts_with("insert")
        || m == "name"
        || m == "host"
        || m == "hosts"
        || m == "defaults"
        || m == "prefix"
        || m == "nameprefix"
        || m == "middleware"
}

fn is_reader_method(method: &str) -> bool {
    let m = method.to_lowercase();
    m.starts_with("get")
        || m.starts_with("has")
        || m.starts_with("find")
        || m.starts_with("is")
        || m == "all"
        || m == "toarray"
        || m == "tobuffer"
        || m == "tostring"
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
