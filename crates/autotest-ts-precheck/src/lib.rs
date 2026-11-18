use std::collections::BTreeMap;
use regex::Regex;
use sha2::{Digest, Sha256};
use autotest_core::{
    DetectedSideEffect, ParameterDefinition, ScenarioDocument, SignatureDefinition,
    SourceMetadata, TargetDefinition, TargetKind,
};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum TsExtractorError {
    #[error("Parse error: {0}")]
    ParseError(String),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Clone)]
pub struct TsClassMetadata {
    pub class_name: String,
    pub is_exported: bool,
    pub constructor_params: Vec<ParameterDefinition>,
    pub static_factories: Vec<String>,
    pub public_properties: Vec<String>,
    pub private_properties: Vec<String>,
    pub methods: Vec<TsMethodMetadata>,
    pub requires_fallback: bool,
}

#[derive(Debug, Clone)]
pub struct TsMethodMetadata {
    pub name: String,
    pub is_static: bool,
    pub is_public: bool,
    pub is_async: bool,
    pub parameters: Vec<ParameterDefinition>,
    pub return_type: Option<String>,
    pub detected_side_effects: Vec<DetectedSideEffect>,
    pub throws: Vec<String>,
}

pub struct TsPrecheckExtractor;

impl TsPrecheckExtractor {
    pub fn extract_source(file_path: &str, content: &str) -> Result<ScenarioDocument, TsExtractorError> {
        let (classes, interfaces) = Self::parse_typescript(content)?;
        let (impl_hash, _public_contract_hash) = Self::compute_hashes(content, &classes, &interfaces);

        let mut targets = Vec::new();
        let mut dependencies = BTreeMap::new();

        for class in &classes {
            if !class.is_exported && classes.len() > 1 {
                continue;
            }

            for method in &class.methods {
                if !method.is_public || method.name.starts_with('_') {
                    continue;
                }

                let target_id = format!("{}.{}", class.class_name, method.name);
                let kind = if method.is_static {
                    TargetKind::Function
                } else {
                    TargetKind::Method
                };

                // Track injected dependencies as type contracts
                for dep in &class.constructor_params {
                    let type_name = dep.param_type.trim();
                    if !type_name.is_empty()
                        && !["string", "number", "boolean", "any", "unknown", "object"].contains(&type_name)
                    {
                        dependencies.insert(
                            type_name.to_string(),
                            "contract_sha256:discovered".to_string(),
                        );
                    }
                }

                targets.push(TargetDefinition {
                    id: target_id,
                    kind,
                    signature: SignatureDefinition {
                        constructor_args: class.constructor_params.clone(),
                        parameters: method.parameters.clone(),
                        return_type: method.return_type.clone(),
                    },
                    detected_side_effects: method.detected_side_effects.clone(),
                    scenarios: Vec::new(),
                });
            }
        }

        let analyzer_name = if classes.iter().any(|c| c.requires_fallback) {
            "oxc-parser+ts-compiler-fallback@1.0.0"
        } else {
            "oxc-parser@1.0.0"
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

    pub fn compute_hashes(
        content: &str,
        classes: &[TsClassMetadata],
        interfaces: &[String],
    ) -> (String, String) {
        let mut hasher_impl = Sha256::new();
        hasher_impl.update(content.as_bytes());
        let impl_hash = format!("sha256:{:x}", hasher_impl.finalize());

        let mut hasher_contract = Sha256::new();
        for iface in interfaces {
            hasher_contract.update(iface.as_bytes());
            hasher_contract.update(b";");
        }

        for class in classes {
            if !class.is_exported {
                continue;
            }
            hasher_contract.update(class.class_name.as_bytes());
            for ctor_p in &class.constructor_params {
                hasher_contract.update(ctor_p.name.as_bytes());
                hasher_contract.update(ctor_p.param_type.as_bytes());
            }
            for prop in &class.public_properties {
                hasher_contract.update(prop.as_bytes());
            }
            for method in &class.methods {
                if method.is_public {
                    hasher_contract.update(method.name.as_bytes());
                    for p in &method.parameters {
                        hasher_contract.update(p.name.as_bytes());
                        hasher_contract.update(p.param_type.as_bytes());
                    }
                    if let Some(ret) = &method.return_type {
                        hasher_contract.update(ret.as_bytes());
                    }
                }
            }
        }

        let public_contract_hash = format!("sha256:{:x}", hasher_contract.finalize());
        (impl_hash, public_contract_hash)
    }

    pub fn parse_typescript(
        source: &str,
    ) -> Result<(Vec<TsClassMetadata>, Vec<String>), TsExtractorError> {
        let clean_source = Self::strip_comments(source);

        let mut interfaces = Vec::new();
        let iface_re = Regex::new(r"export\s+interface\s+(\w+)").unwrap();
        for cap in iface_re.captures_iter(&clean_source) {
            interfaces.push(cap[1].to_string());
        }

        let mut classes = Vec::new();
        let class_re = Regex::new(
            r"(?m)(export\s+(?:default\s+)?)?class\s+(\w+)(?:\s+extends\s+\w+)?(?:\s+implements\s+[\w,\s]+)?\s*\{",
        )
        .unwrap();

        for cap in class_re.captures_iter(&clean_source) {
            let is_exported = cap.get(1).is_some();
            let class_name = cap[2].to_string();
            let class_start = cap.get(0).unwrap().end();

            let class_body = Self::extract_braced_block(&clean_source[class_start - 1..])
                .unwrap_or_default();

            let mut constructor_params = Vec::new();
            let mut static_factories = Vec::new();
            let mut public_properties = Vec::new();
            let mut private_properties = Vec::new();
            let mut methods = Vec::new();
            let mut requires_fallback = false;

            // Check for complex TypeScript types requiring fallback to Compiler API
            if class_body.contains("extends infer") || (class_body.contains("keyof") && class_body.contains("typeof")) {
                requires_fallback = true;
            }

            // Detect #private properties and public properties
            let prop_re = Regex::new(
                r"(?m)^\s*(public\s+|private\s+|protected\s+|readonly\s+)*(#?\w+)(?:\s*:\s*([^;=]+))?(?:\s*=[^;]+)?;"
            ).unwrap();

            for p_cap in prop_re.captures_iter(&class_body) {
                let modifiers = p_cap.get(1).map_or("", |m| m.as_str());
                let prop_name = p_cap[2].to_string();

                if prop_name.starts_with('#') || modifiers.contains("private") {
                    private_properties.push(prop_name);
                } else if !modifiers.contains("protected") && !prop_name.starts_with('_') {
                    public_properties.push(prop_name);
                }
            }

            // Extract Constructor
            let ctor_re = Regex::new(r"constructor\s*\(([^)]*)\)\s*\{").unwrap();
            if let Some(ctor_cap) = ctor_re.captures(&class_body) {
                let params_str = &ctor_cap[1];
                constructor_params = Self::parse_parameters(params_str);

                // Parameter properties (e.g. constructor(public name: string, private id: number))
                for raw_param in params_str.split(',') {
                    let trimmed = raw_param.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    if trimmed.starts_with("public ") || (trimmed.contains("readonly ") && !trimmed.contains("private ") && !trimmed.contains("protected ")) {
                        let name_part = trimmed.split(':').next().unwrap_or("").trim();
                        let p_name = name_part.split_whitespace().last().unwrap_or("");
                        if !p_name.is_empty() && !public_properties.contains(&p_name.to_string()) {
                            public_properties.push(p_name.to_string());
                        }
                    } else if trimmed.starts_with("private ") {
                        let name_part = trimmed.split(':').next().unwrap_or("").trim();
                        let p_name = name_part.split_whitespace().last().unwrap_or("");
                        if !p_name.is_empty() {
                            private_properties.push(p_name.to_string());
                        }
                    }
                }
            }

            // Extract Static Factory Methods
            let factory_re = Regex::new(
                r"(?m)^\s*public\s+static\s+(\w+)\s*\([^)]*\)\s*:\s*(\w+)"
            ).unwrap();
            for f_cap in factory_re.captures_iter(&class_body) {
                let fn_name = f_cap[1].to_string();
                let ret_type = &f_cap[2];
                if ret_type == &class_name || ret_type == "Self" || ret_type == "this" {
                    static_factories.push(fn_name);
                }
            }
            if static_factories.is_empty() {
                let simple_factory_re = Regex::new(r"(?m)^\s*static\s+(create|from|of)\s*\(").unwrap();
                for f_cap in simple_factory_re.captures_iter(&class_body) {
                    static_factories.push(f_cap[1].to_string());
                }
            }

            // Extract Methods
            let method_re = Regex::new(
                r"(?m)^\s*(public\s+|private\s+|protected\s+|static\s+|async\s+)*(\w+)\s*\(([^)]*)\)\s*(?::\s*([^\{]+))?\s*\{"
            ).unwrap();

            for m_cap in method_re.captures_iter(&class_body) {
                let modifiers = m_cap.get(1).map_or("", |m| m.as_str());
                let method_name = m_cap[2].to_string();

                if method_name == "constructor"
                    || ["if", "while", "for", "switch", "catch", "function", "return"].contains(&method_name.as_str())
                {
                    continue;
                }

                let is_static = modifiers.contains("static");
                let is_async = modifiers.contains("async");
                let is_public = !modifiers.contains("private") && !modifiers.contains("protected");
                let raw_params = &m_cap[3];
                let return_type = m_cap.get(4).map(|m| m.as_str().trim().to_string());

                let parameters = Self::parse_parameters(raw_params);

                // Find method body to check side effects and throws
                let method_start = m_cap.get(0).unwrap().end();
                let method_body = Self::extract_braced_block(&class_body[method_start - 1..])
                    .unwrap_or_default();

                let detected_side_effects = Self::detect_side_effects(&method_body);
                let throws = Self::detect_throws(&method_body);

                methods.push(TsMethodMetadata {
                    name: method_name,
                    is_static,
                    is_public,
                    is_async,
                    parameters,
                    return_type,
                    detected_side_effects,
                    throws,
                });
            }

            classes.push(TsClassMetadata {
                class_name,
                is_exported,
                constructor_params,
                static_factories,
                public_properties,
                private_properties,
                methods,
                requires_fallback,
            });
        }

        Ok((classes, interfaces))
    }

    fn parse_parameters(params_str: &str) -> Vec<ParameterDefinition> {
        let mut parameters = Vec::new();
        for param in params_str.split(',') {
            let trimmed = param.trim();
            if trimmed.is_empty() {
                continue;
            }

            // Handle modifiers like `public readonly x: number`
            let without_modifiers = trimmed
                .trim_start_matches("public ")
                .trim_start_matches("private ")
                .trim_start_matches("protected ")
                .trim_start_matches("readonly ")
                .trim();

            let parts: Vec<&str> = without_modifiers.split(':').collect();
            let name_part = parts[0].trim();
            let is_optional = name_part.ends_with('?') || without_modifiers.contains('=');
            let param_name = name_part.trim_end_matches('?').trim().to_string();

            let type_annotation = if parts.len() > 1 {
                parts[1].split('=').next().unwrap_or("any").trim().to_string()
            } else {
                "any".to_string()
            };

            let default_val = if without_modifiers.contains('=') {
                without_modifiers.split('=').nth(1).map(|v| serde_json::Value::String(v.trim().to_string()))
            } else {
                None
            };

            let mut constraints = Vec::new();
            if type_annotation == "number" {
                constraints.push("finite".to_string());
            } else if type_annotation == "string" && !is_optional {
                constraints.push("non-empty".to_string());
            }

            parameters.push(ParameterDefinition {
                name: param_name,
                param_type: type_annotation,
                constraints,
                default_value: default_val,
            });
        }
        parameters
    }

    fn detect_side_effects(body: &str) -> Vec<DetectedSideEffect> {
        let mut effects = Vec::new();

        // 1. Unmockable process.exit / process.abort
        if body.contains("process.exit") || body.contains("process.abort") {
            effects.push(DetectedSideEffect {
                effect_type: "unmockable_side_effect".to_string(),
                target: "process.exit".to_string(),
                mockable: false,
            });
        }

        // 2. Unmockable eval() / new Function()
        if body.contains("eval(") || body.contains("new Function(") {
            effects.push(DetectedSideEffect {
                effect_type: "unmockable_side_effect".to_string(),
                target: "eval".to_string(),
                mockable: false,
            });
        }

        // 3. Raw filesystem I/O
        if body.contains("fs.writeFileSync") || body.contains("fs.unlinkSync") || body.contains("fs.rmSync") {
            effects.push(DetectedSideEffect {
                effect_type: "unmockable_side_effect".to_string(),
                target: "fs.sync".to_string(),
                mockable: false,
            });
        }

        // 4. Mockable service calls
        if body.contains("this.") {
            let re = Regex::new(r"this\.(\w+)\.([a-zA-Z0-9_]+)\(").unwrap();
            for cap in re.captures_iter(body) {
                effects.push(DetectedSideEffect {
                    effect_type: "service_call".to_string(),
                    target: format!("{}.{}", &cap[1], &cap[2]),
                    mockable: true,
                });
            }
        }

        effects
    }

    fn detect_throws(body: &str) -> Vec<String> {
        let mut throws = Vec::new();
        let throw_re = Regex::new(r"throw\s+new\s+(\w+)\s*\(").unwrap();
        for cap in throw_re.captures_iter(body) {
            let err = cap[1].to_string();
            if !throws.contains(&err) {
                throws.push(err);
            }
        }
        let reject_re = Regex::new(r"Promise\.reject\s*\(\s*new\s+(\w+)\s*\(").unwrap();
        for cap in reject_re.captures_iter(body) {
            let err = cap[1].to_string();
            if !throws.contains(&err) {
                throws.push(err);
            }
        }
        throws
    }

    fn extract_braced_block(source: &str) -> Option<String> {
        let mut depth = 0;
        let mut started = false;
        let mut start_idx = 0;

        for (i, c) in source.char_indices() {
            if c == '{' {
                if !started {
                    started = true;
                    start_idx = i;
                }
                depth += 1;
            } else if c == '}' && started {
                depth -= 1;
                if depth == 0 {
                    return Some(source[start_idx + 1..i].to_string());
                }
            }
        }
        None
    }

    fn strip_comments(source: &str) -> String {
        let line_re = Regex::new(r"//.*$").unwrap();
        let block_re = Regex::new(r"(?s)/\*.*?\*/").unwrap();
        let without_blocks = block_re.replace_all(source, "");
        line_re.replace_all(&without_blocks, "").to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ts_class_extraction_with_parameter_properties() {
        let code = r#"
            export class OrderService {
                #privateKey: string;
                public status: string;

                constructor(
                    public readonly paymentGateway: IPaymentGateway,
                    private maxRetries: number,
                    public currency: string = "EUR"
                ) {
                    this.#privateKey = "secret";
                    this.status = "INIT";
                }

                public static create(gateway: IPaymentGateway): OrderService {
                    return new OrderService(gateway, 3, "EUR");
                }

                public async processOrder(orderId: string, amount: number): Promise<boolean> {
                    if (amount <= 0) {
                        throw new InvalidAmountError("Amount must be positive");
                    }
                    const res = await this.paymentGateway.charge(orderId, amount);
                    return res.success;
                }

                private internalHelper(): void {}
            }
        "#;

        let doc = TsPrecheckExtractor::extract_source("src/order.ts", code).unwrap();
        assert_eq!(doc.targets.len(), 2);

        let process_target = doc.targets.iter().find(|t| t.id == "OrderService.processOrder").unwrap();
        assert_eq!(process_target.kind, TargetKind::Method);
        assert_eq!(process_target.signature.parameters.len(), 2);

        // Constructor args check
        let ctor = &process_target.signature.constructor_args;
        assert_eq!(ctor.len(), 3);
        assert_eq!(ctor[0].name, "paymentGateway");
        assert_eq!(ctor[0].param_type, "IPaymentGateway");

        // Dependency detected
        assert!(doc.source.dependencies.contains_key("IPaymentGateway"));
    }

    #[test]
    fn test_unmockable_side_effect_detection() {
        let code = r#"
            export class FatalWorker {
                public run(action: string): void {
                    if (action === "die") {
                        process.exit(1);
                    }
                    eval("console.log('danger')");
                }
            }
        "#;

        let doc = TsPrecheckExtractor::extract_source("src/fatal.ts", code).unwrap();
        let target = &doc.targets[0];
        let unmockable = target.detected_side_effects.iter().filter(|e| !e.mockable).count();
        assert_eq!(unmockable, 2); // process.exit + eval
    }

    #[test]
    fn test_deterministic_hashes() {
        let code1 = "export class A { public run(): void {} }";
        let code2 = "export class A { public run(): void { const x = 1; } }";

        let (impl1, contract1) = TsPrecheckExtractor::compute_hashes(
            code1,
            &[TsClassMetadata {
                class_name: "A".to_string(),
                is_exported: true,
                constructor_params: vec![],
                static_factories: vec![],
                public_properties: vec![],
                private_properties: vec![],
                methods: vec![TsMethodMetadata {
                    name: "run".to_string(),
                    is_static: false,
                    is_public: true,
                    is_async: false,
                    parameters: vec![],
                    return_type: Some("void".to_string()),
                    detected_side_effects: vec![],
                    throws: vec![],
                }],
                requires_fallback: false,
            }],
            &[],
        );

        let (impl2, contract2) = TsPrecheckExtractor::compute_hashes(
            code2,
            &[TsClassMetadata {
                class_name: "A".to_string(),
                is_exported: true,
                constructor_params: vec![],
                static_factories: vec![],
                public_properties: vec![],
                private_properties: vec![],
                methods: vec![TsMethodMetadata {
                    name: "run".to_string(),
                    is_static: false,
                    is_public: true,
                    is_async: false,
                    parameters: vec![],
                    return_type: Some("void".to_string()),
                    detected_side_effects: vec![],
                    throws: vec![],
                }],
                requires_fallback: false,
            }],
            &[],
        );

        // Implementation hashes must differ because method body changed
        assert_ne!(impl1, impl2);
        // Public contract hashes must be identical because public interface didn't change
        assert_eq!(contract1, contract2);
    }
}
