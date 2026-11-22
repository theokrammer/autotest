use std::collections::{BTreeMap, HashSet};
use regex::Regex;
use serde_json::{json, Map, Value};
use tracing::debug;
use crate::ir::{
    DetectedSideEffect, ExpectedOutcome, FixtureDefinition, FixtureReturn, MetadataSeed,
    MockReferenceOrInline, OutcomeKind, ParameterDefinition, ScenarioDefinition, ScenarioDocument,
    ScenarioMode, SetupCall, TargetDefinition, TargetKind,
};
use crate::project_index::{ClassIndexEntry, ProjectIndex};

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

    /// Generates boundary and contract scenarios for a target (without cross-file index)
    pub fn solve_target(&self, target: &TargetDefinition) -> Vec<ScenarioDefinition> {
        self.solve_target_with_index(target, "", &ProjectIndex::default())
    }

    /// Generates boundary and contract scenarios for a target using cross-file ProjectIndex
    pub fn solve_target_with_index(
        &self,
        target: &TargetDefinition,
        source_file: &str,
        index: &ProjectIndex,
    ) -> Vec<ScenarioDefinition> {
        debug!("Solving target {}", target.id);
        if let Some(_blocked) = Self::has_unmockable_side_effect(&target.detected_side_effects) {
            return Vec::new();
        }

        let is_php = source_file.ends_with(".php") || target.id.contains("::");
        let mut scenarios = Vec::new();
        let target_prefix = target.id.replace("::", "_").replace('\\', "_").replace('.', "_");

        // Determine default static factory for private-constructor classes
        let target_short_cls = split_class_method(&target.id)
            .map(|(c, _)| c.rsplit('\\').next().unwrap_or(c).rsplit('.').next().unwrap_or(c))
            .unwrap_or("");
        let default_factory = if target.signature.private_constructor && !target.signature.static_factories.is_empty() {
            let chosen = choose_best_static_factory(&target.signature.static_factories, target_short_cls)
                .unwrap_or(&target.signature.static_factories[0]);
            let fargs: Vec<Value> = chosen
                .parameters
                .iter()
                .map(|p| synthesize_value_for_param(p, index, false, is_php, 0))
                .collect();
            Some(SetupCall {
                method: chosen.name.clone(),
                args: fargs,
            })
        } else {
            None
        };

        // 1. Happy-path boundary scenario
        let (boundary_inputs, _) =
            self.generate_boundary_inputs_with_index(&target.signature.parameters, false, index, is_php);
        scenarios.push(ScenarioDefinition {
            name: format!("{}_boundary_nominal", target.id.replace("::", "_").replace('\\', "_")),
            mode: ScenarioMode::SymbolicContract,
            locked: false,
            stale: false,
            tags: vec!["boundary".to_string(), "happy_path".to_string()],
            mocks: Vec::new(),
            setup_calls: Vec::new(),
            invoke_returned_args: None,
            factory: default_factory.clone(),
            constructor_overrides: BTreeMap::new(),
            metadata_seeds: Vec::new(),
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

        // 1b. Contrasting boundary scenario
        let (alt_inputs, _) =
            self.generate_boundary_inputs_with_index(&target.signature.parameters, true, index, is_php);
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
                factory: default_factory.clone(),
                constructor_overrides: BTreeMap::new(),
                metadata_seeds: Vec::new(),
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

        // 1b-factory. Named static factory variants when class exposes multiple factories (e.g. fromSuccess / fromFailure)
        if target.kind == TargetKind::Method && target.signature.static_factories.len() >= 2 {
            for f in target.signature.static_factories.iter().take(2) {
                let fargs: Vec<Value> = f
                    .parameters
                    .iter()
                    .map(|p| synthesize_value_for_param(p, index, false, is_php, 0))
                    .collect();
                let sc_name = format!("{}_via_{}", target_prefix, f.name);
                if !scenarios.iter().any(|s| s.name == sc_name) {
                    scenarios.push(ScenarioDefinition {
                        name: sc_name,
                        mode: ScenarioMode::SymbolicContract,
                        locked: false,
                        stale: false,
                        tags: vec!["static_factory".to_string(), "branch_coverage".to_string()],
                        mocks: Vec::new(),
                        setup_calls: Vec::new(),
                        invoke_returned_args: None,
                        factory: Some(SetupCall {
                            method: f.name.clone(),
                            args: fargs,
                        }),
                        constructor_overrides: BTreeMap::new(),
                        metadata_seeds: Vec::new(),
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
        }

        // 1c. Higher-order / callable return invocation
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
                factory: default_factory.clone(),
                constructor_overrides: BTreeMap::new(),
                metadata_seeds: Vec::new(),
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
                    factory: default_factory.clone(),
                    constructor_overrides: BTreeMap::new(),
                    metadata_seeds: Vec::new(),
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
                if branch_count >= 10 {
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
                            inputs.insert(other.name.clone(), synthesize_value_for_param(other, index, false, is_php, 0));
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
                            factory: default_factory.clone(),
                            constructor_overrides: BTreeMap::new(),
                            metadata_seeds: Vec::new(),
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

        // 1d-props. Object-shape scenario when parameters have `prop:*` constraints
        let has_prop_constraints = target
            .signature
            .parameters
            .iter()
            .any(|p| p.constraints.iter().any(|c| c.starts_with("prop:") || c.starts_with("prop_bool:") || c.starts_with("prop_arr:")));
        if has_prop_constraints {
            let mut prop_inputs = Map::new();
            for p in &target.signature.parameters {
                if p.constraints.iter().any(|c| c.starts_with("prop:") || c.starts_with("prop_bool:") || c.starts_with("prop_arr:")) {
                    prop_inputs.insert(p.name.clone(), synthesize_object_from_prop_constraints(&p.constraints));
                } else {
                    prop_inputs.insert(p.name.clone(), synthesize_value_for_param(p, index, false, is_php, 0));
                }
            }
            let sc_name = format!("{}_with_props", target_prefix);
            if !scenarios.iter().any(|s| s.name == sc_name) {
                scenarios.push(ScenarioDefinition {
                    name: sc_name,
                    mode: ScenarioMode::SymbolicContract,
                    locked: false,
                    stale: false,
                    tags: vec!["object_shape".to_string(), "branch_coverage".to_string()],
                    mocks: Vec::new(),
                    setup_calls: Vec::new(),
                    invoke_returned_args: None,
                    factory: default_factory.clone(),
                    constructor_overrides: BTreeMap::new(),
                    metadata_seeds: Vec::new(),
                    inputs: Value::Object(prop_inputs),
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

        // 1e. Type-aware mock returns, boolean flips, and multi-factory return variants
        if !target.signature.constructor_args.is_empty() {
            let mut unique_effects: Vec<(&str, &str, Option<&str>)> = Vec::new();
            let mut seen_targets = HashSet::new();
            for eff in &target.detected_side_effects {
                if eff.mockable
                    && (eff.effect_type == "interface_call" || eff.effect_type == "service_call")
                    && seen_targets.insert(eff.target.clone())
                {
                    let (dep_name, dep_method) = eff
                        .target
                        .split_once('.')
                        .or_else(|| eff.target.split_once("::"))
                        .unwrap_or(("", eff.target.as_str()));
                    let dep_type = target
                        .signature
                        .constructor_args
                        .iter()
                        .find(|p| p.name == dep_name)
                        .map(|p| p.param_type.as_str())
                        .unwrap_or("");
                    let ret_type = if !dep_type.is_empty() {
                        index.resolve_method_return_type(dep_type, dep_method)
                    } else {
                        None
                    };
                    unique_effects.push((eff.target.as_str(), dep_method, ret_type));
                }
            }

            if !unique_effects.is_empty() {
                // Base type-aware mocked dependency scenario
                let inline_mocks: Vec<MockReferenceOrInline> = unique_effects
                    .iter()
                    .map(|(eff_target, _m, ret_ty)| {
                        let mock_ret = if let Some(rt) = ret_ty {
                            synthesize_value_for_type(rt, _m, index, true, is_php, 0)
                        } else {
                            synthesize_mock_return_for_target(eff_target)
                        };
                        make_inline_mock(eff_target, mock_ret)
                    })
                    .collect();

                scenarios.push(ScenarioDefinition {
                    name: format!("{}_with_mocked_dep", target_prefix),
                    mode: ScenarioMode::SymbolicContract,
                    locked: false,
                    stale: false,
                    tags: vec!["mocked_dependency".to_string(), "branch_coverage".to_string()],
                    mocks: inline_mocks,
                    setup_calls: Vec::new(),
                    invoke_returned_args: None,
                    factory: default_factory.clone(),
                    constructor_overrides: BTreeMap::new(),
                    metadata_seeds: Vec::new(),
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

                // Boolean branch flips if any mocked method returns bool
                let has_bool_mock = unique_effects.iter().any(|(_, m, rt)| {
                    let ml = m.to_lowercase();
                    rt.map(|r| r.contains("bool")).unwrap_or(false)
                        || ml.starts_with("is")
                        || ml.starts_with("has")
                        || ml.starts_with("can")
                });
                if has_bool_mock {
                    for bool_val in [true, false] {
                        let mocks: Vec<MockReferenceOrInline> = unique_effects
                            .iter()
                            .map(|(eff_target, m, rt)| {
                                let ml = m.to_lowercase();
                                let is_bool = rt.map(|r| r.contains("bool")).unwrap_or(false)
                                    || ml.starts_with("is")
                                    || ml.starts_with("has")
                                    || ml.starts_with("can");
                                let val = if is_bool {
                                    json!(bool_val)
                                } else if let Some(rt) = rt {
                                    synthesize_value_for_type(rt, m, index, true, is_php, 0)
                                } else {
                                    synthesize_mock_return_for_target(eff_target)
                                };
                                make_inline_mock(eff_target, val)
                            })
                            .collect();
                        let sc_name = format!("{}_mock_{}", target_prefix, bool_val);
                        if !scenarios.iter().any(|s| s.name == sc_name) {
                            scenarios.push(ScenarioDefinition {
                                name: sc_name,
                                mode: ScenarioMode::SymbolicContract,
                                locked: false,
                                stale: false,
                                tags: vec!["mock_branch_flip".to_string(), "branch_coverage".to_string()],
                                mocks,
                                setup_calls: Vec::new(),
                                invoke_returned_args: None,
                                factory: default_factory.clone(),
                                constructor_overrides: BTreeMap::new(),
                                metadata_seeds: Vec::new(),
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
                }

                // Multi-factory return variants (e.g. UrlMatcherInterface::match -> MatchingResult::fromSuccess vs fromFailure)
                for (idx_eff, (_eff_target, _m, ret_ty)) in unique_effects.iter().enumerate() {
                    if let Some(rt) = ret_ty {
                        if let Some(ret_cls) = index.resolve_class(rt) {
                            if ret_cls.static_factories.len() >= 2 {
                                for f in ret_cls.static_factories.iter().take(2) {
                                    let fargs: Vec<Value> = f
                                        .parameters
                                        .iter()
                                        .map(|p| synthesize_value_for_param(p, index, true, is_php, 1))
                                        .collect();
                                    let mut dto = json!({
                                        "$type": ret_cls.name,
                                        "$factory": f.name,
                                        "$ctor": fargs
                                    });
                                    if !is_php && !ret_cls.source_file.is_empty() {
                                        dto["$source_file"] = json!(ret_cls.source_file);
                                    }
                                    let mut mocks: Vec<MockReferenceOrInline> = Vec::new();
                                    for (j, (other_target, other_m, other_rt)) in unique_effects.iter().enumerate() {
                                        if j == idx_eff {
                                            mocks.push(make_inline_mock(other_target, dto.clone()));
                                        } else {
                                            let val = if let Some(ort) = other_rt {
                                                synthesize_value_for_type(ort, other_m, index, true, is_php, 0)
                                            } else {
                                                synthesize_mock_return_for_target(other_target)
                                            };
                                            mocks.push(make_inline_mock(other_target, val));
                                        }
                                    }
                                    let sc_name = format!("{}_mock_{}", target_prefix, f.name);
                                    if !scenarios.iter().any(|s| s.name == sc_name) {
                                        scenarios.push(ScenarioDefinition {
                                            name: sc_name,
                                            mode: ScenarioMode::SymbolicContract,
                                            locked: false,
                                            stale: false,
                                            tags: vec!["mock_factory_variant".to_string(), "branch_coverage".to_string()],
                                            mocks,
                                            setup_calls: Vec::new(),
                                            invoke_returned_args: None,
                                            factory: default_factory.clone(),
                                            constructor_overrides: BTreeMap::new(),
                                            metadata_seeds: Vec::new(),
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
                            }
                        }
                    }
                }
            }
        }

        // 1f. Metadata seeding scenarios (for Reflect.getMetadata / reflector.get targets)
        if !target.metadata_keys.is_empty() {
            let mut seeds = Vec::new();
            let mut class_meta = Map::new();
            for raw_key in &target.metadata_keys {
                let resolved_key = index.resolve_constant(raw_key);
                let val = infer_metadata_value(&resolved_key);
                class_meta.insert(resolved_key.clone(), val.clone());
                seeds.push(MetadataSeed {
                    key: resolved_key,
                    value: val,
                    target_param: None,
                    on_method: None,
                });
            }

            let mut meta_inputs = Map::new();
            for p in &target.signature.parameters {
                let pn = p.name.to_lowercase();
                let pt = p.param_type.to_lowercase();
                if pn.contains("prototype") {
                    meta_inputs.insert(p.name.clone(), json!({
                        "$type": "DecoratedClass",
                        "kind": "prototype",
                        "metadata": class_meta,
                        "method_name": "handle",
                        "method_metadata": class_meta
                    }));
                } else if pn.contains("instance") || pn.contains("controller") {
                    meta_inputs.insert(p.name.clone(), json!({
                        "$type": "DecoratedClass",
                        "kind": "instance",
                        "metadata": class_meta,
                        "method_name": "handle",
                        "method_metadata": class_meta
                    }));
                } else if pn == "method" || pn == "methodname" || pn == "propertykey" {
                    meta_inputs.insert(p.name.clone(), json!("handle"));
                } else if pn == "target" || pn == "metatype" || pn == "type" || pt.contains("type<") || pt == "function" {
                    meta_inputs.insert(p.name.clone(), json!({
                        "$type": "DecoratedClass",
                        "kind": "class",
                        "metadata": class_meta,
                        "method_name": "handle",
                        "method_metadata": class_meta
                    }));
                } else if pn == "targets" || pt.contains("type<any>[]") || pt.contains("function)[]") {
                    meta_inputs.insert(p.name.clone(), json!([{
                        "$type": "DecoratedClass",
                        "kind": "class",
                        "metadata": class_meta,
                        "method_name": "handle",
                        "method_metadata": class_meta
                    }]));
                } else {
                    meta_inputs.insert(p.name.clone(), synthesize_value_for_param(p, index, false, is_php, 0));
                }
            }

            let sc_name = format!("{}_with_metadata", target_prefix);
            if !scenarios.iter().any(|s| s.name == sc_name) {
                scenarios.push(ScenarioDefinition {
                    name: sc_name,
                    mode: ScenarioMode::SymbolicContract,
                    locked: false,
                    stale: false,
                    tags: vec!["metadata_seed".to_string(), "branch_coverage".to_string()],
                    mocks: Vec::new(),
                    setup_calls: Vec::new(),
                    invoke_returned_args: None,
                    factory: default_factory.clone(),
                    constructor_overrides: BTreeMap::new(),
                    metadata_seeds: seeds,
                    inputs: Value::Object(meta_inputs),
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

        // 2. Guard-clause / negative test scenarios
        for param in &target.signature.parameters {
            for constraint in &param.constraints {
                if let Some((invalid_val, exc_name)) = generate_negative_sample(&param.param_type, constraint) {
                    let mut inputs = Map::new();
                    for other in &target.signature.parameters {
                        if other.name == param.name {
                            inputs.insert(param.name.clone(), invalid_val.clone());
                        } else {
                            inputs.insert(other.name.clone(), synthesize_value_for_param(other, index, false, is_php, 0));
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
                        factory: default_factory.clone(),
                        constructor_overrides: BTreeMap::new(),
                        metadata_seeds: Vec::new(),
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

    /// Solves all targets in a document (without cross-file index)
    pub fn solve_document(&self, doc: &mut ScenarioDocument) {
        self.solve_document_with_index(doc, &ProjectIndex::default());
    }

    /// Solves all targets in a document using cross-file ProjectIndex:
    /// - Single-class multi-mutator stateful call chains (`setup_calls`)
    /// - Multi-class producer -> consumer chains (`constructor_overrides`)
    pub fn solve_document_with_index(&self, doc: &mut ScenarioDocument, index: &ProjectIndex) {
        let source_file = doc.source.file.clone();
        let is_php = source_file.ends_with(".php");

        // Resolve inter-procedural forwarded and lookup-table constraints before solving targets
        for target in &mut doc.targets {
            for p in &mut target.signature.parameters {
                p.constraints = index.resolve_dynamic_constraints(&p.constraints, 0);
            }
            for p in &mut target.signature.constructor_args {
                p.constraints = index.resolve_dynamic_constraints(&p.constraints, 0);
            }
            for f in &mut target.signature.static_factories {
                for p in &mut f.parameters {
                    p.constraints = index.resolve_dynamic_constraints(&p.constraints, 0);
                }
            }
        }

        for target in &mut doc.targets {
            let new_scenarios = self.solve_target_with_index(target, &source_file, index);
            if !new_scenarios.is_empty() {
                target.scenarios = new_scenarios;
            }
        }

        // Collect up to 3 complementary mutators per class
        let mut class_mutators: BTreeMap<String, Vec<(String, Vec<Value>)>> = BTreeMap::new();
        for target in &doc.targets {
            if target.kind != TargetKind::Method {
                continue;
            }
            if Self::has_unmockable_side_effect(&target.detected_side_effects).is_some() {
                continue;
            }
            if let Some((cls, method)) = split_class_method(&target.id) {
                if is_mutator_method(method) {
                    let list = class_mutators.entry(cls.to_string()).or_default();
                    if list.len() < 3 && !list.iter().any(|(m, _)| m == method) {
                        let args: Vec<Value> = target
                            .signature
                            .parameters
                            .iter()
                            .map(|p| synthesize_value_for_param(p, index, true, is_php, 0))
                            .collect();
                        list.push((method.to_string(), args));
                    }
                }
            }
        }

        for target in &mut doc.targets {
            if target.kind != TargetKind::Method || target.scenarios.is_empty() {
                continue;
            }
            let target_prefix = target.id.replace("::", "_").replace('\\', "_").replace('.', "_");
            let base_factory = target.scenarios[0].factory.clone();

            // A. Single-class multi-mutator chains for reader/getter methods
            if let Some((cls, method)) = split_class_method(&target.id) {
                if is_reader_method(method) {
                    if let Some(mutators) = class_mutators.get(cls) {
                        let setup_calls: Vec<SetupCall> = mutators
                            .iter()
                            .filter(|(m, _)| m != method)
                            .map(|(m, args)| SetupCall {
                                method: m.clone(),
                                args: args.clone(),
                            })
                            .collect();
                        if !setup_calls.is_empty() {
                            let first_mut = &setup_calls[0].method;
                            let sc_name = format!("{}_after_{}", target_prefix, first_mut);
                            if !target.scenarios.iter().any(|s| s.name == sc_name) {
                                let (boundary_inputs, _) = self.generate_boundary_inputs_with_index(
                                    &target.signature.parameters,
                                    false,
                                    index,
                                    is_php,
                                );
                                target.scenarios.push(ScenarioDefinition {
                                    name: sc_name,
                                    mode: ScenarioMode::SymbolicContract,
                                    locked: false,
                                    stale: false,
                                    tags: vec!["stateful_chain".to_string(), "branch_coverage".to_string()],
                                    mocks: Vec::new(),
                                    setup_calls,
                                    invoke_returned_args: None,
                                    factory: base_factory.clone(),
                                    constructor_overrides: BTreeMap::new(),
                                    metadata_seeds: Vec::new(),
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

            // B. Cross-class producer -> consumer chains
            if !target.signature.constructor_args.is_empty() {
                let mut overrides = BTreeMap::new();
                for ctor_p in &target.signature.constructor_args {
                    let producer_entry = index
                        .resolve_implementer(&ctor_p.param_type)
                        .or_else(|| index.resolve_class(&ctor_p.param_type));
                    if let Some(producer) = producer_entry {
                        if let Some(producer_dto) = build_producer_dto(producer, index, is_php) {
                            overrides.insert(ctor_p.name.clone(), producer_dto);
                        }
                    }
                }

                if !overrides.is_empty() {
                    let sc_name = format!("{}_with_producer_chain", target_prefix);
                    if !target.scenarios.iter().any(|s| s.name == sc_name) {
                        let (nominal_inputs, _) = self.generate_boundary_inputs_with_index(
                            &target.signature.parameters,
                            false,
                            index,
                            is_php,
                        );
                        target.scenarios.push(ScenarioDefinition {
                            name: sc_name,
                            mode: ScenarioMode::SymbolicContract,
                            locked: false,
                            stale: false,
                            tags: vec!["producer_chain".to_string(), "branch_coverage".to_string()],
                            mocks: Vec::new(),
                            setup_calls: Vec::new(),
                            invoke_returned_args: None,
                            factory: base_factory.clone(),
                            constructor_overrides: overrides.clone(),
                            metadata_seeds: Vec::new(),
                            inputs: Value::Object(nominal_inputs.clone()),
                            expected: ExpectedOutcome {
                                outcome: OutcomeKind::Return,
                                expected_type: target.signature.return_type.clone(),
                                value: target.signature.return_type.as_deref().map(default_return_for_type),
                                exception_type: None,
                                message_contains: None,
                                expected_events: Vec::new(),
                            },
                        });

                        let (alt_inputs, _) = self.generate_boundary_inputs_with_index(
                            &target.signature.parameters,
                            true,
                            index,
                            is_php,
                        );
                        if !alt_inputs.is_empty() && alt_inputs != nominal_inputs {
                            let alt_name = format!("{}_with_producer_chain_alt", target_prefix);
                            if !target.scenarios.iter().any(|s| s.name == alt_name) {
                                target.scenarios.push(ScenarioDefinition {
                                    name: alt_name,
                                    mode: ScenarioMode::SymbolicContract,
                                    locked: false,
                                    stale: false,
                                    tags: vec!["producer_chain".to_string(), "branch_coverage".to_string()],
                                    mocks: Vec::new(),
                                    setup_calls: Vec::new(),
                                    invoke_returned_args: None,
                                    factory: base_factory,
                                    constructor_overrides: overrides,
                                    metadata_seeds: Vec::new(),
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
                        }
                    }
                }
            }
        }
    }

    fn generate_boundary_inputs_with_index(
        &self,
        params: &[ParameterDefinition],
        alt: bool,
        index: &ProjectIndex,
        is_php: bool,
    ) -> (Map<String, Value>, String) {
        let mut map = Map::new();
        for p in params {
            let val = synthesize_value_for_param(p, index, alt, is_php, 0);
            map.insert(p.name.clone(), val);
        }
        (map, if alt { "alt" } else { "nominal" }.to_string())
    }
}

fn make_inline_mock(target: &str, return_value: Value) -> MockReferenceOrInline {
    MockReferenceOrInline::Inline(FixtureDefinition {
        target: target.to_string(),
        when_args: Vec::new(),
        returns: vec![FixtureReturn {
            return_value: Some(return_value),
            exception: None,
        }],
        mutate_args: BTreeMap::new(),
        expect_calls: None,
    })
}

fn choose_best_static_factory<'a>(
    factories: &'a [ crate::ir::FactorySignature ],
    class_short_name: &str,
) -> Option<&'a crate::ir::FactorySignature> {
    if factories.is_empty() {
        return None;
    }
    let is_non_recursive = |f: &&crate::ir::FactorySignature| -> bool {
        if class_short_name.is_empty() {
            return true;
        }
        !f.parameters.iter().any(|p| {
            let pt = p.param_type.trim();
            pt == class_short_name || pt.ends_with(&format!("\\{}", class_short_name))
        })
    };
    let priority = [
        "fromSuccess", "of", "fromString", "new", "create", "get", "from", "fromISO", "zero", "fromFailure",
    ];
    for name in priority {
        if let Some(f) = factories.iter().filter(is_non_recursive).find(|f| f.name == name) {
            return Some(f);
        }
    }
    if let Some(f) = factories.iter().find(is_non_recursive) {
        return Some(f);
    }
    for name in priority {
        if let Some(f) = factories.iter().find(|f| f.name == name) {
            return Some(f);
        }
    }
    factories.first()
}

fn build_producer_dto(
    producer: &ClassIndexEntry,
    index: &ProjectIndex,
    is_php: bool,
) -> Option<Value> {
    if producer.private_constructor && producer.static_factories.is_empty() {
        return None;
    }
    let mut setup_calls = Vec::new();
    for (m_name, m_params) in producer.mutators.iter().take(3) {
        let args: Vec<Value> = m_params
            .iter()
            .map(|p| synthesize_value_for_param(p, index, true, is_php, 1))
            .collect();
        setup_calls.push(json!({
            "method": m_name,
            "args": args
        }));
    }

    let mut dto = if producer.private_constructor {
        let factory = choose_best_static_factory(&producer.static_factories, &producer.short_name)?;
        let fargs: Vec<Value> = factory
            .parameters
            .iter()
            .map(|p| synthesize_value_for_param(p, index, false, is_php, 1))
            .collect();
        json!({
            "$type": producer.name,
            "$factory": factory.name,
            "$ctor": fargs,
        })
    } else {
        let ctor_args: Vec<Value> = producer
            .constructor_params
            .iter()
            .map(|p| synthesize_value_for_param(p, index, true, is_php, 1))
            .collect();
        if setup_calls.is_empty() && ctor_args.is_empty() {
            return None;
        }
        json!({
            "$type": producer.name,
            "$ctor": ctor_args,
        })
    };
    if !setup_calls.is_empty() {
        dto["$setup_calls"] = Value::Array(setup_calls);
    }
    if !is_php && !producer.source_file.is_empty() {
        dto["$source_file"] = json!(producer.source_file);
    }
    Some(dto)
}

fn synthesize_object_from_prop_constraints(constraints: &[String]) -> Value {
    let mut obj = Map::new();
    let mut bool_hints: BTreeMap<String, bool> = BTreeMap::new();
    let mut arr_hints: HashSet<String> = HashSet::new();

    for c in constraints {
        if let Some(rest) = c.strip_prefix("prop_bool:") {
            if let Some((field, val_str)) = rest.split_once(':') {
                bool_hints.insert(field.to_string(), val_str == "true");
            }
        } else if let Some(field) = c.strip_prefix("prop_arr:") {
            arr_hints.insert(field.to_string());
        }
    }

    for c in constraints {
        if let Some(field) = c.strip_prefix("prop:") {
            if obj.contains_key(field) {
                continue;
            }
            let fl = field.to_lowercase();
            let val = if let Some(&b) = bool_hints.get(field) {
                json!(b)
            } else if arr_hints.contains(field) || fl.ends_with('s') || fl.contains("list") {
                json!(["test"])
            } else if fl.contains("versioning") || fl.contains("options") || fl.contains("config") {
                json!({ "type": 0, "prefix": "v" })
            } else if fl.contains("version") {
                json!("1")
            } else if fl.contains("prefix") || fl.contains("path") {
                json!("/test")
            } else if fl.contains("is") || fl.contains("has") || fl.contains("enabled") {
                json!(true)
            } else if fl.contains("count") || fl.contains("port") || fl.contains("status") || fl.contains("code") {
                json!(1)
            } else {
                json!("test")
            };
            obj.insert(field.to_string(), val);
        }
    }
    Value::Object(obj)
}

fn synthesize_value_for_param(
    param: &ParameterDefinition,
    index: &ProjectIndex,
    alt: bool,
    is_php: bool,
    depth: usize,
) -> Value {
    if !alt {
        if let Some(ref def) = param.default_value {
            return def.clone();
        }
    }

    let resolved_constraints = index.resolve_dynamic_constraints(&param.constraints, 0);
    let is_variadic = resolved_constraints.iter().any(|c| c == "variadic");
    let t = param.param_type.trim();
    let t_lower = t.to_lowercase();
    let n_lower = param.name.to_lowercase();

    // Variadic union params (e.g. `Route|Group ...$routes` or `string ...$hosts`)
    if is_variadic && alt {
        if t.contains('|') {
            let items: Vec<Value> = t
                .split('|')
                .filter(|part| {
                    let pl = part.trim().to_lowercase();
                    pl != "null" && pl != "undefined"
                })
                .take(2)
                .map(|part| synthesize_value_for_type(part.trim(), &param.name, index, true, is_php, depth + 1))
                .collect();
            if !items.is_empty() {
                return Value::Array(items);
            }
        }
        let elem = synthesize_value_for_type(t, &param.name, index, true, is_php, depth + 1);
        return json!([elem]);
    }

    // Object property constraints when param is an interface/object/any (skip if param name clearly indicates a primitive)
    let has_prim_name_hint = n_lower.contains("alphabet")
        || n_lower.contains("size")
        || n_lower.contains("bytes")
        || n_lower.contains("length")
        || n_lower.contains("count")
        || n_lower.contains("limit")
        || n_lower.contains("port")
        || n_lower.contains("name")
        || n_lower.contains("path")
        || n_lower.contains("url")
        || n_lower.contains("key")
        || n_lower.contains("pattern")
        || n_lower.contains("prefix")
        || n_lower.contains("host");
    if alt
        && (t_lower != "any" || !has_prim_name_hint)
        && resolved_constraints
            .iter()
            .any(|c| c.starts_with("prop:") || c.starts_with("prop_bool:") || c.starts_with("prop_arr:"))
    {
        return synthesize_object_from_prop_constraints(&resolved_constraints);
    }

    // Check parameter constraints for exact string length (`length == N` / `len == N`)
    if t_lower.contains("string") || t_lower == "mixed" || t_lower == "any" {
        for c in &resolved_constraints {
            if let Some(num_str) = c.strip_prefix("length == ").or_else(|| c.strip_prefix("len == ")) {
                if let Ok(len) = num_str.trim().parse::<usize>() {
                    if len == 2 && !alt {
                        return json!("DE");
                    } else if len > 0 && len <= 1024 && (!alt || depth > 0) {
                        return json!("0".repeat(len));
                    }
                }
            }
        }
        // Valid domain samples parsed from regex guards or lookup tables (`valid:...`)
        if !alt || depth > 0 {
            if let Some(valid_sample) = resolved_constraints.iter().find_map(|c| c.strip_prefix("valid:")) {
                return json!(valid_sample);
            }
        }
        if depth > 0 {
            if let Some(eq_sample) = resolved_constraints.iter().find_map(|c| c.strip_prefix("eq:")) {
                return json!(eq_sample);
            }
        }
    }
    if !alt && (t_lower.contains("float") || t_lower.contains("double")) && resolved_constraints.iter().any(|c| c.contains(">= 0") || c.contains("> 0")) {
        return json!(0.0);
    }
    if !alt && (t_lower.contains("int") || t_lower.contains("number")) && resolved_constraints.iter().any(|c| c.contains(">= 0") || c.contains("> 0")) && !n_lower.contains("port") {
        return json!(0);
    }

    synthesize_value_for_type(t, &param.name, index, alt, is_php, depth)
}

fn synthesize_value_for_type(
    raw_type: &str,
    param_name: &str,
    index: &ProjectIndex,
    alt: bool,
    is_php: bool,
    depth: usize,
) -> Value {
    let t = raw_type.trim();
    let tl = t.to_lowercase();
    let nl = param_name.to_lowercase();

    // Always resolve primitives regardless of recursion depth
    if tl.contains("float") || tl.contains("double") {
        return if alt { json!(0.0) } else { json!(1.0) };
    }
    if tl == "int" || tl == "number" || tl == "integer" {
        if nl.contains("port") {
            return json!(0);
        }
        return if alt { json!(0) } else { json!(1) };
    }
    if tl == "string" {
        if nl.contains("alphabet") {
            return if alt { json!("abcde") } else { json!("abcdef0123456789") };
        }
        if nl.contains("host") {
            return if alt { json!("example.com/") } else { json!("example.com") };
        }
        if nl.contains("pattern") || nl.contains("path") || nl.contains("prefix") {
            return if alt { json!("/users/{id}") } else { json!("/test") };
        }
        return if alt { json!("") } else { json!("test") };
    }
    if tl == "bool" || tl == "boolean" {
        return json!(!alt);
    }

    if tl == "null" || tl == "undefined" || tl == "void" || tl == "mixed" {
        return json!(null);
    }

    if depth > 3 {
        return json!(null);
    }

    // Nullable: nominal keeps simple scalar or null if mixed; alt synthesizes non-null inner type
    if t.starts_with('?') {
        let inner = &t[1..];
        if !alt && (inner.contains('\\') || index.resolve_class(inner).is_some()) {
            return json!(null);
        }
        return synthesize_value_for_type(inner, param_name, index, alt, is_php, depth);
    }

    // Union of array element types (e.g. `Group[]|Route[]`, `array[]|callable[]|string[]`)
    if t.contains("[]") && t.contains('|') {
        if !alt {
            return json!([]);
        }
        let mut items = Vec::new();
        for part in t.split('|').take(2) {
            let p = part.trim().trim_end_matches("[]").trim();
            if !p.is_empty() && p != "null" {
                items.push(synthesize_value_for_type(p, param_name, index, true, is_php, depth + 1));
            }
        }
        return Value::Array(items);
    }

    // Map<K, V>
    if tl.starts_with("map<") {
        if !alt {
            return json!({ "$type": "Map", "entries": [] });
        }
        return json!({ "$type": "Map", "entries": [["test", {}]] });
    }

    // Set<T>
    if tl.starts_with("set<") {
        if !alt {
            return json!({ "$type": "Set", "values": [] });
        }
        return json!({ "$type": "Set", "values": ["test"] });
    }

    // Typed associative arrays / Records
    if tl.starts_with("array<string,") || tl.starts_with("record<") {
        if !alt {
            return json!({});
        }
        return json!({ "id": "1" });
    }

    // Typed array / list<T> / Array<T>
    if tl.ends_with("[]")
        || tl.starts_with("array<")
        || tl.starts_with("list<")
        || tl.starts_with("readonlyarray<")
        || (tl.starts_with("readonly ") && tl.ends_with("[]"))
    {
        if !alt {
            return json!([]);
        }
        let elem_type = if tl.ends_with("[]") {
            t.trim_start_matches("readonly ").trim().trim_end_matches("[]").trim()
        } else if let Some(inner) = t.split('<').nth(1).and_then(|s| s.strip_suffix('>')) {
            inner.trim()
        } else {
            "string"
        };
        let elem = synthesize_value_for_type(elem_type, param_name, index, true, is_php, depth + 1);
        return json!([elem]);
    }

    if tl == "array" || tl == "iterable" {
        if !alt {
            return json!([]);
        }
        if nl.contains("method") {
            return json!(["GET"]);
        }
        return json!(["test"]);
    }

    // Callbacks / closures / functions (checked AFTER array/collection types so `callable[]` stays an array)
    if tl.contains("callable")
        || tl.contains("closure")
        || tl.contains("function")
        || tl.contains("=>")
        || (tl == "any" && (nl == "getrandom" || nl == "callback" || nl == "cb" || nl == "fn" || nl == "handler" || nl == "predicate" || nl == "next"))
    {
        if nl == "getrandom" {
            return json!({
                "$type": "Callback",
                "returns": {
                    "$type": "Uint8Array",
                    "data": [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32]
                }
            });
        } else if nl == "predicate" {
            return json!({ "$type": "Callback", "returns": true });
        }
        return json!({ "$type": "Callback", "returns": null });
    }

    // Union type without array suffix (e.g. `Route|Group`, `string|null`, `string|BackedEnum`, `BigNumber|int|string`)
    if t.contains('|') {
        let candidates: Vec<&str> = t
            .split('|')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty() && !s.eq_ignore_ascii_case("null") && !s.eq_ignore_ascii_case("undefined"))
            .collect();
        let is_known_or_primitive = |c: &str| -> bool {
            let cl = c.to_lowercase();
            matches!(
                cl.as_str(),
                "int" | "integer" | "number" | "float" | "double" | "string" | "bool" | "boolean" | "array"
            ) || index.resolve_class(c).is_some()
        };
        if let Some(chosen) = if alt && candidates.len() > 1 {
            candidates.get(1).copied()
        } else {
            candidates
                .iter()
                .copied()
                .find(|c| is_known_or_primitive(c))
                .or_else(|| candidates.first().copied())
        } {
            return synthesize_value_for_type(chosen, param_name, index, alt, is_php, depth);
        }
    }

    // Known concrete class in ProjectIndex
    if let Some(cls) = index.resolve_class(t) {
        if cls.private_constructor && cls.static_factories.is_empty() {
            return json!({ "$type": "AutoMock", "class": cls.name });
        }
        let mut dto = if let Some(factory) = choose_best_static_factory(&cls.static_factories, &cls.short_name) {
            let fargs: Vec<Value> = factory
                .parameters
                .iter()
                .map(|p| synthesize_value_for_param(p, index, false, is_php, depth + 1))
                .collect();
            json!({
                "$type": cls.name,
                "$factory": factory.name,
                "$ctor": fargs
            })
        } else {
            let cargs: Vec<Value> = cls
                .constructor_params
                .iter()
                .map(|p| synthesize_value_for_param(p, index, false, is_php, depth + 1))
                .collect();
            json!({
                "$type": cls.name,
                "$ctor": cargs
            })
        };
        if alt && depth <= 2 && !cls.mutators.is_empty() {
            let mut setup_calls = Vec::new();
            for (m_name, m_params) in cls.mutators.iter().take(3) {
                let args: Vec<Value> = m_params
                    .iter()
                    .map(|p| synthesize_value_for_param(p, index, false, is_php, depth + 1))
                    .collect();
                setup_calls.push(json!({ "method": m_name, "args": args }));
            }
            if !setup_calls.is_empty() {
                dto["$setup_calls"] = Value::Array(setup_calls);
            }
        }
        if !is_php && !cls.source_file.is_empty() {
            dto["$source_file"] = json!(cls.source_file);
        }
        return dto;
    }

    // Known interface in ProjectIndex
    if alt {
        if let Some(impl_cls) = index.resolve_implementer(t) {
            return synthesize_value_for_type(&impl_cls.name, param_name, index, false, is_php, depth + 1);
        }
    }
    if index.interface_methods.contains_key(t.trim_start_matches('\\')) || t.ends_with("Interface") {
        return json!({ "$type": "AutoMock", "class": t.trim_start_matches('\\') });
    }

    if tl == "any" {
        if nl.contains("alphabet") {
            return if alt { json!("abcde") } else { json!("abcdef0123456789") };
        }
        if nl.contains("size") || nl.contains("bytes") || nl.contains("length") || nl.contains("count") || nl.contains("limit") {
            return if alt { json!(0) } else { json!(16) };
        }
        if nl.contains("port") {
            return json!(0);
        }
        if nl.contains("name") || nl.contains("path") || nl.contains("url") || nl.contains("key") || nl.contains("pattern") || nl.contains("prefix") || nl.contains("host") {
            return json!("test");
        }
        return json!({});
    }

    // Unindexed non-primitive PHP/TS class/interface name or inline structural object with methods -> AutoMock
    if t.contains('\\')
        || t.chars().next().map(|c| c.is_uppercase()).unwrap_or(false)
        || (t.starts_with('{') && t.contains('('))
    {
        return json!({ "$type": "AutoMock", "class": t.trim_start_matches('\\') });
    }

    json!({})
}

fn infer_metadata_value(key: &str) -> Value {
    let k = key.to_lowercase();
    if k.contains("path") || k.contains("host") || k.contains("prefix") {
        json!("/test")
    } else if k.contains("method") || k.contains("code") || k.contains("httpcode") {
        json!(0)
    } else if k.contains("version") {
        json!("1")
    } else if k.contains("watermark") || k.contains("global") || k.contains("passthrough") || k.contains("sse") {
        json!(true)
    } else if k.contains("guards")
        || k.contains("pipes")
        || k.contains("interceptors")
        || k.contains("filters")
        || k.contains("paramtypes")
        || k.contains("controllers")
        || k.contains("providers")
        || k.contains("imports")
        || k.contains("exports")
        || k.contains("headers")
    {
        json!([])
    } else {
        json!({ "key": key })
    }
}

fn sanitize_tag(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .collect()
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
        || m.starts_with("enable")
        || m.starts_with("useglobal")
        || m == "name"
        || m == "host"
        || m == "hosts"
        || m == "defaults"
        || m == "prefix"
        || m == "nameprefix"
        || m == "middleware"
        || m == "prependmiddleware"
        || m == "routes"
        || m == "withcors"
        || m == "action"
}

fn is_reader_method(method: &str) -> bool {
    let m = method.to_lowercase();
    m.starts_with("get")
        || m.starts_with("has")
        || m.starts_with("find")
        || m.starts_with("is")
        || m == "all"
        || m == "getdata"
        || m == "toarray"
        || m == "tobuffer"
        || m == "tostring"
        || m == "__tostring"
        || m == "__debuginfo"
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
