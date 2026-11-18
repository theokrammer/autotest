use std::collections::BTreeMap;
use crate::ir::{FixtureDefinition, MockReferenceOrInline, ScenarioDocument};

pub struct FixtureDeduplicator;

impl FixtureDeduplicator {
    /// Extracts duplicated or inline fixtures into document.fixtures and points scenarios to them via $ref
    pub fn deduplicate(doc: &mut ScenarioDocument) {
        let mut key_counter = 1;
        let mut fixture_map: BTreeMap<String, FixtureDefinition> = doc.fixtures.clone();

        for target in &mut doc.targets {
            for scenario in &mut target.scenarios {
                for mock in &mut scenario.mocks {
                    if let MockReferenceOrInline::Inline(inline_fixture) = mock {
                        // Check if an identical fixture already exists in fixture_map
                        let existing_key = fixture_map.iter().find_map(|(k, v)| {
                            if v == inline_fixture {
                                Some(k.clone())
                            } else {
                                None
                            }
                        });

                        let key = match existing_key {
                            Some(k) => k,
                            None => {
                                let sanitized_target = inline_fixture
                                    .target
                                    .replace('.', "_")
                                    .replace("::", "_")
                                    .replace('\\', "_")
                                    .to_lowercase();
                                let mut candidate = format!("mock_{}", sanitized_target);
                                while fixture_map.contains_key(&candidate) {
                                    candidate = format!("mock_{}_{}", sanitized_target, key_counter);
                                    key_counter += 1;
                                }
                                fixture_map.insert(candidate.clone(), inline_fixture.clone());
                                candidate
                            }
                        };

                        *mock = MockReferenceOrInline::Reference {
                            reference: format!("#/fixtures/{}", key),
                        };
                    }
                }
            }
        }

        doc.fixtures = fixture_map;
    }

    /// Resolves a $ref pointer to its concrete FixtureDefinition
    pub fn resolve_ref<'a>(doc: &'a ScenarioDocument, ref_str: &str) -> Option<&'a FixtureDefinition> {
        let prefix = "#/fixtures/";
        if let Some(key) = ref_str.strip_prefix(prefix) {
            doc.fixtures.get(key)
        } else {
            None
        }
    }
}
