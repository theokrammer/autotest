use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use serde_json::json;
use autotest_core::{
    DetectedSideEffect, ExpectedOutcome, FixtureDefinition, FixtureReturn, MockReferenceOrInline,
    OutcomeKind, ParameterDefinition, ScenarioDefinition, ScenarioDocument, ScenarioMode,
    SignatureDefinition, SourceMetadata, TargetDefinition, TargetKind,
};

fn main() -> anyhow::Result<()> {
    let out_dir = Path::new("conformance/core");
    fs::create_dir_all(out_dir)?;

    let docs = build_all_25_conformance_docs();
    for (file_id, doc) in docs {
        let file_path = out_dir.join(format!("{}.autotest.json", file_id));
        let content = serde_json::to_string_pretty(&doc)?;
        fs::write(file_path, content)?;
    }

    println!("Generated 25 authentic conformance scenarios in {}", out_dir.display());
    Ok(())
}

fn create_doc(
    _file_id: &str,
    file_path: &str,
    fixtures: BTreeMap<String, FixtureDefinition>,
    target_id: &str,
    parameters: Vec<ParameterDefinition>,
    return_type: Option<String>,
    detected_side_effects: Vec<DetectedSideEffect>,
    scenarios: Vec<ScenarioDefinition>,
    dependencies: BTreeMap<String, String>,
) -> ScenarioDocument {
    ScenarioDocument {
        schema: Some("https://autotest.dev/schema/scenario.v1.schema.json".to_string()),
        schema_version: "1.0".to_string(),
        source: SourceMetadata {
            file: file_path.to_string(),
            content_hash: "sha256:0123456789abcdef0123456789abcdef".to_string(),
            analyzer: "conformance-harness@1.0.0".to_string(),
            dependencies,
        },
        fixtures,
        targets: vec![TargetDefinition {
            id: target_id.to_string(),
            kind: TargetKind::Method,
            signature: SignatureDefinition {
                constructor_args: vec![],
                parameters,
                return_type,
            },
            detected_side_effects,
            scenarios,
        }],
    }
}

