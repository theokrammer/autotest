use std::collections::{BTreeMap, HashMap};
use regex::Regex;
use sha2::{Digest, Sha256};
use autotest_core::{
    DetectedSideEffect, FactorySignature, ParameterDefinition, ScenarioDocument,
    SignatureDefinition, SourceMetadata, TargetDefinition, TargetKind,
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
    pub implements: Vec<String>,
    pub private_constructor: bool,
    pub constructor_params: Vec<ParameterDefinition>,
    pub static_factories: Vec<FactorySignature>,
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
                if !method.is_public {
                    continue;
                }
                if method.name.starts_with("__")
                    && method.name != "__toString"
                    && method.name != "__debugInfo"
                {
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
                        static_factories: class.static_factories.clone(),
                        implements: class.implements.clone(),
                        private_constructor: class.private_constructor,
                    },
                    detected_side_effects: method.detected_side_effects.clone(),
                    metadata_keys: Vec::new(),
                    scenarios: Vec::new(),
                };

                // Track injected dependencies as type contracts
                for dep in &class.constructor_params {
                    if dep.param_type.contains('\\')
                        || dep.param_type.ends_with("Interface")
                        || dep.param_type.ends_with("Repository")
                    {
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

    fn parse_param_def(
        param_str: &str,
        namespace: &str,
        use_map: &HashMap<String, String>,
    ) -> (String, String, Option<serde_json::Value>, bool) {
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
        let (raw_type, raw_name) = if tokens.len() >= 2 {
            let name_part = tokens.last().unwrap();
            let type_part = tokens[tokens.len() - 2];
            (type_part, *name_part)
        } else if tokens.len() == 1 {
            ("mixed", tokens[0])
        } else {
            ("mixed", "unknown")
        };

        let is_variadic = raw_name.starts_with("...");
        let pname = raw_name
            .trim_start_matches("...")
            .trim_start_matches('&')
            .trim_start_matches('$')
            .to_string();
        let ptype = Self::resolve_php_type(raw_type, namespace, use_map);

        (ptype, pname, default_val, is_variadic)
    }

    fn resolve_php_type(raw: &str, _namespace: &str, use_map: &HashMap<String, String>) -> String {
        let builtins = [
            "string", "int", "float", "bool", "array", "callable", "mixed",
            "iterable", "object", "null", "void", "self", "static", "never", "true", "false",
        ];
        let parts: Vec<String> = raw
            .split('|')
            .map(|part| {
                let p = part.trim();
                let nullable = p.starts_with('?');
                let no_q = p.trim_start_matches('?');
                let is_arr = no_q.ends_with("[]");
                let base = no_q.trim_end_matches("[]").trim_start_matches('\\');
                let resolved_base = if builtins.contains(&base.to_lowercase().as_str()) || base.contains('<') {
                    base.to_string()
                } else if let Some(fqcn) = use_map.get(base) {
                    fqcn.clone()
                } else {
                    base.to_string()
                };
                format!(
                    "{}{}{}",
                    if nullable { "?" } else { "" },
                    resolved_base,
                    if is_arr { "[]" } else { "" }
                )
            })
            .collect();
        parts.join("|")
    }

    fn extract_class_body<'a>(content: &'a str, class_name: &str) -> Option<&'a str> {
        let pattern = format!(r"(?:final\s+|readonly\s+|abstract\s+)*class\s+{}\b[^{{]*\{{", regex::escape(class_name));
        let re = Regex::new(&pattern).ok()?;
        let mat = re.find(content)?;
        let start_brace = mat.end() - 1;
        Self::extract_braced_slice(content, start_brace)
    }

    fn extract_braced_slice<'a>(content: &'a str, start_brace: usize) -> Option<&'a str> {
        let mut depth = 0usize;
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

    fn extract_preceding_docblock(before: &str) -> Option<&str> {
        let trimmed = before.trim_end();
        if !trimmed.ends_with("*/") {
            return None;
        }
        let start = trimmed.rfind("/**")?;
        Some(&trimmed[start..])
    }

    fn extract_docblock_param_types(
        doc: &str,
        namespace: &str,
        use_map: &HashMap<String, String>,
    ) -> HashMap<String, String> {
        let mut map = HashMap::new();
        if let Ok(re) = Regex::new(r"@(?:psalm-)?param\s+([^\s]+)\s+\$([A-Za-z0-9_]+)") {
            for cap in re.captures_iter(doc) {
                let raw_t = cap[1].trim();
                let name = cap[2].to_string();
                let resolved = Self::resolve_php_type(raw_t, namespace, use_map);
                map.insert(name, resolved);
            }
        }
        map
    }

    fn extract_docblock_return_type(
        doc: &str,
        namespace: &str,
        use_map: &HashMap<String, String>,
    ) -> Option<String> {
        let re = Regex::new(r"@(?:psalm-)?return\s+([^\s*]+)").ok()?;
        for cap in re.captures_iter(doc) {
            let raw_t = cap[1].trim();
            if !raw_t.is_empty() && raw_t != "mixed" {
                return Some(Self::resolve_php_type(raw_t, namespace, use_map));
            }
        }
        None
    }

    fn parse_method_params(
        raw_params: &str,
        docblock: Option<&str>,
        namespace: &str,
        use_map: &HashMap<String, String>,
        method_body: &str,
    ) -> Vec<ParameterDefinition> {
        let doc_params = docblock
            .map(|d| Self::extract_docblock_param_types(d, namespace, use_map))
            .unwrap_or_default();

        let mut params = Vec::new();
        for p in raw_params.split(',') {
            let p_trim = p.trim();
            if p_trim.is_empty() {
                continue;
            }
            let (mut ptype, pname, pdefault, is_variadic) = Self::parse_param_def(p_trim, namespace, use_map);
            if (ptype == "array" || ptype == "iterable" || ptype == "mixed") && doc_params.contains_key(&pname) {
                let doc_t = &doc_params[&pname];
                if doc_t.contains("[]") || doc_t.contains('<') || doc_t.contains('\\') {
                    ptype = doc_t.clone();
                }
            }

            let mut constraints = Vec::new();
            if is_variadic {
                constraints.push("variadic".to_string());
            }

            let scope = if method_body.is_empty() { "" } else { method_body };
            if ptype == "float" || ptype == "int" {
                constraints.push("finite".to_string());
                if scope.contains(&format!("${} < 0", pname)) || scope.contains(&format!("${} <= 0", pname)) {
                    constraints.push(">= 0.0".to_string());
                }
            } else if ptype == "string" {
                let has_non_empty_guard = scope.contains(&format!("empty(${})", pname))
                    || scope.contains(&format!("${} === ''", pname))
                    || scope.contains(&format!("${} === \"\"", pname))
                    || scope.contains(&format!("trim(${}) === ''", pname))
                    || scope.contains(&format!("strlen(${}) === 0", pname))
                    || scope.contains(&format!("non-empty-string ${}", pname));
                if has_non_empty_guard {
                    constraints.push("non-empty-string".to_string());
                }
                if scope.contains(&format!("strlen(${}) !== 2", pname)) || scope.contains(&format!("strlen(${}) != 2", pname)) {
                    constraints.push("length == 2".to_string());
                }
            }

            if !scope.is_empty() && (ptype == "string" || ptype == "mixed") {
                let escaped = regex::escape(&pname);
                let mut added = 0usize;

                // Direct equality checks: $param === 'lit'
                if let Ok(eq_re) = Regex::new(&format!(r"\${}\s*[!=]==?\s*['\x22]([A-Za-z0-9_./:-]{{1,32}})['\x22]", escaped)) {
                    for eq_cap in eq_re.captures_iter(scope) {
                        let c = format!("eq:{}", &eq_cap[1]);
                        if !constraints.contains(&c) {
                            constraints.push(c);
                            added += 1;
                            if added >= 12 {
                                break;
                            }
                        }
                    }
                }

                // PHP 8 match ($param) { 'lit1' => ..., 'lit2' => ... }
                if added < 12 {
                    if let Ok(match_re) = Regex::new(&format!(r"match\s*\(\s*\${}\s*\)\s*\{{", escaped)) {
                        if let Some(m_hit) = match_re.find(scope) {
                            if let Some(match_body) = Self::extract_braced_slice(scope, m_hit.end() - 1) {
                                if let Ok(arm_re) = Regex::new(r"['\x22]([A-Za-z0-9_./:-]{1,32})['\x22]\s*=>") {
                                    for arm_cap in arm_re.captures_iter(match_body) {
                                        let c = format!("eq:{}", &arm_cap[1]);
                                        if !constraints.contains(&c) {
                                            constraints.push(c);
                                            added += 1;
                                            if added >= 12 {
                                                break;
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                // switch ($param) { case 'lit': ... }
                if added < 12 && scope.contains(&format!("switch (${})", pname)) {
                    if let Ok(case_re) = Regex::new(r"case\s+['\x22]([A-Za-z0-9_./:-]{1,32})['\x22]\s*:") {
                        for case_cap in case_re.captures_iter(scope) {
                            let c = format!("eq:{}", &case_cap[1]);
                            if !constraints.contains(&c) {
                                constraints.push(c);
                                added += 1;
                                if added >= 12 {
                                    break;
                                }
                            }
                        }
                    }
                }
            }

            if !scope.is_empty() && (ptype == "array" || ptype.starts_with("array<") || ptype == "mixed") {
                let escaped = regex::escape(&pname);
                if let Ok(key_re) = Regex::new(&format!(r"\${}\[['\x22]([A-Za-z0-9_-]{{1,32}})['\x22]\]", escaped)) {
                    let mut prop_count = 0usize;
                    for k_cap in key_re.captures_iter(scope) {
                        let c = format!("prop:{}", &k_cap[1]);
                        if !constraints.contains(&c) {
                            constraints.push(c);
                            prop_count += 1;
                            if prop_count >= 8 {
                                break;
                            }
                        }
                    }
                }
            }

            params.push(ParameterDefinition {
                name: pname,
                param_type: ptype,
                constraints,
                default_value: pdefault,
            });
        }
        params
    }

    pub fn parse_php(content: &str) -> Result<Vec<PhpClassMetadata>, ExtractorError> {
        let mut classes = Vec::new();

        let ns_regex = Regex::new(r"namespace\s+([^;{\s]+);").unwrap();
        let namespace = ns_regex
            .captures(content)
            .map(|c| c[1].trim().to_string())
            .unwrap_or_default();

        let mut use_map: HashMap<String, String> = HashMap::new();
        let use_re = Regex::new(r"(?m)^\s*use\s+([A-Za-z0-9_\\]+)(?:\s+as\s+([A-Za-z0-9_]+))?\s*;").unwrap();
        for cap in use_re.captures_iter(content) {
            let fqcn = cap[1].trim_start_matches('\\').to_string();
            let alias = cap
                .get(2)
                .map(|m| m.as_str().to_string())
                .unwrap_or_else(|| fqcn.rsplit('\\').next().unwrap_or(&fqcn).to_string());
            use_map.insert(alias, fqcn);
        }

        let class_header_re = Regex::new(
            r"(?:final\s+|readonly\s+|abstract\s+)*class\s+([A-Za-z0-9_]+)(?:\s+extends\s+([A-Za-z0-9_\\]+))?(?:\s+implements\s+([A-Za-z0-9_\\,\s]+?))?\s*\{"
        ).unwrap();

        let mut class_headers = Vec::new();
        for cap in class_header_re.captures_iter(content) {
            let class_name = cap[1].to_string();
            let mut implements = Vec::new();
            if let Some(ext) = cap.get(2) {
                implements.push(Self::resolve_php_type(ext.as_str().trim(), &namespace, &use_map));
            }
            if let Some(imp) = cap.get(3) {
                for iface in imp.as_str().split(',') {
                    let trimmed = iface.trim();
                    if !trimmed.is_empty() {
                        implements.push(Self::resolve_php_type(trimmed, &namespace, &use_map));
                    }
                }
            }
            class_headers.push((class_name, implements));
        }

        if class_headers.is_empty() {
            return Ok(classes);
        }

        let requires_fallback = content.contains("@template") || content.contains("@psalm-") || content.contains("array{");

        for (class_name, implements) in class_headers {
            let class_content = Self::extract_class_body(content, &class_name).unwrap_or(content);

            let mut constructor_params = Vec::new();
            let mut private_constructor = false;
            let mut static_factories = Vec::new();
            let public_properties = Vec::new();
            let mut methods = Vec::new();

            // Constructor Property Promotion and Standard Constructor (public, protected, or private)
            let ctor_regex = Regex::new(r"(?m)(public|protected|private)?\s*function\s+__construct\s*\(([^)]*)\)").unwrap();
            if let Some(cap) = ctor_regex.captures(class_content) {
                let vis = cap.get(1).map_or("public", |m| m.as_str());
                private_constructor = vis == "private" || vis == "protected";
                let ctor_doc = Self::extract_preceding_docblock(&class_content[..cap.get(0).unwrap().start()]);
                let doc_params = ctor_doc
                    .map(|d| Self::extract_docblock_param_types(d, &namespace, &use_map))
                    .unwrap_or_default();

                let params_str = &cap[2];
                for param_def in params_str.split(',') {
                    let trimmed = param_def.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    let (mut param_type, param_name, default_value, is_variadic) =
                        Self::parse_param_def(trimmed, &namespace, &use_map);
                    if (param_type == "array" || param_type == "iterable" || param_type == "mixed")
                        && doc_params.contains_key(&param_name)
                    {
                        param_type = doc_params[&param_name].clone();
                    }
                    let mut constraints = Vec::new();
                    if is_variadic {
                        constraints.push("variadic".to_string());
                    }

                    constructor_params.push(ParameterDefinition {
                        name: param_name,
                        param_type,
                        constraints,
                        default_value,
                    });
                }
            }

            // Static factories returning self/static/ClassName
            let factory_regex = Regex::new(
                r"public\s+static\s+function\s+([A-Za-z0-9_]+)\s*\(([^)]*)\)\s*:\s*(self|static|[A-Za-z0-9_\\]+)"
            ).unwrap();
            for cap in factory_regex.captures_iter(class_content) {
                let fname = cap[1].to_string();
                let rtype = &cap[3];
                if rtype == "self" || rtype == "static" || rtype == class_name {
                    let fparams = Self::parse_method_params(&cap[2], None, &namespace, &use_map, "");
                    static_factories.push(FactorySignature {
                        name: fname,
                        parameters: fparams,
                    });
                }
            }

            // Methods
            let method_regex = Regex::new(
                r"(?m)(public|protected|private)?\s*(static\s+)?function\s+([A-Za-z0-9_]+)\s*\(([^)]*)\)(?:\s*:\s*([?A-Za-z0-9_\\|&]+))?\s*(\{|;)"
            ).unwrap();
            for cap in method_regex.captures_iter(class_content) {
                let visibility = cap.get(1).map_or("public", |m| m.as_str());
                let is_static = cap.get(2).is_some();
                let method_name = cap[3].to_string();
                let raw_params = &cap[4];
                let native_rt = cap.get(5).map(|m| Self::resolve_php_type(m.as_str().trim(), &namespace, &use_map));
                let terminator = &cap[6];

                let m_start = cap.get(0).unwrap().start();
                let docblock = Self::extract_preceding_docblock(&class_content[..m_start]);

                let method_body = if terminator == "{" {
                    let brace_pos = cap.get(6).unwrap().start();
                    Self::extract_braced_slice(class_content, brace_pos).unwrap_or("")
                } else {
                    ""
                };

                let params = Self::parse_method_params(raw_params, docblock, &namespace, &use_map, method_body);

                let return_type = match (native_rt.as_deref(), docblock.and_then(|d| Self::extract_docblock_return_type(d, &namespace, &use_map))) {
                    (Some("array") | Some("iterable") | None, Some(doc_rt)) => Some(doc_rt),
                    (Some(n), _) => Some(n.to_string()),
                    (None, None) => None,
                };

                // Detect side effects scoped to this method body
                let mut side_effects = Vec::new();
                for ctor_p in &constructor_params {
                    let target_method_regex = Regex::new(&format!(r"\$this->{}->([A-Za-z0-9_]+)\(", regex::escape(&ctor_p.name))).unwrap();
                    for call_cap in target_method_regex.captures_iter(method_body) {
                        side_effects.push(DetectedSideEffect {
                            effect_type: "interface_call".to_string(),
                            target: format!("{}.{}", ctor_p.name, &call_cap[1]),
                            mockable: true,
                        });
                    }
                }

                // Clock reads
                if method_body.contains("new \\DateTimeImmutable()") || method_body.contains("new DateTimeImmutable()") {
                    side_effects.push(DetectedSideEffect {
                        effect_type: "clock_read".to_string(),
                        target: "clock.now".to_string(),
                        mockable: true,
                    });
                }

                // Unmockable side effects
                if method_body.contains("$_ENV") || method_body.contains("$_SERVER") || method_body.contains("exec(") || method_body.contains("system(") {
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
                implements,
                private_constructor,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_php_variadic_private_ctor_match_and_phpdoc() {
        let code = r#"<?php
namespace Yiisoft\Router;

use Psr\Http\Message\ResponseInterface;

final class MatchingResult
{
    private function __construct(private readonly ?Route $route) {}

    public static function fromSuccess(Route $route, array $arguments): self
    {
        return new self($route);
    }

    public static function fromFailure(array $methods): self
    {
        return new self(null);
    }

    public function getData(string $key): mixed
    {
        return match ($key) {
            'name' => 'x',
            'pattern' => '/a',
            'hosts' => [],
            default => null,
        };
    }

    /**
     * @param Route|Group ...$routes
     */
    public function addRoute(Route|Group ...$routes): self
    {
        return $this;
    }
}
"#;
        let doc = PhpExtractor::extract_source("src/MatchingResult.php", code).unwrap();
        let get_data = doc.targets.iter().find(|t| t.id == "Yiisoft\\Router\\MatchingResult::getData").unwrap();
        assert!(get_data.signature.private_constructor);
        assert_eq!(get_data.signature.static_factories.len(), 2);
        assert!(get_data.signature.parameters[0].constraints.contains(&"eq:name".to_string()));
        assert!(get_data.signature.parameters[0].constraints.contains(&"eq:pattern".to_string()));

        let add_route = doc.targets.iter().find(|t| t.id == "Yiisoft\\Router\\MatchingResult::addRoute").unwrap();
        assert_eq!(add_route.signature.parameters[0].name, "routes");
        assert!(add_route.signature.parameters[0].constraints.contains(&"variadic".to_string()));
    }
}
