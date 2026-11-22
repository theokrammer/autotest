use std::collections::BTreeMap;
use regex::Regex;
use sha2::{Digest, Sha256};
use autotest_core::{
    DetectedSideEffect, ParameterDefinition, ScenarioDocument, SignatureDefinition,
    SourceMetadata, TargetDefinition, TargetKind,
};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum ExtractorError {
    #[error("Parse error: {0}")]
    ParseError(String),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Clone)]
pub struct PhpClassMetadata {
    pub namespace: String,
    pub class_name: String,
    pub constructor_params: Vec<ParameterDefinition>,
    pub static_factories: Vec<String>,
    pub public_properties: Vec<String>,
    pub methods: Vec<PhpMethodMetadata>,
    pub requires_fallback: bool,
}

#[derive(Debug, Clone)]
pub struct PhpMethodMetadata {
    pub name: String,
    pub is_static: bool,
    pub is_public: bool,
    pub parameters: Vec<ParameterDefinition>,
    pub return_type: Option<String>,
    pub detected_side_effects: Vec<DetectedSideEffect>,
    pub throws: Vec<String>,
}

pub struct PhpExtractor;

impl PhpExtractor {
    pub fn extract_source(file_path: &str, content: &str) -> Result<ScenarioDocument, ExtractorError> {
        let classes = Self::parse_php(content)?;
        let (impl_hash, _public_contract_hash) = Self::compute_hashes(content, &classes);

        let mut targets = Vec::new();
        let mut dependencies = BTreeMap::new();

        for class in &classes {
            let full_class_name = if class.namespace.is_empty() {
                class.class_name.clone()
            } else {
                format!("{}\\{}", class.namespace, class.class_name)
            };

            for method in &class.methods {
                if !method.is_public || method.name.starts_with("__") {
                    continue;
                }

                let target_id = format!("{}::{}", full_class_name, method.name);
                let target = TargetDefinition {
                    id: target_id,
                    kind: TargetKind::Method,
                    signature: SignatureDefinition {
                        constructor_args: class.constructor_params.clone(),
                        parameters: method.parameters.clone(),
                        return_type: method.return_type.clone(),
                    },
                    detected_side_effects: method.detected_side_effects.clone(),
                    scenarios: Vec::new(),
                };

                // Track injected dependencies as type contracts
                for dep in &class.constructor_params {
                    if dep.param_type.contains('\\') || dep.param_type.ends_with("Interface") || dep.param_type.ends_with("Repository") {
                        dependencies.insert(dep.param_type.clone(), "contract_sha256:discovered".to_string());
                    }
                }

                targets.push(target);
            }
        }

        let analyzer_name = if classes.iter().any(|c| c.requires_fallback) {
            "mago-analyzer+phpstan-fallback@1.59.0"
        } else {
            "mago-analyzer@1.59.0"
        };

        Ok(ScenarioDocument {
            schema: Some("https://autotest.dev/schema/scenario.v1.schema.json".to_string()),
            schema_version: "1.0".to_string(),
            source: SourceMetadata {
                file: file_path.to_string(),
                content_hash: impl_hash,
                analyzer: analyzer_name.to_string(),
                dependencies,
            },
            fixtures: BTreeMap::new(),
            targets,
        })
    }

    fn compute_hashes(content: &str, classes: &[PhpClassMetadata]) -> (String, String) {
        let mut full_hasher = Sha256::new();
        full_hasher.update(content.as_bytes());
        let impl_hash = format!("sha256:{:x}", full_hasher.finalize());

        let mut pub_contract_str = String::new();
        for c in classes {
            pub_contract_str.push_str(&c.namespace);
            pub_contract_str.push(':');
            pub_contract_str.push_str(&c.class_name);
            pub_contract_str.push('{');
            for p in &c.constructor_params {
                pub_contract_str.push_str(&p.name);
                pub_contract_str.push(':');
                pub_contract_str.push_str(&p.param_type);
                pub_contract_str.push(';');
            }
            for m in &c.methods {
                if m.is_public {
                    pub_contract_str.push_str(&m.name);
                    pub_contract_str.push('(');
                    for param in &m.parameters {
                        pub_contract_str.push_str(&param.name);
                        pub_contract_str.push(':');
                        pub_contract_str.push_str(&param.param_type);
                        pub_contract_str.push(',');
                    }
                    pub_contract_str.push(')');
                    if let Some(ref rt) = m.return_type {
                        pub_contract_str.push(':');
                        pub_contract_str.push_str(rt);
                    }
                    pub_contract_str.push(';');
                }
            }
            pub_contract_str.push('}');
        }

        let mut contract_hasher = Sha256::new();
        contract_hasher.update(pub_contract_str.as_bytes());
        let public_contract_hash = format!("contract_sha256:{:x}", contract_hasher.finalize());

        (impl_hash, public_contract_hash)
    }

