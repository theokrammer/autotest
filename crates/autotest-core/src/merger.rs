use std::collections::{BTreeMap, HashSet};
use serde_json::Value;
use crate::ir::{
    MockReferenceOrInline, ParameterDefinition, ScenarioDefinition, ScenarioDocument,
    TargetDefinition,
};
use crate::mock::FixtureDeduplicator;

#[derive(Debug, Default, Clone)]
pub struct MergeReport {
    pub added_scenarios: usize,
    pub retained_locked: usize,
    pub removed_unlocked: usize,
    pub stale_assertions: Vec<StaleAssertionDrift>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StaleAssertionDrift {
    pub file: String,
    pub target_id: String,
    pub scenario_name: String,
    pub reason: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeMode {
    NonDestructive,
    Force,
    ForceAll,
}

pub struct ScenarioMerger;

impl ScenarioMerger {
    pub fn merge(
        existing: Option<&ScenarioDocument>,
        generated: &ScenarioDocument,
        mode: MergeMode,
    ) -> (ScenarioDocument, MergeReport) {
        let mut report = MergeReport::default();

        if mode == MergeMode::ForceAll || existing.is_none() {
            let doc = generated.clone();
            report.added_scenarios = doc.targets.iter().map(|t| t.scenarios.len()).sum();
            return (doc, report);
        }

        let existing = existing.unwrap();
        let mut merged_doc = generated.clone();

        // 1. Preserve existing manual fixtures
        for (k, v) in &existing.fixtures {
            if !merged_doc.fixtures.contains_key(k) {
                merged_doc.fixtures.insert(k.clone(), v.clone());
            }
        }

        // Map targets from existing document by target id
        let existing_targets: BTreeMap<String, &TargetDefinition> =
            existing.targets.iter().map(|t| (t.id.clone(), t)).collect();

        for target in &mut merged_doc.targets {
            if let Some(existing_target) = existing_targets.get(&target.id) {
                let current_param_names: HashSet<String> = target
                    .signature
                    .parameters
                    .iter()
                    .map(|p| p.name.clone())
                    .collect();

                let current_side_effects: HashSet<String> = target
                    .detected_side_effects
                    .iter()
                    .map(|s| s.target.clone())
                    .collect();

                let mut merged_scenarios: Vec<ScenarioDefinition> = Vec::new();
                let existing_scenarios = &existing_target.scenarios;

                // Process existing scenarios
                for ex_scenario in existing_scenarios {
                    if ex_scenario.locked {
                        if mode == MergeMode::ForceAll {
                            // Skipped in ForceAll
                            continue;
                        }

                        let mut locked_scenario = ex_scenario.clone();
                        // Check for signature / mock drift
                        let drift_reason = check_drift(
                            &locked_scenario,
                            &target.signature.parameters,
                            &current_param_names,
                            &current_side_effects,
                            existing,
                        );

                        if let Some(reason) = drift_reason {
                            locked_scenario.stale = true;
                            report.stale_assertions.push(StaleAssertionDrift {
                                file: merged_doc.source.file.clone(),
                                target_id: target.id.clone(),
                                scenario_name: locked_scenario.name.clone(),
                                reason,
                            });
                        } else {
                            locked_scenario.stale = false;
                        }

                        merged_scenarios.push(locked_scenario);
                        report.retained_locked += 1;
                    } else if mode == MergeMode::NonDestructive {
                        // In non-destructive merge, retain existing unlocked scenarios as well
                        merged_scenarios.push(ex_scenario.clone());
                    } else {
                        // In Force mode, unlocked scenarios are overwritten
                        report.removed_unlocked += 1;
                    }
                }

                // Add newly generated scenarios that don't collide with retained locked names
                let locked_names: HashSet<String> =
                    merged_scenarios.iter().map(|s| s.name.clone()).collect();

                for gen_scenario in &target.scenarios {
                    if !locked_names.contains(&gen_scenario.name) {
                        merged_scenarios.push(gen_scenario.clone());
                        report.added_scenarios += 1;
                    }
                }

                target.scenarios = merged_scenarios;
            } else {
                report.added_scenarios += target.scenarios.len();
            }
        }

        (merged_doc, report)
    }
}

fn check_drift(
    scenario: &ScenarioDefinition,
    _params: &[ParameterDefinition],
    param_names: &HashSet<String>,
    side_effects: &HashSet<String>,
    doc: &ScenarioDocument,
) -> Option<String> {
    // Check if input parameters exist in current signature
    if let Value::Object(inputs) = &scenario.inputs {
        for key in inputs.keys() {
            if !param_names.contains(key) {
                return Some(format!("Parameter '{}' removed or renamed", key));
            }
        }
    }

    // Check if any mock targets are no longer in detected side effects
    for mock in &scenario.mocks {
        let mock_target = match mock {
            MockReferenceOrInline::Reference { reference } => {
                FixtureDeduplicator::resolve_ref(doc, reference).map(|f| f.target.as_str())
            }
            MockReferenceOrInline::Inline(fixture) => Some(fixture.target.as_str()),
        };

        if let Some(mt) = mock_target {
            if !side_effects.contains(mt) {
                return Some(format!("Mocked target '{}' no longer called by method", mt));
            }
        }
    }

    None
}