pub fn build_all_25_conformance_docs() -> Vec<(&'static str, ScenarioDocument)> {
    vec![
        // 01: Single $ref resolution
        (
            "01_ref_resolution_single",
            {
                let mut fixtures = BTreeMap::new();
                fixtures.insert("mock_tax_repo".to_string(), FixtureDefinition {
                    target: "TaxRepository::getRate".to_string(),
                    when_args: vec![json!("DE")],
                    returns: vec![FixtureReturn {
                        return_value: Some(json!(0.19)),
                        exception: None,
                    }],
                    mutate_args: BTreeMap::new(),
                    expect_calls: Some(1),
                });
                create_doc(
                    "01_ref_resolution_single",
                    "src/Tax/TaxCalculator.php",
                    fixtures,
                    "App\\Tax\\TaxCalculator::calculateTax",
                    vec![
                        ParameterDefinition {
                            name: "country".to_string(),
                            param_type: "string".to_string(),
                            constraints: vec!["non-empty-string".to_string()],
                            default_value: None,
                        },
                        ParameterDefinition {
                            name: "netAmount".to_string(),
                            param_type: "float".to_string(),
                            constraints: vec![">= 0.0".to_string()],
                            default_value: None,
                        },
                    ],
                    Some("float".to_string()),
                    vec![DetectedSideEffect {
                        effect_type: "method_call".to_string(),
                        target: "TaxRepository::getRate".to_string(),
                        mockable: true,
                    }],
                    vec![ScenarioDefinition {
                        name: "calculate_tax_de".to_string(),
                        mode: ScenarioMode::SymbolicContract,
                        locked: false,
                        stale: false,
                        tags: vec!["tax".to_string(), "ref_resolution".to_string()],
                        mocks: vec![MockReferenceOrInline::Reference {
                            reference: "#/fixtures/mock_tax_repo".to_string(),
                        }],
                        setup_calls: vec![],
                        invoke_returned_args: None,
                        inputs: json!({"country": "DE", "netAmount": 100.0}),
                        expected: ExpectedOutcome {
                            outcome: OutcomeKind::Return,
                            expected_type: Some("float".to_string()),
                            value: Some(json!(19.0)),
                            exception_type: None,
                            message_contains: None,
                            expected_events: vec![],
                        },
                    }],
                    BTreeMap::new(),
                )
            },
        ),

        // 02: Multiple $ref resolution
        (
            "02_ref_resolution_multiple",
            {
                let mut fixtures = BTreeMap::new();
                fixtures.insert("mock_tax_repo".to_string(), FixtureDefinition {
                    target: "TaxRepository::getRate".to_string(),
                    when_args: vec![json!("DE")],
                    returns: vec![FixtureReturn {
                        return_value: Some(json!(0.19)),
                        exception: None,
                    }],
                    mutate_args: BTreeMap::new(),
                    expect_calls: Some(1),
                });
                fixtures.insert("mock_currency_converter".to_string(), FixtureDefinition {
                    target: "CurrencyConverter::getExchangeRate".to_string(),
                    when_args: vec![json!("USD"), json!("EUR")],
                    returns: vec![FixtureReturn {
                        return_value: Some(json!(0.92)),
                        exception: None,
                    }],
                    mutate_args: BTreeMap::new(),
                    expect_calls: Some(1),
                });
                create_doc(
                    "02_ref_resolution_multiple",
                    "src/Tax/MultiCurrencyTaxCalculator.php",
                    fixtures,
                    "App\\Tax\\MultiCurrencyTaxCalculator::calculateConvertedTax",
                    vec![
                        ParameterDefinition {
                            name: "country".to_string(),
                            param_type: "string".to_string(),
                            constraints: vec![],
                            default_value: None,
                        },
                        ParameterDefinition {
                            name: "currency".to_string(),
                            param_type: "string".to_string(),
                            constraints: vec![],
                            default_value: None,
                        },
                        ParameterDefinition {
                            name: "amount".to_string(),
                            param_type: "float".to_string(),
                            constraints: vec![],
                            default_value: None,
                        },
                    ],
                    Some("float".to_string()),
                    vec![
                        DetectedSideEffect {
                            effect_type: "method_call".to_string(),
                            target: "TaxRepository::getRate".to_string(),
                            mockable: true,
                        },
                        DetectedSideEffect {
                            effect_type: "method_call".to_string(),
                            target: "CurrencyConverter::getExchangeRate".to_string(),
                            mockable: true,
                        },
                    ],
                    vec![ScenarioDefinition {
                        name: "calculate_converted_tax".to_string(),
                        mode: ScenarioMode::SymbolicContract,
                        locked: false,
                        stale: false,
                        tags: vec!["multi_currency".to_string(), "ref_resolution".to_string()],
                        mocks: vec![
                            MockReferenceOrInline::Reference {
                                reference: "#/fixtures/mock_tax_repo".to_string(),
                            },
                            MockReferenceOrInline::Reference {
                                reference: "#/fixtures/mock_currency_converter".to_string(),
                            },
                        ],
                        setup_calls: vec![],
                        invoke_returned_args: None,
                        inputs: json!({"country": "DE", "currency": "USD", "amount": 100.0}),
                        expected: ExpectedOutcome {
                            outcome: OutcomeKind::Return,
                            expected_type: Some("float".to_string()),
                            value: Some(json!(17.48)),
                            exception_type: None,
                            message_contains: None,
                            expected_events: vec![],
                        },
                    }],
                    BTreeMap::new(),
                )
            },
        ),

        // 03: $ctor positional argument hydration
        (
            "03_ctor_hydration",
            create_doc(
                "03_ctor_hydration",
                "src/Payment/PaymentService.php",
                BTreeMap::new(),
                "App\\Payment\\PaymentService::applyVat",
                vec![ParameterDefinition {
                    name: "money".to_string(),
                    param_type: "App\\ValueObject\\Money".to_string(),
                    constraints: vec![],
                    default_value: None,
                }],
                Some("App\\ValueObject\\Money".to_string()),
                vec![],
                vec![ScenarioDefinition {
                    name: "hydrate_money_via_ctor".to_string(),
                    mode: ScenarioMode::SymbolicContract,
                    locked: false,
                    stale: false,
                    tags: vec!["hydration".to_string(), "ctor".to_string()],
                    mocks: vec![],
                    setup_calls: vec![],
                    invoke_returned_args: None,
                    inputs: json!({
                        "money": {
                            "$type": "App\\ValueObject\\Money",
                            "$ctor": [10000, "EUR"]
                        }
                    }),
                    expected: ExpectedOutcome {
                        outcome: OutcomeKind::Return,
                        expected_type: Some("App\\ValueObject\\Money".to_string()),
                        value: Some(json!({
                            "$type": "App\\ValueObject\\Money",
                            "$ctor": [11900, "EUR"]
                        })),
                        exception_type: None,
                        message_contains: None,
                        expected_events: vec![],
                    },
                }],
                BTreeMap::new(),
            ),
        ),

        // 04: $factory static method hydration
        (
            "04_factory_hydration",
            create_doc(
                "04_factory_hydration",
                "src/Payment/FactoryService.php",
                BTreeMap::new(),
                "App\\Payment\\FactoryService::processNamedFactory",
                vec![ParameterDefinition {
                    name: "money".to_string(),
                    param_type: "App\\ValueObject\\Money".to_string(),
                    constraints: vec![],
                    default_value: None,
                }],
                Some("App\\ValueObject\\Money".to_string()),
                vec![],
                vec![ScenarioDefinition {
                    name: "hydrate_money_via_factory".to_string(),
                    mode: ScenarioMode::SymbolicContract,
                    locked: false,
                    stale: false,
                    tags: vec!["hydration".to_string(), "factory".to_string()],
                    mocks: vec![],
                    setup_calls: vec![],
                    invoke_returned_args: None,
                    inputs: json!({
                        "money": {
                            "$type": "App\\ValueObject\\Money",
                            "$factory": "fromCents",
                            "$ctor": [5000, "USD"]
                        }
                    }),
                    expected: ExpectedOutcome {
                        outcome: OutcomeKind::Return,
                        expected_type: Some("App\\ValueObject\\Money".to_string()),
                        value: Some(json!({
                            "$type": "App\\ValueObject\\Money",
                            "$factory": "fromCents",
                            "$ctor": [5950, "USD"]
                        })),
                        exception_type: None,
                        message_contains: None,
                        expected_events: vec![],
                    },
                }],
                BTreeMap::new(),
            ),
        ),

        // 05: $props property assignment hydration
        (
            "05_props_hydration",
            create_doc(
                "05_props_hydration",
                "src/User/ProfileManager.php",
                BTreeMap::new(),
                "App\\User\\ProfileManager::validateProfile",
                vec![ParameterDefinition {
                    name: "profile".to_string(),
                    param_type: "App\\DTO\\UserProfile".to_string(),
                    constraints: vec![],
                    default_value: None,
                }],
                Some("bool".to_string()),
                vec![],
                vec![ScenarioDefinition {
                    name: "hydrate_profile_via_props".to_string(),
                    mode: ScenarioMode::SymbolicContract,
                    locked: false,
                    stale: false,
                    tags: vec!["hydration".to_string(), "props".to_string()],
                    mocks: vec![],
                    setup_calls: vec![],
                    invoke_returned_args: None,
                    inputs: json!({
                        "profile": {
                            "$type": "App\\DTO\\UserProfile",
                            "$props": {
                                "id": 42,
                                "email": "alice@example.com",
                                "active": true
                            }
                        }
                    }),
                    expected: ExpectedOutcome {
                        outcome: OutcomeKind::Return,
                        expected_type: Some("bool".to_string()),
                        value: Some(json!(true)),
                        exception_type: None,
                        message_contains: None,
                        expected_events: vec![],
                    },
                }],
                BTreeMap::new(),
            ),
        ),

        // 06: Zero float boundary
        (
            "06_boundary_zero_float",
            create_doc(
                "06_boundary_zero_float",
                "src/Billing/DiscountService.php",
                BTreeMap::new(),
                "App\\Billing\\DiscountService::calculateDiscount",
                vec![ParameterDefinition {
                    name: "amount".to_string(),
                    param_type: "float".to_string(),
                    constraints: vec![">= 0.0".to_string(), "finite".to_string()],
                    default_value: None,
                }],
                Some("float".to_string()),
                vec![],
                vec![ScenarioDefinition {
                    name: "boundary_zero_float_nominal".to_string(),
                    mode: ScenarioMode::SymbolicContract,
                    locked: false,
                    stale: false,
                    tags: vec!["boundary".to_string(), "zero_float".to_string()],
                    mocks: vec![],
                    setup_calls: vec![],
                    invoke_returned_args: None,
                    inputs: json!({"amount": 0.0}),
                    expected: ExpectedOutcome {
                        outcome: OutcomeKind::Return,
                        expected_type: Some("float".to_string()),
                        value: Some(json!(0.0)),
                        exception_type: None,
                        message_contains: None,
                        expected_events: vec![],
                    },
                }],
                BTreeMap::new(),
            ),
        ),

        // 07: Negative guard throws
        (
            "07_boundary_negative_guard",
            create_doc(
                "07_boundary_negative_guard",
                "src/Billing/DiscountService.php",
                BTreeMap::new(),
                "App\\Billing\\DiscountService::calculateDiscount",
                vec![ParameterDefinition {
                    name: "amount".to_string(),
                    param_type: "float".to_string(),
                    constraints: vec![">= 0.0".to_string()],
                    default_value: None,
                }],
                Some("float".to_string()),
                vec![],
                vec![ScenarioDefinition {
                    name: "guard_amount_negative_throws".to_string(),
                    mode: ScenarioMode::SymbolicContract,
                    locked: false,
                    stale: false,
                    tags: vec!["guard_clause".to_string(), "negative".to_string()],
                    mocks: vec![],
                    setup_calls: vec![],
                    invoke_returned_args: None,
                    inputs: json!({"amount": -0.01}),
                    expected: ExpectedOutcome {
                        outcome: OutcomeKind::Exception,
                        expected_type: None,
                        value: None,
                        exception_type: Some("InvalidArgumentException".to_string()),
                        message_contains: Some("amount".to_string()),
                        expected_events: vec![],
                    },
                }],
                BTreeMap::new(),
            ),
        ),

        // 08: Empty string guard throws
        (
            "08_boundary_empty_string_guard",
            create_doc(
                "08_boundary_empty_string_guard",
                "src/Catalog/ProductCodeValidator.php",
                BTreeMap::new(),
                "App\\Catalog\\ProductCodeValidator::validateCode",
                vec![ParameterDefinition {
                    name: "code".to_string(),
                    param_type: "string".to_string(),
                    constraints: vec!["non-empty-string".to_string()],
                    default_value: None,
                }],
                Some("bool".to_string()),
                vec![],
                vec![ScenarioDefinition {
                    name: "guard_code_empty_string_throws".to_string(),
                    mode: ScenarioMode::SymbolicContract,
                    locked: false,
                    stale: false,
                    tags: vec!["guard_clause".to_string(), "empty_string".to_string()],
                    mocks: vec![],
                    setup_calls: vec![],
                    invoke_returned_args: None,
                    inputs: json!({"code": ""}),
                    expected: ExpectedOutcome {
                        outcome: OutcomeKind::Exception,
                        expected_type: None,
                        value: None,
                        exception_type: Some("InvalidArgumentException".to_string()),
                        message_contains: Some("code".to_string()),
                        expected_events: vec![],
                    },
                }],
                BTreeMap::new(),
            ),
        ),

        // 09: Fixed length string boundary
        (
            "09_boundary_fixed_length_string",
            create_doc(
                "09_boundary_fixed_length_string",
                "src/Geo/CountryValidator.php",
                BTreeMap::new(),
                "App\\Geo\\CountryValidator::isSupportedCountry",
                vec![ParameterDefinition {
                    name: "countryCode".to_string(),
                    param_type: "string".to_string(),
                    constraints: vec!["length == 2".to_string()],
                    default_value: None,
                }],
                Some("bool".to_string()),
                vec![],
                vec![ScenarioDefinition {
                    name: "fixed_length_string_nominal".to_string(),
                    mode: ScenarioMode::SymbolicContract,
                    locked: false,
                    stale: false,
                    tags: vec!["boundary".to_string(), "fixed_length".to_string()],
                    mocks: vec![],
                    setup_calls: vec![],
                    invoke_returned_args: None,
                    inputs: json!({"countryCode": "DE"}),
                    expected: ExpectedOutcome {
                        outcome: OutcomeKind::Return,
                        expected_type: Some("bool".to_string()),
                        value: Some(json!(true)),
                        exception_type: None,
                        message_contains: None,
                        expected_events: vec![],
                    },
                }],
                BTreeMap::new(),
            ),
        ),

        // 10: Exact argument matcher
        (
            "10_matcher_exact",
            {
                let mut fixtures = BTreeMap::new();
                fixtures.insert("mock_exact_order".to_string(), FixtureDefinition {
                    target: "OrderRepository::findByReference".to_string(),
                    when_args: vec![json!({"$matcher": "exact", "value": "ORD-12345"})],
                    returns: vec![FixtureReturn {
                        return_value: Some(json!({"id": 12345, "status": "shipped"})),
                        exception: None,
                    }],
                    mutate_args: BTreeMap::new(),
                    expect_calls: Some(1),
                });
                create_doc(
                    "10_matcher_exact",
                    "src/Order/OrderStatusChecker.php",
                    fixtures,
                    "App\\Order\\OrderStatusChecker::checkStatus",
                    vec![ParameterDefinition {
                        name: "orderNumber".to_string(),
                        param_type: "string".to_string(),
                        constraints: vec![],
                        default_value: None,
                    }],
                    Some("string".to_string()),
                    vec![DetectedSideEffect {
                        effect_type: "method_call".to_string(),
                        target: "OrderRepository::findByReference".to_string(),
                        mockable: true,
                    }],
                    vec![ScenarioDefinition {
                        name: "match_exact_argument".to_string(),
                        mode: ScenarioMode::SymbolicContract,
                        locked: false,
                        stale: false,
                        tags: vec!["matcher".to_string(), "exact".to_string()],
                        mocks: vec![MockReferenceOrInline::Reference {
                            reference: "#/fixtures/mock_exact_order".to_string(),
                        }],
                        setup_calls: vec![],
                        invoke_returned_args: None,
                        inputs: json!({"orderNumber": "ORD-12345"}),
                        expected: ExpectedOutcome {
                            outcome: OutcomeKind::Return,
                            expected_type: Some("string".to_string()),
                            value: Some(json!("shipped")),
                            exception_type: None,
                            message_contains: None,
                            expected_events: vec![],
                        },
                    }],
                    BTreeMap::new(),
                )
            },
        ),

        // 11: Regex argument matcher
        (
            "11_matcher_regex",
            {
                let mut fixtures = BTreeMap::new();
                fixtures.insert("mock_email_sender".to_string(), FixtureDefinition {
                    target: "MailerGateway::sendWelcome".to_string(),
                    when_args: vec![json!({
                        "$matcher": "regex",
                        "pattern": "^[a-zA-Z0-9._%+-]+@[a-zA-Z0-9.-]+\\.[a-zA-Z]{2,}$"
                    })],
                    returns: vec![FixtureReturn {
                        return_value: Some(json!(true)),
                        exception: None,
                    }],
                    mutate_args: BTreeMap::new(),
                    expect_calls: Some(1),
                });
                create_doc(
                    "11_matcher_regex",
                    "src/User/UserRegistration.php",
                    fixtures,
                    "App\\User\\UserRegistration::register",
                    vec![ParameterDefinition {
                        name: "email".to_string(),
                        param_type: "string".to_string(),
                        constraints: vec![],
                        default_value: None,
                    }],
                    Some("bool".to_string()),
                    vec![DetectedSideEffect {
                        effect_type: "method_call".to_string(),
                        target: "MailerGateway::sendWelcome".to_string(),
                        mockable: true,
                    }],
                    vec![ScenarioDefinition {
                        name: "match_regex_argument".to_string(),
                        mode: ScenarioMode::SymbolicContract,
                        locked: false,
                        stale: false,
                        tags: vec!["matcher".to_string(), "regex".to_string()],
                        mocks: vec![MockReferenceOrInline::Reference {
                            reference: "#/fixtures/mock_email_sender".to_string(),
                        }],
                        setup_calls: vec![],
                        invoke_returned_args: None,
                        inputs: json!({"email": "test@example.com"}),
                        expected: ExpectedOutcome {
                            outcome: OutcomeKind::Return,
                            expected_type: Some("bool".to_string()),
                            value: Some(json!(true)),
                            exception_type: None,
                            message_contains: None,
                            expected_events: vec![],
                        },
                    }],
                    BTreeMap::new(),
                )
            },
        ),

        // 12: Range argument matcher
        (
            "12_matcher_range",
            {
                let mut fixtures = BTreeMap::new();
                fixtures.insert("mock_tier_calc".to_string(), FixtureDefinition {
                    target: "DiscountCalculator::getTierDiscount".to_string(),
                    when_args: vec![json!({"$matcher": "range", "min": 100, "max": 500})],
                    returns: vec![FixtureReturn {
                        return_value: Some(json!(0.10)),
                        exception: None,
                    }],
                    mutate_args: BTreeMap::new(),
                    expect_calls: Some(1),
                });
                create_doc(
                    "12_matcher_range",
                    "src/Checkout/PricingEngine.php",
                    fixtures,
                    "App\\Checkout\\PricingEngine::calculateDiscount",
                    vec![ParameterDefinition {
                        name: "total".to_string(),
                        param_type: "float".to_string(),
                        constraints: vec![],
                        default_value: None,
                    }],
                    Some("float".to_string()),
                    vec![DetectedSideEffect {
                        effect_type: "method_call".to_string(),
                        target: "DiscountCalculator::getTierDiscount".to_string(),
                        mockable: true,
                    }],
                    vec![ScenarioDefinition {
                        name: "match_range_argument".to_string(),
                        mode: ScenarioMode::SymbolicContract,
                        locked: false,
                        stale: false,
                        tags: vec!["matcher".to_string(), "range".to_string()],
                        mocks: vec![MockReferenceOrInline::Reference {
                            reference: "#/fixtures/mock_tier_calc".to_string(),
                        }],
                        setup_calls: vec![],
                        invoke_returned_args: None,
                        inputs: json!({"total": 250.0}),
                        expected: ExpectedOutcome {
                            outcome: OutcomeKind::Return,
                            expected_type: Some("float".to_string()),
                            value: Some(json!(25.0)),
                            exception_type: None,
                            message_contains: None,
                            expected_events: vec![],
                        },
                    }],
                    BTreeMap::new(),
                )
            },
        ),

        // 13: Subset object argument matcher
        (
            "13_matcher_subset",
            {
                let mut fixtures = BTreeMap::new();
                fixtures.insert("mock_auth_guard".to_string(), FixtureDefinition {
                    target: "AuthGuard::verifyClaims".to_string(),
                    when_args: vec![json!({"$matcher": "subset", "value": {"role": "admin"}})],
                    returns: vec![FixtureReturn {
                        return_value: Some(json!(true)),
                        exception: None,
                    }],
                    mutate_args: BTreeMap::new(),
                    expect_calls: Some(1),
                });
                create_doc(
                    "13_matcher_subset",
                    "src/Security/AdminGate.php",
                    fixtures,
                    "App\\Security\\AdminGate::canAccess",
                    vec![ParameterDefinition {
                        name: "session".to_string(),
                        param_type: "array".to_string(),
                        constraints: vec![],
                        default_value: None,
                    }],
                    Some("bool".to_string()),
                    vec![DetectedSideEffect {
                        effect_type: "method_call".to_string(),
                        target: "AuthGuard::verifyClaims".to_string(),
                        mockable: true,
                    }],
                    vec![ScenarioDefinition {
                        name: "match_subset_argument".to_string(),
                        mode: ScenarioMode::SymbolicContract,
                        locked: false,
                        stale: false,
                        tags: vec!["matcher".to_string(), "subset".to_string()],
                        mocks: vec![MockReferenceOrInline::Reference {
                            reference: "#/fixtures/mock_auth_guard".to_string(),
                        }],
                        setup_calls: vec![],
                        invoke_returned_args: None,
                        inputs: json!({"session": {"userId": "usr_99", "role": "admin", "token": "xyz"}}),
                        expected: ExpectedOutcome {
                            outcome: OutcomeKind::Return,
                            expected_type: Some("bool".to_string()),
                            value: Some(json!(true)),
                            exception_type: None,
                            message_contains: None,
                            expected_events: vec![],
                        },
                    }],
                    BTreeMap::new(),
                )
            },
        ),

        // 14: Sequential returns mock
        (
            "14_mock_sequential_returns",
            {
                let mut fixtures = BTreeMap::new();
                fixtures.insert("mock_token_sequence".to_string(), FixtureDefinition {
                    target: "TokenProvider::nextToken".to_string(),
                    when_args: vec![],
                    returns: vec![
                        FixtureReturn { return_value: Some(json!("token_alpha")), exception: None },
                        FixtureReturn { return_value: Some(json!("token_beta")), exception: None },
                        FixtureReturn { return_value: Some(json!("token_gamma")), exception: None },
                    ],
                    mutate_args: BTreeMap::new(),
                    expect_calls: Some(3),
                });
                create_doc(
                    "14_mock_sequential_returns",
                    "src/Auth/TokenPool.php",
                    fixtures,
                    "App\\Auth\\TokenPool::fetchBatch",
                    vec![ParameterDefinition {
                        name: "count".to_string(),
                        param_type: "int".to_string(),
                        constraints: vec![],
                        default_value: None,
                    }],
                    Some("array".to_string()),
                    vec![DetectedSideEffect {
                        effect_type: "method_call".to_string(),
                        target: "TokenProvider::nextToken".to_string(),
                        mockable: true,
                    }],
                    vec![ScenarioDefinition {
                        name: "sequential_mock_returns".to_string(),
                        mode: ScenarioMode::SymbolicContract,
                        locked: false,
                        stale: false,
                        tags: vec!["mock".to_string(), "sequential_returns".to_string()],
                        mocks: vec![MockReferenceOrInline::Reference {
                            reference: "#/fixtures/mock_token_sequence".to_string(),
                        }],
                        setup_calls: vec![],
                        invoke_returned_args: None,
                        inputs: json!({"count": 3}),
                        expected: ExpectedOutcome {
                            outcome: OutcomeKind::Return,
                            expected_type: Some("array".to_string()),
                            value: Some(json!(["token_alpha", "token_beta", "token_gamma"])),
                            exception_type: None,
                            message_contains: None,
                            expected_events: vec![],
                        },
                    }],
                    BTreeMap::new(),
                )
            },
        ),

        // 15: Mutate args mock
        (
            "15_mock_mutate_args",
            {
                let mut fixtures = BTreeMap::new();
                let mut mutate_args = BTreeMap::new();
                mutate_args.insert("0".to_string(), json!({"bytesRead": 4, "data": "OKAY"}));
                fixtures.insert("mock_byte_reader".to_string(), FixtureDefinition {
                    target: "StreamReader::readInto".to_string(),
                    when_args: vec![],
                    returns: vec![FixtureReturn {
                        return_value: Some(json!(4)),
                        exception: None,
                    }],
                    mutate_args,
                    expect_calls: Some(1),
                });
                create_doc(
                    "15_mock_mutate_args",
                    "src/IO/BufferProcessor.php",
                    fixtures,
                    "App\\IO\\BufferProcessor::fillBuffer",
                    vec![ParameterDefinition {
                        name: "buffer".to_string(),
                        param_type: "array".to_string(),
                        constraints: vec![],
                        default_value: None,
                    }],
                    Some("int".to_string()),
                    vec![DetectedSideEffect {
                        effect_type: "method_call".to_string(),
                        target: "StreamReader::readInto".to_string(),
                        mockable: true,
                    }],
                    vec![ScenarioDefinition {
                        name: "mutate_args_mock_behavior".to_string(),
                        mode: ScenarioMode::SymbolicContract,
                        locked: false,
                        stale: false,
                        tags: vec!["mock".to_string(), "mutate_args".to_string()],
                        mocks: vec![MockReferenceOrInline::Reference {
                            reference: "#/fixtures/mock_byte_reader".to_string(),
                        }],
                        setup_calls: vec![],
                        invoke_returned_args: None,
                        inputs: json!({"buffer": {"capacity": 1024}}),
                        expected: ExpectedOutcome {
                            outcome: OutcomeKind::Return,
                            expected_type: Some("int".to_string()),
                            value: Some(json!(4)),
                            exception_type: None,
                            message_contains: None,
                            expected_events: vec![],
                        },
                    }],
                    BTreeMap::new(),
                )
            },
        ),

        // 16: Expect calls assertion
        (
            "16_mock_expect_calls",
            {
                let mut fixtures = BTreeMap::new();
                fixtures.insert("mock_audit_logger".to_string(), FixtureDefinition {
                    target: "AuditLogger::logEvent".to_string(),
                    when_args: vec![json!("ORDER_PLACED")],
                    returns: vec![FixtureReturn {
                        return_value: Some(json!(null)),
                        exception: None,
                    }],
                    mutate_args: BTreeMap::new(),
                    expect_calls: Some(2),
                });
                create_doc(
                    "16_mock_expect_calls",
                    "src/Order/BatchOrderProcessor.php",
                    fixtures,
                    "App\\Order\\BatchOrderProcessor::processBatch",
                    vec![ParameterDefinition {
                        name: "orderIds".to_string(),
                        param_type: "array".to_string(),
                        constraints: vec![],
                        default_value: None,
                    }],
                    Some("int".to_string()),
                    vec![DetectedSideEffect {
                        effect_type: "method_call".to_string(),
                        target: "AuditLogger::logEvent".to_string(),
                        mockable: true,
                    }],
                    vec![ScenarioDefinition {
                        name: "verify_mock_call_count".to_string(),
                        mode: ScenarioMode::SymbolicContract,
                        locked: false,
                        stale: false,
                        tags: vec!["mock".to_string(), "expect_calls".to_string()],
                        mocks: vec![MockReferenceOrInline::Reference {
                            reference: "#/fixtures/mock_audit_logger".to_string(),
                        }],
                        setup_calls: vec![],
                        invoke_returned_args: None,
                        inputs: json!({"orderIds": [101, 102]}),
                        expected: ExpectedOutcome {
                            outcome: OutcomeKind::Return,
                            expected_type: Some("int".to_string()),
                            value: Some(json!(2)),
                            exception_type: None,
                            message_contains: None,
                            expected_events: vec![],
                        },
                    }],
                    BTreeMap::new(),
                )
            },
        ),

        // 17: Exception type check
        (
            "17_exception_type",
            create_doc(
                "17_exception_type",
                "src/Repository/ItemRepository.php",
                BTreeMap::new(),
                "App\\Repository\\ItemRepository::findById",
                vec![ParameterDefinition {
                    name: "id".to_string(),
                    param_type: "string".to_string(),
                    constraints: vec![],
                    default_value: None,
                }],
                Some("App\\Entity\\Item".to_string()),
                vec![],
                vec![ScenarioDefinition {
                    name: "missing_item_throws_entity_not_found".to_string(),
                    mode: ScenarioMode::SymbolicContract,
                    locked: false,
                    stale: false,
                    tags: vec!["exception".to_string(), "exception_type".to_string()],
                    mocks: vec![],
                    setup_calls: vec![],
                    invoke_returned_args: None,
                    inputs: json!({"id": "MISSING_ID_999"}),
                    expected: ExpectedOutcome {
                        outcome: OutcomeKind::Exception,
                        expected_type: None,
                        value: None,
                        exception_type: Some("App\\Exception\\EntityNotFoundException".to_string()),
                        message_contains: None,
                        expected_events: vec![],
                    },
                }],
                BTreeMap::new(),
            ),
        ),

        // 18: Exception message contains
        (
            "18_exception_message_contains",
            create_doc(
                "18_exception_message_contains",
                "src/Security/TokenAuthenticator.php",
                BTreeMap::new(),
                "App\\Security\\TokenAuthenticator::authenticateToken",
                vec![ParameterDefinition {
                    name: "token".to_string(),
                    param_type: "string".to_string(),
                    constraints: vec![],
                    default_value: None,
                }],
                Some("bool".to_string()),
                vec![],
                vec![ScenarioDefinition {
                    name: "expired_token_throws_with_message".to_string(),
                    mode: ScenarioMode::SymbolicContract,
                    locked: false,
                    stale: false,
                    tags: vec!["exception".to_string(), "message_contains".to_string()],
                    mocks: vec![],
                    setup_calls: vec![],
                    invoke_returned_args: None,
                    inputs: json!({"token": "EXPIRED_TOKEN_ABC"}),
                    expected: ExpectedOutcome {
                        outcome: OutcomeKind::Exception,
                        expected_type: None,
                        value: None,
                        exception_type: Some("App\\Exception\\AuthenticationException".to_string()),
                        message_contains: Some("token has expired".to_string()),
                        expected_events: vec![],
                    },
                }],
                BTreeMap::new(),
            ),
        ),

        // 19: No crash outcome
        (
            "19_no_crash_outcome",
            create_doc(
                "19_no_crash_outcome",
                "src/Diagnostics/HeartbeatSender.php",
                BTreeMap::new(),
                "App\\Diagnostics\\HeartbeatSender::pulse",
                vec![ParameterDefinition {
                    name: "payload".to_string(),
                    param_type: "array".to_string(),
                    constraints: vec![],
                    default_value: None,
                }],
                None,
                vec![],
                vec![ScenarioDefinition {
                    name: "heartbeat_executes_without_crash".to_string(),
                    mode: ScenarioMode::SymbolicContract,
                    locked: false,
                    stale: false,
                    tags: vec!["no_crash".to_string(), "void_method".to_string()],
                    mocks: vec![],
                    setup_calls: vec![],
                    invoke_returned_args: None,
                    inputs: json!({"payload": {"event": "HEARTBEAT"}}),
                    expected: ExpectedOutcome {
                        outcome: OutcomeKind::NoCrash,
                        expected_type: None,
                        value: None,
                        exception_type: None,
                        message_contains: None,
                        expected_events: vec![],
                    },
                }],
                BTreeMap::new(),
            ),
        ),

        // 20: Preserve locked scenarios on merge
        (
            "20_merge_preserve_locked",
            create_doc(
                "20_merge_preserve_locked",
                "src/Vat/CustomVatCalculator.php",
                BTreeMap::new(),
                "App\\Vat\\CustomVatCalculator::calculateCustomRate",
                vec![ParameterDefinition {
                    name: "rate".to_string(),
                    param_type: "float".to_string(),
                    constraints: vec![],
                    default_value: None,
                }],
                Some("float".to_string()),
                vec![],
                vec![ScenarioDefinition {
                    name: "custom_curated_locked_case".to_string(),
                    mode: ScenarioMode::Manual,
                    locked: true,
                    stale: false,
                    tags: vec!["curated".to_string(), "locked_preserve".to_string()],
                    mocks: vec![],
                    setup_calls: vec![],
                    invoke_returned_args: None,
                    inputs: json!({"rate": 0.25}),
                    expected: ExpectedOutcome {
                        outcome: OutcomeKind::Return,
                        expected_type: Some("float".to_string()),
                        value: Some(json!(125.0)),
                        exception_type: None,
                        message_contains: None,
                        expected_events: vec![],
                    },
                }],
                BTreeMap::new(),
            ),
        ),

        // 21: Remove obsolete unlocked scenarios
        (
            "21_merge_remove_unlocked",
            create_doc(
                "21_merge_remove_unlocked",
                "src/Legacy/LegacyFormatter.php",
                BTreeMap::new(),
                "App\\Legacy\\LegacyFormatter::formatOutput",
                vec![ParameterDefinition {
                    name: "param".to_string(),
                    param_type: "string".to_string(),
                    constraints: vec![],
                    default_value: None,
                }],
                Some("string".to_string()),
                vec![],
                vec![ScenarioDefinition {
                    name: "obsolete_unlocked_scenario".to_string(),
                    mode: ScenarioMode::SymbolicContract,
                    locked: false,
                    stale: false,
                    tags: vec!["unlocked".to_string(), "candidate_for_removal".to_string()],
                    mocks: vec![],
                    setup_calls: vec![],
                    invoke_returned_args: None,
                    inputs: json!({"param": "deprecated_value"}),
                    expected: ExpectedOutcome {
                        outcome: OutcomeKind::Return,
                        expected_type: Some("string".to_string()),
                        value: Some(json!("deprecated_result")),
                        exception_type: None,
                        message_contains: None,
                        expected_events: vec![],
                    },
                }],
                BTreeMap::new(),
            ),
        ),

        // 22: Parameter removed drift detection
        (
            "22_drift_param_removed",
            create_doc(
                "22_drift_param_removed",
                "src/Order/InvoiceModifier.php",
                BTreeMap::new(),
                "App\\Order\\InvoiceModifier::modifyInvoice",
                vec![ParameterDefinition {
                    name: "newParam".to_string(),
                    param_type: "string".to_string(),
                    constraints: vec![],
                    default_value: None,
                }],
                Some("bool".to_string()),
                vec![],
                vec![ScenarioDefinition {
                    name: "drift_stale_due_to_removed_param".to_string(),
                    mode: ScenarioMode::Manual,
                    locked: true,
                    stale: true,
                    tags: vec!["drift".to_string(), "param_removed".to_string()],
                    mocks: vec![],
                    setup_calls: vec![],
                    invoke_returned_args: None,
                    inputs: json!({"obsoleteParam": "drift_value"}),
                    expected: ExpectedOutcome {
                        outcome: OutcomeKind::Return,
                        expected_type: Some("bool".to_string()),
                        value: Some(json!(true)),
                        exception_type: None,
                        message_contains: None,
                        expected_events: vec![],
                    },
                }],
                BTreeMap::new(),
            ),
        ),

        // 23: Mock removed drift detection
        (
            "23_drift_mock_removed",
            {
                let mut fixtures = BTreeMap::new();
                fixtures.insert("removed_side_effect".to_string(), FixtureDefinition {
                    target: "LegacyService::obsoleteMethod".to_string(),
                    when_args: vec![],
                    returns: vec![FixtureReturn {
                        return_value: Some(json!(true)),
                        exception: None,
                    }],
                    mutate_args: BTreeMap::new(),
                    expect_calls: Some(1),
                });
                create_doc(
                    "23_drift_mock_removed",
                    "src/Service/RefactoredService.php",
                    fixtures,
                    "App\\Service\\RefactoredService::handleTask",
                    vec![ParameterDefinition {
                        name: "taskName".to_string(),
                        param_type: "string".to_string(),
                        constraints: vec![],
                        default_value: None,
                    }],
                    Some("bool".to_string()),
                    vec![], // Side effect removed from method!
                    vec![ScenarioDefinition {
                        name: "drift_stale_due_to_removed_mock".to_string(),
                        mode: ScenarioMode::Manual,
                        locked: true,
                        stale: true,
                        tags: vec!["drift".to_string(), "mock_removed".to_string()],
                        mocks: vec![MockReferenceOrInline::Reference {
                            reference: "#/fixtures/removed_side_effect".to_string(),
                        }],
                        setup_calls: vec![],
                        invoke_returned_args: None,
                        inputs: json!({"taskName": "daily_sync"}),
                        expected: ExpectedOutcome {
                            outcome: OutcomeKind::Return,
                            expected_type: Some("bool".to_string()),
                            value: Some(json!(true)),
                            exception_type: None,
                            message_contains: None,
                            expected_events: vec![],
                        },
                    }],
                    BTreeMap::new(),
                )
            },
        ),

        // 24: Transitive contract invalidation
        (
            "24_cache_transitive_invalidation",
            {
                let mut deps = BTreeMap::new();
                deps.insert(
                    "App\\Service\\TaxRateProvider".to_string(),
                    "contract_sha256:a1b2c3d4e5f60718293a4b5c6d7e8f90".to_string(),
                );
                create_doc(
                    "24_cache_transitive_invalidation",
                    "src/Billing/DependentBillingService.php",
                    BTreeMap::new(),
                    "App\\Billing\\DependentBillingService::calculateFinalBill",
                    vec![ParameterDefinition {
                        name: "amount".to_string(),
                        param_type: "float".to_string(),
                        constraints: vec![],
                        default_value: None,
                    }],
                    Some("float".to_string()),
                    vec![],
                    vec![ScenarioDefinition {
                        name: "transitive_cache_scenario".to_string(),
                        mode: ScenarioMode::SymbolicContract,
                        locked: false,
                        stale: false,
                        tags: vec!["cache".to_string(), "transitive_invalidation".to_string()],
                        mocks: vec![],
                        setup_calls: vec![],
                        invoke_returned_args: None,
                        inputs: json!({"amount": 100.0}),
                        expected: ExpectedOutcome {
                            outcome: OutcomeKind::Return,
                            expected_type: Some("float".to_string()),
                            value: Some(json!(119.0)),
                            exception_type: None,
                            message_contains: None,
                            expected_events: vec![],
                        },
                    }],
                    deps,
                )
            },
        ),

        // 25: Cyclic dependency Tarjan SCC
        (
            "25_cache_cyclic_tarjan_scc",
            {
                let mut deps = BTreeMap::new();
                deps.insert(
                    "App\\Service\\BillingService".to_string(),
                    "contract_sha256:aaaa1111bbbb2222".to_string(),
                );
                deps.insert(
                    "App\\Service\\InvoiceService".to_string(),
                    "contract_sha256:cccc3333dddd4444".to_string(),
                );
                create_doc(
                    "25_cache_cyclic_tarjan_scc",
                    "src/Order/CircularOrderService.php",
                    BTreeMap::new(),
                    "App\\Order\\CircularOrderService::processOrder",
                    vec![ParameterDefinition {
                        name: "orderId".to_string(),
                        param_type: "string".to_string(),
                        constraints: vec![],
                        default_value: None,
                    }],
                    Some("bool".to_string()),
                    vec![],
                    vec![ScenarioDefinition {
                        name: "tarjan_scc_cyclic_scenario".to_string(),
                        mode: ScenarioMode::SymbolicContract,
                        locked: false,
                        stale: false,
                        tags: vec!["cache".to_string(), "cyclic_tarjan_scc".to_string()],
                        mocks: vec![],
                        setup_calls: vec![],
                        invoke_returned_args: None,
                        inputs: json!({"orderId": "ORD-42"}),
                        expected: ExpectedOutcome {
                            outcome: OutcomeKind::Return,
                            expected_type: Some("bool".to_string()),
                            value: Some(json!(true)),
                            exception_type: None,
                            message_contains: None,
                            expected_events: vec![],
                        },
                    }],
                    deps,
                )
            },
        ),
    ]
}
