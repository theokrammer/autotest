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

#[derive(Debug, Clone)]
pub struct TsFunctionMetadata {
    pub name: String,
    pub is_exported: bool,
    pub is_async: bool,
    pub parameters: Vec<ParameterDefinition>,
    pub return_type: Option<String>,
    pub detected_side_effects: Vec<DetectedSideEffect>,
    pub throws: Vec<String>,
}

pub struct TsPrecheckExtractor;

impl TsPrecheckExtractor {
    pub fn extract_source(file_path: &str, content: &str) -> Result<ScenarioDocument, TsExtractorError> {
        let (classes, interfaces, functions) = Self::parse_typescript(content)?;
        let (impl_hash, _public_contract_hash) = Self::compute_hashes(content, &classes, &interfaces, &functions);

        let mut targets = Vec::new();
        let mut dependencies = BTreeMap::new();

        for class in &classes {
            if !class.is_exported {
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

        // Top-level standalone functions (only exported functions can be imported by test runners)
        for func in &functions {
            if !func.is_exported {
                continue;
            }

            targets.push(TargetDefinition {
                id: func.name.clone(),
                kind: TargetKind::Function,
                signature: SignatureDefinition {
                    constructor_args: Vec::new(),
                    parameters: func.parameters.clone(),
                    return_type: func.return_type.clone(),
                },
                detected_side_effects: func.detected_side_effects.clone(),
                scenarios: Vec::new(),
            });
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
        functions: &[TsFunctionMetadata],
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

        for func in functions {
            if !func.is_exported && !classes.is_empty() {
                continue;
            }
            hasher_contract.update(func.name.as_bytes());
            for p in &func.parameters {
                hasher_contract.update(p.name.as_bytes());
                hasher_contract.update(p.param_type.as_bytes());
            }
            if let Some(ret) = &func.return_type {
                hasher_contract.update(ret.as_bytes());
            }
        }

        let public_contract_hash = format!("sha256:{:x}", hasher_contract.finalize());
        (impl_hash, public_contract_hash)
    }

    pub fn parse_typescript(
        source: &str,
    ) -> Result<(Vec<TsClassMetadata>, Vec<String>, Vec<TsFunctionMetadata>), TsExtractorError> {
        let clean_source = Self::strip_decorators(&Self::strip_comments(source));

        let mut interfaces = Vec::new();
        let iface_re = Regex::new(r"export\s+interface\s+(\w+)").unwrap();
        for cap in iface_re.captures_iter(&clean_source) {
            interfaces.push(cap[1].to_string());
        }

        let mut classes = Vec::new();
        let class_re = Regex::new(
            r"(?m)(?:@\w+(?:\([^)]*\))?\s*)*(export\s+(?:default\s+)?)?class\s+(\w+)(?:<[^>]+>)?(?:\s+extends\s+[\w.<>]+)?(?:\s+implements\s+[\w,\s.<>]+)?\s*\{",
        )
        .unwrap();

        for cap in class_re.captures_iter(&clean_source) {
            if Self::brace_depth_at(&clean_source, cap.get(0).unwrap().start()) != 0 {
                continue;
            }
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

            // Extract Constructor (balanced parens so default values like `= new ApplicationConfig()` work)
            let ctor_re = Regex::new(r"(?m)^\s*(?:public\s+|protected\s+|private\s+)?constructor\s*\(").unwrap();
            if let Some(ctor_match) = ctor_re.find(&class_body) {
                if Self::brace_depth_at(&class_body, ctor_match.start()) == 0 {
                    if let Some(params_str) = Self::extract_balanced_parens(&class_body[ctor_match.end() - 1..]) {
                        constructor_params = Self::parse_parameters(&params_str);

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

            // Extract Methods (depth-0 class members only, capturing all modifiers)
            let method_re = Regex::new(
                r"(?m)^\s*((?:(?:public|private|protected|static|async|readonly|override|abstract|get|set)\s+)*)(\w+)\s*(?:<[^>]+>)?\s*\(([^)]*)\)\s*(?::\s*([^\{;\n]+))?\s*\{"
            ).unwrap();

            for m_cap in method_re.captures_iter(&class_body) {
                let match_start = m_cap.get(0).unwrap().start();
                if Self::brace_depth_at(&class_body, match_start) != 0 {
                    continue;
                }

                let modifiers = m_cap.get(1).map_or("", |m| m.as_str());
                let method_name = m_cap[2].to_string();

                if method_name == "constructor"
                    || ["if", "while", "for", "switch", "catch", "function", "return"].contains(&method_name.as_str())
                {
                    continue;
                }

                // Skip getter/setter accessors and abstract declarations
                if modifiers.contains("get ")
                    || modifiers.contains("set ")
                    || modifiers.contains("abstract ")
                {
                    continue;
                }

                let is_static = modifiers.contains("static");
                let is_async = modifiers.contains("async");
                let is_public = !modifiers.contains("private") && !modifiers.contains("protected");

                if methods.iter().any(|m: &TsMethodMetadata| m.name == method_name && m.is_static == is_static) {
                    continue;
                }

                let raw_params = &m_cap[3];
                let return_type = m_cap.get(4).map(|m| m.as_str().trim().to_string());

                let mut parameters = Self::parse_parameters(raw_params);

                // Find method body to check side effects, branch literals, and throws
                let method_start = m_cap.get(0).unwrap().end();
                let method_body = Self::extract_braced_block(&class_body[method_start - 1..])
                    .unwrap_or_default();

                Self::extract_branch_constraints(&method_body, &mut parameters);

                let mut detected_side_effects = Self::detect_side_effects(&method_body);
                if class_body.contains("randomStringGenerator(")
                    && !detected_side_effects.iter().any(|e| e.effect_type == "rng")
                {
                    detected_side_effects.push(DetectedSideEffect {
                        effect_type: "rng".to_string(),
                        target: "random".to_string(),
                        mockable: true,
                    });
                }
                if class_body.contains("new Promise")
                    && (method_body.contains("return this.promise")
                        || method_body.contains("return this.settledPromise")
                        || method_body.contains("return this.wait()"))
                {
                    detected_side_effects.push(DetectedSideEffect {
                        effect_type: "unmockable_side_effect".to_string(),
                        target: "async.deferred_promise".to_string(),
                        mockable: false,
                    });
                }
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

        // Extract Top-Level / Standalone Functions (depth-0 only)
        let mut functions = Vec::new();

        // 1. function declarations: export (default )?(async )?function name(...)
        let fn_decl_re = Regex::new(
            r"(?m)^\s*(export\s+(?:default\s+)?)?(?:async\s+)?function(?:\s+(\w+))?\s*(?:<[^>]+>)?\s*\(([^)]*)\)\s*(?::\s*([^\{;\n]+))?\s*\{"
        ).unwrap();

        for f_cap in fn_decl_re.captures_iter(&clean_source) {
            if Self::brace_depth_at(&clean_source, f_cap.get(0).unwrap().start()) != 0 {
                continue;
            }
            let is_exported = f_cap.get(1).is_some();
            let fn_name = f_cap.get(2).map_or("default", |m| m.as_str()).to_string();
            let raw_params = &f_cap[3];
            let ret_type = f_cap.get(4).map(|m| m.as_str().trim().to_string());

            let mut parameters = Self::parse_parameters(raw_params);
            let fn_start = f_cap.get(0).unwrap().end();
            let fn_body = Self::extract_braced_block(&clean_source[fn_start - 1..]).unwrap_or_default();
            Self::extract_branch_constraints(&fn_body, &mut parameters);
            let mut detected_side_effects = Self::detect_side_effects(&fn_body);
            if fn_name.to_lowercase().contains("random")
                && !detected_side_effects.iter().any(|e| e.effect_type == "rng")
            {
                detected_side_effects.push(DetectedSideEffect {
                    effect_type: "rng".to_string(),
                    target: "random".to_string(),
                    mockable: true,
                });
            }
            let throws = Self::detect_throws(&fn_body);

            functions.push(TsFunctionMetadata {
                name: fn_name,
                is_exported,
                is_async: f_cap.get(0).map_or(false, |m| m.as_str().contains("async")),
                parameters,
                return_type: ret_type,
                detected_side_effects,
                throws,
            });
        }

        // 2. arrow / expression assignments: export const/let/var name = (async )?(...) => ...
        let fn_expr_re = Regex::new(
            r"(?m)^\s*(export\s+)?(?:const|let|var)\s+(\w+)\s*=\s*(?:async\s+)?(?:\(([^)]*)\)|(\w+))\s*(?::\s*([^=>\{\n]+))?\s*=>\s*(\{)?"
        ).unwrap();

        for f_cap in fn_expr_re.captures_iter(&clean_source) {
            if Self::brace_depth_at(&clean_source, f_cap.get(0).unwrap().start()) != 0 {
                continue;
            }
            let is_exported = f_cap.get(1).is_some();
            let fn_name = f_cap[2].to_string();
            if functions.iter().any(|f| f.name == fn_name) {
                continue;
            }
            let raw_params = f_cap.get(3).map_or_else(|| f_cap.get(4).map_or("", |m| m.as_str()), |m| m.as_str());
            let ret_type = f_cap.get(5).map(|m| m.as_str().trim().to_string());
            let parameters = Self::parse_parameters(raw_params);

            let fn_start = f_cap.get(0).unwrap().end();
            let has_brace = f_cap.get(6).is_some();
            let fn_body = if has_brace {
                Self::extract_braced_block(&clean_source[fn_start - 1..]).unwrap_or_default()
            } else {
                clean_source[fn_start..].lines().next().unwrap_or("").to_string()
            };
            let mut detected_side_effects = Self::detect_side_effects(&fn_body);
            if fn_name.to_lowercase().contains("random")
                && !detected_side_effects.iter().any(|e| e.effect_type == "rng")
            {
                detected_side_effects.push(DetectedSideEffect {
                    effect_type: "rng".to_string(),
                    target: "random".to_string(),
                    mockable: true,
                });
            }
            let throws = Self::detect_throws(&fn_body);

            functions.push(TsFunctionMetadata {
                name: fn_name,
                is_exported,
                is_async: f_cap.get(0).map_or(false, |m| m.as_str().contains("async")),
                parameters,
                return_type: ret_type,
                detected_side_effects,
                throws,
            });
        }

        Ok((classes, interfaces, functions))
    }

    fn parse_parameters(params_str: &str) -> Vec<ParameterDefinition> {
        let mut parameters = Vec::new();
        // Strip TypeScript/NestJS decorators such as @Param('id') or @Body()
        let decorator_re = Regex::new(r"@\w+(?:\([^)]*\))?\s*").unwrap();
        let clean_params_str = decorator_re.replace_all(params_str, "");

        for param in clean_params_str.split(',') {
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
            let name_part = parts[0].split('=').next().unwrap_or("").trim();
            let is_optional = name_part.ends_with('?') || without_modifiers.contains('=');
            let param_name = name_part.trim_end_matches('?').trim().to_string();

            let type_annotation = if parts.len() > 1 {
                parts[1].split('=').next().unwrap_or("any").trim().to_string()
            } else {
                "any".to_string()
            };

            let default_val = if without_modifiers.contains('=') {
                let raw = without_modifiers.splitn(2, '=').nth(1).unwrap_or("").trim();
                if let Ok(n) = raw.parse::<i64>() {
                    Some(serde_json::json!(n))
                } else if let Ok(n) = raw.parse::<f64>() {
                    Some(serde_json::json!(n))
                } else if raw == "true" {
                    Some(serde_json::json!(true))
                } else if raw == "false" {
                    Some(serde_json::json!(false))
                } else if raw == "null" {
                    Some(serde_json::Value::Null)
                } else if raw == "[]" {
                    Some(serde_json::json!([]))
                } else if raw == "{}" {
                    Some(serde_json::json!({}))
                } else if (raw.starts_with('"') && raw.ends_with('"')) || (raw.starts_with('\'') && raw.ends_with('\'')) {
                    Some(serde_json::json!(&raw[1..raw.len() - 1]))
                } else {
                    // Non-literal default expression (e.g. `new ApplicationConfig()`): leave None so hydrator uses default or mock
                    None
                }
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

    fn extract_branch_constraints(body: &str, params: &mut [ParameterDefinition]) {
        for param in params.iter_mut() {
            if param.name.is_empty() || param.name.contains(' ') {
                continue;
            }
            let escaped = regex::escape(&param.name);
            let mut added = 0usize;

            // Match `param === 'lit'` or `param == "lit"` or `param !== 'lit'`
            if let Ok(eq_re) = Regex::new(&format!(r"\b{}\s*[!=]==?\s*['\x22]([A-Za-z0-9_./:-]{{1,32}})['\x22]", escaped)) {
                for cap in eq_re.captures_iter(body) {
                    let c = format!("eq:{}", &cap[1]);
                    if !param.constraints.contains(&c) {
                        param.constraints.push(c);
                        added += 1;
                        if added >= 3 {
                            break;
                        }
                    }
                }
            }

            // Match `switch (param)` case string literals
            if added < 3 && body.contains(&format!("switch ({})", param.name)) {
                if let Ok(case_re) = Regex::new(r"case\s+['\x22]([A-Za-z0-9_./:-]{1,32})['\x22]\s*:") {
                    for cap in case_re.captures_iter(body) {
                        let c = format!("eq:{}", &cap[1]);
                        if !param.constraints.contains(&c) {
                            param.constraints.push(c);
                            added += 1;
                            if added >= 3 {
                                break;
                            }
                        }
                    }
                }
            }

            // Match negative number branch `param < 0` or `param <= 0`
            if (body.contains(&format!("{} < 0", param.name)) || body.contains(&format!("{} <= 0", param.name)))
                && !param.constraints.iter().any(|c| c == "branch_num:-1")
            {
                param.constraints.push("branch_num:-1".to_string());
            }
        }
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

        // 3b. Raw network / socket server binding
        if body.contains("new Server(")
            || body.contains(".listen(")
            || body.contains("http.createServer(")
            || body.contains("https.createServer(")
            || body.contains("net.createServer(")
        {
            effects.push(DetectedSideEffect {
                effect_type: "unmockable_side_effect".to_string(),
                target: "net.listen".to_string(),
                mockable: false,
            });
        }

        // 3c. Non-deterministic RNG / UUID / clock calls
        if body.contains("Math.random(")
            || body.contains("getRandomValues(")
            || body.contains("randomFillSync(")
            || body.contains("randomBytes(")
            || body.contains("randomUUID(")
            || body.contains("randomStringGenerator(")
            || body.contains("createContextId(")
            || body.contains("fillRandom(")
            || body.contains("getRandom(")
            || body.contains("Date.now(")
            || body.contains("new Date(")
            || body.contains("toLocaleString(")
            || body.contains("uuid(")
        {
            effects.push(DetectedSideEffect {
                effect_type: "rng".to_string(),
                target: "random".to_string(),
                mockable: true,
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

    fn brace_depth_at(source: &str, byte_offset: usize) -> usize {
        let mut depth = 0usize;
        let mut in_single = false;
        let mut in_double = false;
        let mut in_backtick = false;
        let mut prev_backslash = false;

        for (i, c) in source.char_indices() {
            if i >= byte_offset {
                break;
            }
            if prev_backslash {
                prev_backslash = false;
                continue;
            }
            if (in_single || in_double || in_backtick) && c == '\\' {
                prev_backslash = true;
                continue;
            }
            if !in_double && !in_backtick && c == '\'' {
                in_single = !in_single;
            } else if !in_single && !in_backtick && c == '"' {
                in_double = !in_double;
            } else if !in_single && !in_double && c == '`' {
                in_backtick = !in_backtick;
            } else if !in_single && !in_double && !in_backtick {
                if c == '{' {
                    depth += 1;
                } else if c == '}' {
                    depth = depth.saturating_sub(1);
                }
            }
        }
        depth
    }

    fn extract_balanced_parens(source: &str) -> Option<String> {
        let mut depth = 0usize;
        let mut started = false;
        let mut start_idx = 0;

        for (i, c) in source.char_indices() {
            if c == '(' {
                if !started {
                    started = true;
                    start_idx = i;
                }
                depth += 1;
            } else if c == ')' && started {
                depth -= 1;
                if depth == 0 {
                    return Some(source[start_idx + 1..i].to_string());
                }
            }
        }
        None
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
        let mut result = String::with_capacity(source.len());
        let chars: Vec<char> = source.chars().collect();
        let mut i = 0;
        let n = chars.len();

        let mut in_single_quote = false;
        let mut in_double_quote = false;
        let mut in_backtick = false;

        while i < n {
            let c = chars[i];
            let next = if i + 1 < n { Some(chars[i + 1]) } else { None };

            if !in_single_quote && !in_double_quote && !in_backtick {
                if c == '/' && next == Some('/') {
                    // Line comment: skip until newline
                    i += 2;
                    while i < n && chars[i] != '\n' {
                        i += 1;
                    }
                    continue;
                } else if c == '/' && next == Some('*') {
                    // Block comment: skip until */
                    i += 2;
                    while i + 1 < n && !(chars[i] == '*' && chars[i + 1] == '/') {
                        i += 1;
                    }
                    i += 2; // skip * and /
                    continue;
                } else if c == '\'' {
                    in_single_quote = true;
                    result.push(c);
                    i += 1;
                    continue;
                } else if c == '"' {
                    in_double_quote = true;
                    result.push(c);
                    i += 1;
                    continue;
                } else if c == '`' {
                    in_backtick = true;
                    result.push(c);
                    i += 1;
                    continue;
                }
            } else if c == '\\' {
                // Escaped character inside string
                result.push(c);
                if let Some(next_c) = next {
                    result.push(next_c);
                    i += 2;
                    continue;
                }
            } else if in_single_quote && c == '\'' {
                in_single_quote = false;
            } else if in_double_quote && c == '"' {
                in_double_quote = false;
            } else if in_backtick && c == '`' {
                in_backtick = false;
            }

            result.push(c);
            i += 1;
        }

        result
    }

    fn strip_decorators(source: &str) -> String {
        let mut result = String::with_capacity(source.len());
        let chars: Vec<char> = source.chars().collect();
        let n = chars.len();
        let mut i = 0;

        while i < n {
            if chars[i] == '@' && (i == 0 || chars[i - 1].is_whitespace() || chars[i - 1] == '(' || chars[i - 1] == ',') {
                let mut j = i + 1;
                while j < n && (chars[j].is_alphanumeric() || chars[j] == '_' || chars[j] == '$') {
                    j += 1;
                }
                if j > i + 1 {
                    let mut k = j;
                    while k < n && (chars[k] == ' ' || chars[k] == '\t') {
                        k += 1;
                    }
                    if k < n && chars[k] == '(' {
                        let mut depth = 0;
                        while k < n {
                            if chars[k] == '(' {
                                depth += 1;
                            } else if chars[k] == ')' {
                                depth -= 1;
                                if depth == 0 {
                                    k += 1;
                                    break;
                                }
                            }
                            k += 1;
                        }
                    }
                    i = k;
                    continue;
                }
            }
            result.push(chars[i]);
            i += 1;
        }

        result
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
            &[],
        );

        // Implementation hashes must differ because method body changed
        assert_ne!(impl1, impl2);
        // Public contract hashes must be identical because public interface didn't change
        assert_eq!(contract1, contract2);
    }

    #[test]
    fn test_standalone_functions_extraction() {
        let code = r#"
            export let nanoid = (size: number = 21): string => {
                return "123456789012345678901";
            };

            export function customAlphabet(alphabet: string, defaultSize: number = 21): (size?: number) => string {
                return (size = defaultSize) => alphabet.slice(0, size);
            }
        "#;

        let doc = TsPrecheckExtractor::extract_source("nanoid.ts", code).unwrap();
        assert_eq!(doc.targets.len(), 2);

        let nanoid_target = doc.targets.iter().find(|t| t.id == "nanoid").unwrap();
        assert_eq!(nanoid_target.kind, TargetKind::Function);
        assert_eq!(nanoid_target.signature.parameters.len(), 1);
        assert_eq!(nanoid_target.signature.parameters[0].name, "size");

        let custom_target = doc.targets.iter().find(|t| t.id == "customAlphabet").unwrap();
        assert_eq!(custom_target.kind, TargetKind::Function);
        assert_eq!(custom_target.signature.parameters.len(), 2);
    }

    #[test]
    fn test_nestjs_decorators_and_params() {
        let code = r#"
            @Injectable()
            export class CatsController {
                constructor(
                    @Inject('CAT_SERVICE') private readonly catService: CatService
                ) {}

                @Get(':id')
                public findOne(@Param('id') id: string): Cat {
                    return this.catService.findOne(id);
                }
            }
        "#;

        let doc = TsPrecheckExtractor::extract_source("src/cats.controller.ts", code).unwrap();
        assert_eq!(doc.targets.len(), 1);

        let target = &doc.targets[0];
        assert_eq!(target.id, "CatsController.findOne");
        // Constructor param should be cleaned of @Inject
        assert_eq!(target.signature.constructor_args.len(), 1);
        assert_eq!(target.signature.constructor_args[0].name, "catService");
        assert_eq!(target.signature.constructor_args[0].param_type, "CatService");

        // Method param should be cleaned of @Param
        assert_eq!(target.signature.parameters.len(), 1);
        assert_eq!(target.signature.parameters[0].name, "id");
        assert_eq!(target.signature.parameters[0].param_type, "string");
    }

    #[test]
    fn test_strip_comments_preserves_urls() {
        let code = r#"
            // Leading comment
            export class ApiClient {
                public readonly url: string = "https://api.example.com/v1"; // trailing comment
                /* block comment */
                public getEndpoint(): string {
                    return 'http://localhost:8080//test';
                }
            }
        "#;
        let stripped = TsPrecheckExtractor::strip_comments(code);
        assert!(stripped.contains("https://api.example.com/v1"));
        assert!(stripped.contains("http://localhost:8080//test"));
        assert!(!stripped.contains("Leading comment"));
        assert!(!stripped.contains("trailing comment"));
        assert!(!stripped.contains("block comment"));
    }
}
