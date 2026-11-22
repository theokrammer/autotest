use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::Path;
use serde_json::json;
use autotest_core::{
    AstHashes, DependencyGraph, EntrypointScaffolder, ExpectedOutcome, FixtureDeduplicator,
    MergeMode, MockReferenceOrInline, OutcomeKind, ParameterDefinition, ScenarioDefinition,
    ScenarioDocument, ScenarioMerger, ScenarioMode, SignatureDefinition, SourceMetadata,
    TargetDefinition, TargetKind,
};

fn get_conformance_dir() -> &'static Path {
    let dir = Path::new("../../conformance/core");
    let fallback = Path::new("conformance/core");
    if dir.exists() { dir } else { fallback }
}

fn load_conformance_scenario(name: &str) -> ScenarioDocument {
    let path = get_conformance_dir().join(format!("{}.autotest.json", name));
    let content = fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("Could not read scenario file {}: {}", path.display(), e));
    serde_json::from_str::<ScenarioDocument>(&content)
        .unwrap_or_else(|e| panic!("Could not parse scenario JSON {}: {}", path.display(), e))
}

fn create_base_doc(id: &str, file: &str) -> ScenarioDocument {
    ScenarioDocument {
        schema: Some("https://autotest.dev/schema/scenario.v1.schema.json".to_string()),
        schema_version: "1.0".to_string(),
        source: SourceMetadata {
            file: file.to_string(),
            content_hash: "sha256:dummyhash".to_string(),
            analyzer: "test-analyzer".to_string(),
            dependencies: BTreeMap::new(),
        },
        fixtures: BTreeMap::new(),
        targets: vec![TargetDefinition {
            id: id.to_string(),
            kind: TargetKind::Method,
            signature: SignatureDefinition::default(),
            detected_side_effects: Vec::new(),
            scenarios: Vec::new(),
        }],
    }
}

#[test]
fn test_conformance_01_ref_resolution_single() {
    let doc = load_conformance_scenario("01_ref_resolution_single");
    assert!(doc.fixtures.contains_key("mock_tax_repo"));
    let mock = &doc.targets[0].scenarios[0].mocks[0];
    if let MockReferenceOrInline::Reference { reference } = mock {
        let resolved = FixtureDeduplicator::resolve_ref(&doc, reference);
        assert!(resolved.is_some());
        assert_eq!(resolved.unwrap().target, "TaxRepository::getRate");
    } else {
        panic!("Expected MockReferenceOrInline::Reference");
    }
}

#[test]
fn test_conformance_02_ref_resolution_multiple() {
    let doc = load_conformance_scenario("02_ref_resolution_multiple");
    assert!(doc.fixtures.contains_key("mock_tax_repo"));
    assert!(doc.fixtures.contains_key("mock_currency_converter"));
    assert_eq!(doc.targets[0].scenarios[0].mocks.len(), 2);

    for mock in &doc.targets[0].scenarios[0].mocks {
        if let MockReferenceOrInline::Reference { reference } = mock {
            let resolved = FixtureDeduplicator::resolve_ref(&doc, reference);
            assert!(resolved.is_some());
        } else {
            panic!("Expected MockReferenceOrInline::Reference");
        }
    }
}

#[test]
fn test_conformance_03_04_05_hydration_metas() {
    let doc_ctor = load_conformance_scenario("03_ctor_hydration");
    let ctor_input = &doc_ctor.targets[0].scenarios[0].inputs["money"];
    assert_eq!(ctor_input["$type"], "App\\ValueObject\\Money");
    assert!(ctor_input["$ctor"].is_array());

    let doc_factory = load_conformance_scenario("04_factory_hydration");
    let factory_input = &doc_factory.targets[0].scenarios[0].inputs["money"];
    assert_eq!(factory_input["$factory"], "fromCents");

    let doc_props = load_conformance_scenario("05_props_hydration");
    let props_input = &doc_props.targets[0].scenarios[0].inputs["profile"];
    assert_eq!(props_input["$props"]["id"], 42);
}

#[test]
fn test_conformance_06_to_09_boundary_solver() {
    let doc_06 = load_conformance_scenario("06_boundary_zero_float");
    assert_eq!(doc_06.targets[0].scenarios[0].inputs["amount"], 0.0);

    let doc_07 = load_conformance_scenario("07_boundary_negative_guard");
    assert_eq!(doc_07.targets[0].scenarios[0].expected.outcome, OutcomeKind::Exception);
    assert_eq!(doc_07.targets[0].scenarios[0].expected.exception_type.as_deref(), Some("InvalidArgumentException"));

    let doc_08 = load_conformance_scenario("08_boundary_empty_string_guard");
    assert_eq!(doc_08.targets[0].scenarios[0].inputs["code"], "");
    assert_eq!(doc_08.targets[0].scenarios[0].expected.outcome, OutcomeKind::Exception);

    let doc_09 = load_conformance_scenario("09_boundary_fixed_length_string");
    assert_eq!(doc_09.targets[0].scenarios[0].inputs["countryCode"], "DE");
}