    fn parse_param_def(param_str: &str) -> (String, String, Option<serde_json::Value>) {
        let parts_eq: Vec<&str> = param_str.splitn(2, '=').collect();
        let left = parts_eq[0].trim();
        let default_val = if parts_eq.len() > 1 {
            let raw_val = parts_eq[1].trim();
            if let Ok(num) = raw_val.parse::<i64>() {
                Some(serde_json::json!(num))
            } else if let Ok(num) = raw_val.parse::<f64>() {
                Some(serde_json::json!(num))
            } else if raw_val == "true" {
                Some(serde_json::json!(true))
            } else if raw_val == "false" {
                Some(serde_json::json!(false))
            } else if raw_val == "null" {
                Some(serde_json::Value::Null)
            } else if raw_val == "[]" {
                Some(serde_json::json!([]))
            } else if (raw_val.starts_with('\'') && raw_val.ends_with('\''))
                || (raw_val.starts_with('"') && raw_val.ends_with('"'))
            {
                let unquoted = raw_val.trim_matches('\'').trim_matches('"');
                Some(serde_json::json!(unquoted))
            } else {
                None
            }
        } else {
            None
        };

        let tokens: Vec<&str> = left.split_whitespace().collect();
        let (ptype, pname) = if tokens.len() >= 2 {
            let name_part = tokens.last().unwrap().trim_start_matches('$').to_string();
            let type_part = tokens[tokens.len() - 2].to_string();
            (type_part, name_part)
        } else if tokens.len() == 1 {
            ("mixed".to_string(), tokens[0].trim_start_matches('$').to_string())
        } else {
            ("mixed".to_string(), "unknown".to_string())
        };

        (ptype, pname, default_val)
    }