#[test]
fn test_conformance_10_to_13_argument_matchers() {
    let doc_10 = load_conformance_scenario("10_matcher_exact");
    let f10 = doc_10.fixtures.get("mock_exact_order").unwrap();
    assert_eq!(f10.when_args[0]["$matcher"], "exact");

    let doc_11 = load_conformance_scenario("11_matcher_regex");
    let f11 = doc_11.fixtures.get("mock_email_sender").unwrap();
    assert_eq!(f11.when_args[0]["$matcher"], "regex");

    let doc_12 = load_conformance_scenario("12_matcher_range");
    let f12 = doc_12.fixtures.get("mock_tier_calc").unwrap();
    assert_eq!(f12.when_args[0]["$matcher"], "range");

    let doc_13 = load_conformance_scenario("13_matcher_subset");
    let f13 = doc_13.fixtures.get("mock_auth_guard").unwrap();
    assert_eq!(f13.when_args[0]["$matcher"], "subset");
}

#[test]
fn test_conformance_14_to_16_mock_behaviors() {
    let doc_14 = load_conformance_scenario("14_mock_sequential_returns");
    let f14 = doc_14.fixtures.get("mock_token_sequence").unwrap();
    assert_eq!(f14.returns.len(), 3);
    assert_eq!(f14.expect_calls, Some(3));

    let doc_15 = load_conformance_scenario("15_mock_mutate_args");
    let f15 = doc_15.fixtures.get("mock_byte_reader").unwrap();
    assert!(!f15.mutate_args.is_empty());

    let doc_16 = load_conformance_scenario("16_mock_expect_calls");
    let f16 = doc_16.fixtures.get("mock_audit_logger").unwrap();
    assert_eq!(f16.expect_calls, Some(2));
}

#[test]
fn test_conformance_17_to_19_expected_outcomes() {
    let doc_17 = load_conformance_scenario("17_exception_type");
    assert_eq!(doc_17.targets[0].scenarios[0].expected.outcome, OutcomeKind::Exception);
    assert_eq!(
        doc_17.targets[0].scenarios[0].expected.exception_type.as_deref(),
        Some("App\\Exception\\EntityNotFoundException")
    );

    let doc_18 = load_conformance_scenario("18_exception_message_contains");
    assert_eq!(doc_18.targets[0].scenarios[0].expected.outcome, OutcomeKind::Exception);
    assert_eq!(
        doc_18.targets[0].scenarios[0].expected.message_contains.as_deref(),
        Some("token has expired")
    );

    let doc_19 = load_conformance_scenario("19_no_crash_outcome");
    assert_eq!(doc_19.targets[0].scenarios[0].expected.outcome, OutcomeKind::NoCrash);
}

#[test]
fn test_conformance_20_to_23_merge_and_drift_detection() {
    let doc_20 = load_conformance_scenario("20_merge_preserve_locked");
    assert!(doc_20.targets[0].scenarios[0].locked);

    let doc_21 = load_conformance_scenario("21_merge_remove_unlocked");
    assert!(!doc_21.targets[0].scenarios[0].locked);

    let doc_22 = load_conformance_scenario("22_drift_param_removed");
    assert!(doc_22.targets[0].scenarios[0].stale);

    let doc_23 = load_conformance_scenario("23_drift_mock_removed");
    assert!(doc_23.targets[0].scenarios[0].stale);

    // Test merger drift detection programmatically
    let mut original = create_base_doc("InvoiceService::calc", "src/Invoice.php");
    original.targets[0].signature.parameters = vec![
        ParameterDefinition {
            name: "amount".to_string(),
            param_type: "float".to_string(),
            constraints: vec![],
            default_value: None,
        },
        ParameterDefinition {
            name: "countryCode".to_string(),
            param_type: "string".to_string(),
            constraints: vec![],
            default_value: None,
        },
    ];
    original.targets[0].scenarios.push(ScenarioDefinition {
        name: "custom_vat_override".to_string(),
        mode: ScenarioMode::Manual,
        locked: true,
        stale: false,
        tags: vec!["custom".to_string()],
        mocks: vec![],
        setup_calls: vec![],
        invoke_returned_args: None,
        inputs: json!({ "amount": 100.0, "countryCode": "DE" }),
        expected: ExpectedOutcome {
            outcome: OutcomeKind::Return,
            expected_type: None,
            value: Some(json!(119.0)),
            exception_type: None,
            message_contains: None,
            expected_events: vec![],
        },
    });

    let mut updated = original.clone();
    updated.targets[0].signature.parameters = vec![
        ParameterDefinition {
            name: "amount".to_string(),
            param_type: "float".to_string(),
            constraints: vec![],
            default_value: None,
        },
        ParameterDefinition {
            name: "country".to_string(),
            param_type: "string".to_string(),
            constraints: vec![],
            default_value: None,
        },
    ];

    let (merged, report) = ScenarioMerger::merge(Some(&original), &updated, MergeMode::NonDestructive);
    assert_eq!(report.retained_locked, 1);
    assert_eq!(report.stale_assertions.len(), 1);
    assert!(merged.targets[0].scenarios[0].stale);
    assert_eq!(report.stale_assertions[0].scenario_name, "custom_vat_override");

    // Test MergeMode::Force pruning of unlocked scenarios
    let mut existing_unlocked = create_base_doc("PruneService::run", "src/Prune.php");
    existing_unlocked.targets[0].scenarios.push(ScenarioDefinition {
        name: "unlocked_case".to_string(),
        mode: ScenarioMode::SymbolicContract,
        locked: false,
        stale: false,
        tags: vec![],
        mocks: vec![],
        setup_calls: vec![],
        invoke_returned_args: None,
        inputs: json!({}),
        expected: ExpectedOutcome {
            outcome: OutcomeKind::Return,
            expected_type: None,
            value: None,
            exception_type: None,
            message_contains: None,
            expected_events: vec![],
        },
    });

    let (_, force_report) = ScenarioMerger::merge(Some(&existing_unlocked), &create_base_doc("PruneService::run", "src/Prune.php"), MergeMode::Force);
    assert_eq!(force_report.removed_unlocked, 1);
}

#[test]
fn test_conformance_24_25_cache_invalidation_and_tarjan_scc() {
    let doc_24 = load_conformance_scenario("24_cache_transitive_invalidation");
    assert!(!doc_24.source.dependencies.is_empty());

    let doc_25 = load_conformance_scenario("25_cache_cyclic_tarjan_scc");
    assert!(doc_25.source.dependencies.len() >= 2);

    // Verify AstHashes computation
    let hashes = AstHashes::compute("function test() { return 42; }", "public function test(): int;");
    assert!(hashes.impl_hash.starts_with("sha256:"));
    assert!(hashes.public_contract_hash.starts_with("contract_sha256:"));

    let mut graph = DependencyGraph::new();

    // Cyclic dependency: Order <-> OrderItem
    let mut order_deps = BTreeMap::new();
    order_deps.insert("OrderItem".to_string(), "contract_sha256:hash1".to_string());
    graph.record_dependencies("Order", order_deps);

    let mut item_deps = BTreeMap::new();
    item_deps.insert("Order".to_string(), "contract_sha256:hash2".to_string());
    graph.record_dependencies("OrderItem", item_deps);

    let cycles = graph.find_cycles();
    assert!(!cycles.is_empty(), "Tarjan SCC find_cycles must identify cyclic component");

    // Test invalidation
    let mut changed = HashMap::new();
    changed.insert("OrderItem".to_string(), "contract_sha256:new_hash".to_string());
    let invalidated = graph.find_invalidated_files(&changed);
    assert!(invalidated.contains("Order"));
}

#[test]
fn test_all_25_conformance_core_files_exist_and_validate() {
    let target_dir = get_conformance_dir();
    assert!(target_dir.exists(), "conformance/core directory must exist");

    let entries = fs::read_dir(target_dir).expect("read conformance dir");
    let mut count = 0;
    for entry in entries {
        let entry = entry.expect("valid entry");
        let path = entry.path();
        if path.is_file() && path.extension().and_then(|s| s.to_str()) == Some("json") {
            let content = fs::read_to_string(&path).expect("read file");
            let doc: ScenarioDocument = serde_json::from_str(&content)
                .unwrap_or_else(|e| panic!("Failed to parse {}: {}", path.display(), e));
            assert_eq!(doc.schema_version, "1.0");
            assert!(!doc.targets.is_empty());
            count += 1;
        }
    }
    assert_eq!(count, 25, "Expected exactly 25 conformance reference scenarios");
}

#[test]
fn test_php_profile_conformance_scenarios_validate() {
    let dir = Path::new("../../conformance/profiles/php");
    let fallback_dir = Path::new("conformance/profiles/php");
    let target_dir = if dir.exists() { dir } else { fallback_dir };
    assert!(target_dir.exists(), "conformance/profiles/php directory must exist");

    let entries = fs::read_dir(target_dir).expect("read php profile dir");
    let mut count = 0;
    for entry in entries {
        let entry = entry.expect("valid entry");
        let path = entry.path();
        if path.is_file() && path.extension().and_then(|s| s.to_str()) == Some("json") {
            let content = fs::read_to_string(&path).expect("read file");
            let doc: ScenarioDocument = serde_json::from_str(&content)
                .unwrap_or_else(|e| panic!("Failed to parse {}: {}", path.display(), e));
            assert_eq!(doc.schema_version, "1.0");
            assert!(!doc.targets.is_empty());
            count += 1;
        }
    }
    assert!(count >= 5, "Expected at least 5 PHP profile scenarios");
}

#[test]
fn test_bridge_scaffolding() {
    let php_bridge = EntrypointScaffolder::generate_phpunit_bridge("tests/scenarios");
    assert!(php_bridge.contains("#[DataProvider('scenarioProvider')]"));
    assert!(php_bridge.contains("class AutotestBridgeTest"));

    let ts_bridge = EntrypointScaffolder::generate_ts_bridge("tests/scenarios", true);
    assert!(ts_bridge.contains("import { describe, it, expect, vi } from 'vitest';"));
    assert!(ts_bridge.contains("it.each(scenarios)"));
}