    fn extract_class_body<'a>(content: &'a str, class_name: &str) -> Option<&'a str> {
        let pattern = format!(r"(?:final\s+|readonly\s+|abstract\s+)*class\s+{}\b[^{{]*\{{", regex::escape(class_name));
        let re = Regex::new(&pattern).ok()?;
        let mat = re.find(content)?;
        let start_brace = mat.end() - 1;

        let mut depth = 0;
        let mut end = start_brace;
        for (idx, b) in content[start_brace..].bytes().enumerate() {
            if b == b'{' {
                depth += 1;
            } else if b == b'}' {
                depth -= 1;
                if depth == 0 {
                    end = start_brace + idx;
                    break;
                }
            }
        }

        if depth == 0 && end > start_brace {
            Some(&content[start_brace + 1..end])
        } else {
            None
        }
    }

    pub fn parse_php(content: &str) -> Result<Vec<PhpClassMetadata>, ExtractorError> {
        let mut classes = Vec::new();

        let ns_regex = Regex::new(r"namespace\s+([^;{\s]+);").unwrap();
        let namespace = ns_regex
            .captures(content)
            .map(|c| c[1].trim().to_string())
            .unwrap_or_default();

        let class_regex = Regex::new(r"(?:final\s+|readonly\s+|abstract\s+)*class\s+([A-Za-z0-9_]+)").unwrap();
        let mut class_names = Vec::new();
        for cap in class_regex.captures_iter(content) {
            class_names.push(cap[1].to_string());
        }

        if class_names.is_empty() {
            return Ok(classes);
        }

        let requires_fallback = content.contains("@template") || content.contains("@psalm-") || content.contains("array{");

        for class_name in class_names {
            let class_content = Self::extract_class_body(content, &class_name).unwrap_or(content);

            let mut constructor_params = Vec::new();
            let mut static_factories = Vec::new();
            let public_properties = Vec::new();
            let mut methods = Vec::new();

            // Constructor Property Promotion and Standard Constructor
            let ctor_regex = Regex::new(r"public\s+function\s+__construct\s*\(([^)]*)\)").unwrap();
            if let Some(cap) = ctor_regex.captures(class_content) {
                let params_str = &cap[1];
                for param_def in params_str.split(',') {
                    let trimmed = param_def.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    let (param_type, param_name, default_value) = Self::parse_param_def(trimmed);

                    constructor_params.push(ParameterDefinition {
                        name: param_name,
                        param_type,
                        constraints: Vec::new(),
                        default_value,
                    });
                }
            }

            // Static factories
            let factory_regex = Regex::new(r"public\s+static\s+function\s+([A-Za-z0-9_]+)\s*\([^)]*\)\s*:\s*(self|static|[A-Za-z0-9_\\]+)").unwrap();
            for cap in factory_regex.captures_iter(class_content) {
                static_factories.push(cap[1].to_string());
            }

            // Methods
            let method_regex = Regex::new(r"(public|protected|private)?\s*(static\s+)?function\s+([A-Za-z0-9_]+)\s*\(([^)]*)\)(?:\s*:\s*([A-Za-z0-9_\\]+))?").unwrap();
            for cap in method_regex.captures_iter(class_content) {
                let visibility = cap.get(1).map_or("public", |m| m.as_str());
                let is_static = cap.get(2).is_some();
                let method_name = cap[3].to_string();
                let raw_params = &cap[4];
                let return_type = cap.get(5).map(|m| m.as_str().to_string());

                let mut params = Vec::new();
                for p in raw_params.split(',') {
                    let p_trim = p.trim();
                    if p_trim.is_empty() {
                        continue;
                    }
                    let (ptype, pname, pdefault) = Self::parse_param_def(p_trim);

                    let mut constraints = Vec::new();
                    if ptype == "float" || ptype == "int" {
                        constraints.push("finite".to_string());
                        if class_content.contains(&format!("${} < 0", pname)) || class_content.contains(&format!("${} <= 0", pname)) {
                            constraints.push(">= 0.0".to_string());
                        }
                    } else if ptype == "string" {
                        let has_non_empty_guard = class_content.contains(&format!("empty(${})", pname))
                            || class_content.contains(&format!("${} === ''", pname))
                            || class_content.contains(&format!("${} === \"\"", pname))
                            || class_content.contains(&format!("trim(${}) === ''", pname))
                            || class_content.contains(&format!("strlen(${}) === 0", pname))
                            || class_content.contains(&format!("non-empty-string ${}", pname));
                        if has_non_empty_guard {
                            constraints.push("non-empty-string".to_string());
                        }
                        if class_content.contains(&format!("strlen(${}) !== 2", pname)) || class_content.contains(&format!("strlen(${}) != 2", pname)) {
                            constraints.push("length == 2".to_string());
                        }
                    }

                    params.push(ParameterDefinition {
                        name: pname,
                        param_type: ptype,
                        constraints,
                        default_value: pdefault,
                    });
                }

                // Detect side effects in method body
                let mut side_effects = Vec::new();
                for ctor_p in &constructor_params {
                    let _target_call = format!("->{}->", ctor_p.name);
                    let target_method_regex = Regex::new(&format!(r"\$this->{}->([A-Za-z0-9_]+)\(", ctor_p.name)).unwrap();
                    for call_cap in target_method_regex.captures_iter(class_content) {
                        side_effects.push(DetectedSideEffect {
                            effect_type: "interface_call".to_string(),
                            target: format!("{}.{}", ctor_p.name, &call_cap[1]),
                            mockable: true,
                        });
                    }
                }

                // Clock reads
                if class_content.contains("new \\DateTimeImmutable()") || class_content.contains("new DateTimeImmutable()") {
                    side_effects.push(DetectedSideEffect {
                        effect_type: "clock_read".to_string(),
                        target: "clock.now".to_string(),
                        mockable: true,
                    });
                }

                // Unmockable side effects
                if class_content.contains("$_ENV") || class_content.contains("$_SERVER") || class_content.contains("exec(") || class_content.contains("system(") {
                    side_effects.push(DetectedSideEffect {
                        effect_type: "unmockable_side_effect".to_string(),
                        target: "global.state".to_string(),
                        mockable: false,
                    });
                }

                methods.push(PhpMethodMetadata {
                    name: method_name,
                    is_static,
                    is_public: visibility == "public",
                    parameters: params,
                    return_type,
                    detected_side_effects: side_effects,
                    throws: Vec::new(),
                });
            }

            classes.push(PhpClassMetadata {
                namespace: namespace.clone(),
                class_name,
                constructor_params,
                static_factories,
                public_properties,
                methods,
                requires_fallback,
            });
        }

        Ok(classes)
    }
}
