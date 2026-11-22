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

        // Standalone (namespaced) functions
        for (fn_id, method) in Self::parse_php_functions(content) {
            targets.push(TargetDefinition {
                id: fn_id,
                kind: TargetKind::Function,
                signature: SignatureDefinition {
                    constructor_args: Vec::new(),
                    parameters: method.parameters.clone(),
                    return_type: method.return_type.clone(),
                    static_factories: Vec::new(),
                    implements: Vec::new(),
                    private_constructor: false,
                },
                detected_side_effects: method.detected_side_effects.clone(),
                metadata_keys: Vec::new(),
                scenarios: Vec::new(),
            });
        }

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

    fn extract_balanced_parens_slice<'a>(content: &'a str, open_paren: usize) -> Option<(&'a str, usize)> {
        let mut depth = 0usize;
        for (idx, b) in content[open_paren..].bytes().enumerate() {
            if b == b'(' {
                depth += 1;
            } else if b == b')' {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    let close_pos = open_paren + idx;
                    return Some((&content[open_paren + 1..close_pos], close_pos + 1));
                }
            }
        }
        None
    }

    fn split_top_level_commas(s: &str) -> Vec<&str> {
        let mut parts = Vec::new();
        let mut depth_paren = 0usize;
        let mut depth_brace = 0usize;
        let mut depth_bracket = 0usize;
        let mut depth_angle = 0usize;
        let mut in_single = false;
        let mut in_double = false;
        let mut prev_bs = false;
        let mut start = 0usize;
        let bytes = s.as_bytes();
        for (i, &b) in bytes.iter().enumerate() {
            if prev_bs {
                prev_bs = false;
                continue;
            }
            if (in_single || in_double) && b == b'\\' {
                prev_bs = true;
                continue;
            }
            if !in_double && b == b'\'' {
                in_single = !in_single;
            } else if !in_single && b == b'"' {
                in_double = !in_double;
            } else if !in_single && !in_double {
                match b {
                    b'(' => depth_paren += 1,
                    b')' => depth_paren = depth_paren.saturating_sub(1),
                    b'{' => depth_brace += 1,
                    b'}' => depth_brace = depth_brace.saturating_sub(1),
                    b'[' => depth_bracket += 1,
                    b']' => depth_bracket = depth_bracket.saturating_sub(1),
                    b'<' => depth_angle += 1,
                    b'>' => depth_angle = depth_angle.saturating_sub(1),
                    b',' if depth_paren == 0 && depth_brace == 0 && depth_bracket == 0 && depth_angle == 0 => {
                        parts.push(&s[start..i]);
                        start = i + 1;
                    }
                    _ => {}
                }
            }
        }
        if start <= s.len() {
            parts.push(&s[start..]);
        }
        parts
    }

    fn harvest_php_regex_consts(content: &str) -> HashMap<String, String> {
        let mut map = HashMap::new();
        let re = Regex::new(
            r#"(?s)const\s+([A-Z0-9_]+)\s*=\s*(?:'([/#~][^']+)'|"([/#~][^"]+)")\s*;"#
        ).unwrap();
        for cap in re.captures_iter(content) {
            let name = cap[1].to_string();
            let pat = cap.get(2).or_else(|| cap.get(3)).map(|m| m.as_str()).unwrap_or("");
            if let Some(sample) = autotest_core::sample_from_regex(pat) {
                map.insert(name, sample);
            }
        }
        map
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
        regex_consts: &HashMap<String, String>,
        owner_class: &str,
    ) -> Vec<ParameterDefinition> {
        let doc_params = docblock
            .map(|d| Self::extract_docblock_param_types(d, namespace, use_map))
            .unwrap_or_default();

        let mut params = Vec::new();
        for p in Self::split_top_level_commas(raw_params) {
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
            let escaped = regex::escape(&pname);

            if ptype == "float" || ptype == "int" || ptype.contains("int") || ptype.contains("float") {
                constraints.push("finite".to_string());
                if scope.contains(&format!("${} < 0", pname))
                    || scope.contains(&format!("${} <= 0", pname))
                    || scope.contains(&format!("$this->{} < 0", pname))
                    || scope.contains(&format!("$this->{} <= 0", pname))
                {
                    constraints.push(">= 0.0".to_string());
                }
                if !scope.is_empty() {
                    if let Ok(num_re) = Regex::new(&format!(
                        r"\$(?:this->)?{}\b\s*(?:[!=]==?|[<>]=?)\s*(-?\d+(?:\.\d+)?)",
                        escaped
                    )) {
                        for n_cap in num_re.captures_iter(scope).take(4) {
                            let c = format!("branch_num:{}", &n_cap[1]);
                            if !constraints.contains(&c) {
                                constraints.push(c);
                            }
                        }
                    }
                }
            }

            if ptype.contains("string") || ptype == "mixed" {
                let has_non_empty_guard = scope.contains(&format!("empty(${})", pname))
                    || scope.contains(&format!("${} === ''", pname))
                    || scope.contains(&format!("${} === \"\"", pname))
                    || scope.contains(&format!("'' !== ${}", pname))
                    || scope.contains(&format!("trim(${}) === ''", pname))
                    || scope.contains(&format!("strlen(${}) === 0", pname))
                    || scope.contains(&format!("non-empty-string ${}", pname));
                if has_non_empty_guard {
                    constraints.push("non-empty-string".to_string());
                }

                // 1. Generalized string length comparisons: strlen($param) / strlen($this->param)
                if !scope.is_empty() {
                    if let Ok(len_re) = Regex::new(&format!(
                        r"(?:mb_)?strlen\s*\(\s*\$(?:this->)?{}\s*\)\s*([!=]==?|[<>]=?)\s*(\d+)",
                        escaped
                    )) {
                        for l_cap in len_re.captures_iter(scope) {
                            let op = &l_cap[1];
                            if let Ok(n) = l_cap[2].parse::<usize>() {
                                let target_len = if op == ">" || op == "<=" { n + 1 } else { n };
                                if target_len > 0 && target_len <= 1024 {
                                    let c = format!("length == {}", target_len);
                                    if !constraints.contains(&c) {
                                        constraints.push(c);
                                    }
                                }
                            }
                        }
                    }
                }
            }

            if !scope.is_empty() && (ptype.contains("string") || ptype == "mixed") {
                let mut added = 0usize;

                // 2. Regex constant & literal matching: preg_match(self::CONST, $param) or preg_match('/.../', $param)
                if let Ok(pm_const_re) = Regex::new(&format!(
                    r"preg_match(?:_all)?\s*\(\s*(?:self::|static::|[A-Za-z0-9_\\]+::)?([A-Z0-9_]+)\s*,\s*(?:\(string\)\s*)?\$(?:this->)?{}\b",
                    escaped
                )) {
                    for pm_cap in pm_const_re.captures_iter(scope) {
                        if let Some(sample) = regex_consts.get(&pm_cap[1]) {
                            let v = format!("valid:{}", sample);
                            let eq = format!("eq:{}", sample);
                            if !constraints.contains(&v) {
                                constraints.push(v);
                            }
                            if !constraints.contains(&eq) {
                                constraints.push(eq);
                                added += 1;
                            }
                        }
                    }
                }
                if let Ok(pm_lit_re) = Regex::new(&format!(
                    r#"preg_match(?:_all)?\s*\(\s*['"]([/#~][^'"]+)['"]\s*,\s*(?:\(string\)\s*)?\$(?:this->)?{}\b"#,
                    escaped
                )) {
                    for pm_cap in pm_lit_re.captures_iter(scope) {
                        if let Some(sample) = autotest_core::sample_from_regex(&pm_cap[1]) {
                            let v = format!("valid:{}", sample);
                            let eq = format!("eq:{}", sample);
                            if !constraints.contains(&v) {
                                constraints.push(v);
                            }
                            if !constraints.contains(&eq) {
                                constraints.push(eq);
                                added += 1;
                            }
                        }
                    }
                }

                // 3. Static lookup-table key accesses: $this->table[$param] or self::$table[$param]
                if let Ok(tbl_re) = Regex::new(&format!(
                    r"(?:\$this->|(?:self|static)::\$?)([A-Za-z0-9_]+)\[\s*\$(?:this->)?{}\s*\]",
                    escaped
                )) {
                    for t_cap in tbl_re.captures_iter(scope) {
                        let c = format!("lookup_key:{}", &t_cap[1]);
                        if !constraints.contains(&c) {
                            constraints.push(c);
                        }
                    }
                }
                if let Ok(ak_re) = Regex::new(&format!(
                    r"array_key_exists\s*\(\s*\$(?:this->)?{}\s*,\s*(?:\$this->|(?:self|static)::\$?)([A-Za-z0-9_]+)\s*\)",
                    escaped
                )) {
                    for t_cap in ak_re.captures_iter(scope) {
                        let c = format!("lookup_key:{}", &t_cap[1]);
                        if !constraints.contains(&c) {
                            constraints.push(c);
                        }
                    }
                }

                // Direct equality checks: $param === 'lit'
                if let Ok(eq_re) = Regex::new(&format!(r"\$(?:this->)?{}\s*[!=]==?\s*['\x22]([A-Za-z0-9_./:-]{{1,32}})['\x22]", escaped)) {
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

            // 4. Inter-procedural call forwarding: `->callee(..., $param, ...)` / `Cls::callee(..., $param, ...)` / `new self(..., $param, ...)`
            if !scope.is_empty() {
                if let Ok(call_re) = Regex::new(r"(?:([A-Za-z0-9_\\]+)::|(->)|(new\s+))([A-Za-z0-9_]+)\s*\(([^()]{1,240})\)") {
                    for c_cap in call_re.captures_iter(scope) {
                        let static_cls = c_cap.get(1).map(|m| m.as_str());
                        let is_new = c_cap.get(3).is_some();
                        let raw_callee = &c_cap[4];
                        if matches!(
                            raw_callee,
                            "strlen" | "mb_strlen" | "preg_match" | "preg_match_all" | "isset" | "empty" | "trim" | "strtolower" | "strtoupper" | "is_string" | "is_int" | "is_array"
                        ) {
                            continue;
                        }
                        let args_slice = &c_cap[5];
                        for (arg_idx, raw_arg) in Self::split_top_level_commas(args_slice).iter().enumerate() {
                            let cleaned_arg = raw_arg
                                .trim()
                                .trim_start_matches("(string)")
                                .trim_start_matches("(int)")
                                .trim_start_matches("(float)")
                                .trim();
                            if cleaned_arg == format!("${}", pname) || cleaned_arg == format!("$this->{}", pname) {
                                let callee_key = if is_new {
                                    if (raw_callee == "self" || raw_callee == "static") && !owner_class.is_empty() {
                                        format!("{}::__construct", owner_class)
                                    } else {
                                        let short_c = raw_callee.rsplit('\\').next().unwrap_or(raw_callee);
                                        format!("{}::__construct", short_c)
                                    }
                                } else if let Some(cls_q) = static_cls {
                                    if (cls_q == "self" || cls_q == "static" || cls_q == "parent") && !owner_class.is_empty() {
                                        format!("{}::{}", owner_class, raw_callee)
                                    } else {
                                        let short_c = cls_q.rsplit('\\').next().unwrap_or(cls_q);
                                        format!("{}::{}", short_c, raw_callee)
                                    }
                                } else {
                                    raw_callee.to_string()
                                };
                                let fwd = format!("forward:{}:{}", callee_key, arg_idx);
                                if !constraints.contains(&fwd) {
                                    constraints.push(fwd);
                                }
                            }
                        }
                    }
                }
            }

            if !scope.is_empty() && (ptype == "array" || ptype.starts_with("array<") || ptype == "mixed") {
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

    /// Extracts top-level (non-class) functions; returns `(Namespace\name, metadata)` pairs.
    pub fn parse_php_functions(content: &str) -> Vec<(String, PhpMethodMetadata)> {
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

        let mut ranges: Vec<(usize, usize)> = Vec::new();
        let decl_re = Regex::new(
            r"(?m)^[ \t]*(?:final\s+|readonly\s+|abstract\s+)*(?:class|interface|trait|enum)\s+[A-Za-z0-9_]+[^{;]*\{"
        ).unwrap();
        for m in decl_re.find_iter(content) {
            let start = m.end() - 1;
            if let Some(body) = Self::extract_braced_slice(content, start) {
                ranges.push((start, start + body.len() + 2));
            }
        }

        let regex_consts = Self::harvest_php_regex_consts(content);
        let fn_re = Regex::new(
            r"(?m)^[ \t]*function\s+&?([A-Za-z0-9_]+)\s*\(([^)]*)\)(?:\s*:\s*([?A-Za-z0-9_\\|&]+))?\s*\{"
        ).unwrap();
        let mut out = Vec::new();
        for cap in fn_re.captures_iter(content) {
            let whole = cap.get(0).unwrap();
            if ranges.iter().any(|(a, b)| whole.start() > *a && whole.start() < *b) {
                continue;
            }
            let name = cap[1].to_string();
            let body = Self::extract_braced_slice(content, whole.end() - 1).unwrap_or("");
            let docblock = Self::extract_preceding_docblock(&content[..whole.start()]);
            let params = Self::parse_method_params(&cap[2], docblock, &namespace, &use_map, body, &regex_consts, "");
            let native_rt = cap.get(3).map(|m| Self::resolve_php_type(m.as_str().trim(), &namespace, &use_map));
            let return_type = match (
                native_rt.as_deref(),
                docblock.and_then(|d| Self::extract_docblock_return_type(d, &namespace, &use_map)),
            ) {
                (Some("array") | Some("iterable") | None, Some(doc_rt)) => Some(doc_rt),
                (Some(n), _) => Some(n.to_string()),
                (None, None) => None,
            };

            let mut side_effects = Vec::new();
            let lname = name.to_lowercase();
            if body.contains("random_bytes(")
                || body.contains("random_int(")
                || body.contains("mt_rand(")
                || body.contains("uniqid(")
                || lname.contains("random")
                || lname.contains("uuid1")
                || lname.contains("uuid4")
                || lname.contains("uuid6")
                || lname.contains("uuid7")
            {
                side_effects.push(DetectedSideEffect {
                    effect_type: "rng".to_string(),
                    target: "random".to_string(),
                    mockable: true,
                });
            }
            if body.contains("$_ENV") || body.contains("$_SERVER") || body.contains("exec(") || body.contains("system(") {
                side_effects.push(DetectedSideEffect {
                    effect_type: "unmockable_side_effect".to_string(),
                    target: "global.state".to_string(),
                    mockable: false,
                });
            }

            let id = if namespace.is_empty() { name.clone() } else { format!("{}\\{}", namespace, name) };
            out.push((
                id,
                PhpMethodMetadata {
                    name,
                    is_static: false,
                    is_public: true,
                    parameters: params,
                    return_type,
                    detected_side_effects: side_effects,
                    throws: Vec::new(),
                },
            ));
        }
        out
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
            let regex_consts = Self::harvest_php_regex_consts(class_content);

            let mut constructor_params = Vec::new();
            let mut private_constructor = false;
            let mut static_factories = Vec::new();
            let public_properties = Vec::new();
            let mut methods = Vec::new();

            // Constructor Property Promotion and Standard Constructor (public, protected, or private, with balanced parens)
            let ctor_regex = Regex::new(r"(?m)(public|protected|private)?\s*function\s+__construct\s*\(").unwrap();
            if let Some(cap) = ctor_regex.captures(class_content) {
                let full = cap.get(0).unwrap();
                let vis = cap.get(1).map_or("public", |m| m.as_str());
                private_constructor = vis == "private" || vis == "protected";
                let ctor_doc = Self::extract_preceding_docblock(&class_content[..full.start()]);
                if let Some((params_str, after_close)) = Self::extract_balanced_parens_slice(class_content, full.end() - 1) {
                    let ctor_body = class_content[after_close..]
                        .find('{')
                        .and_then(|rel_b| Self::extract_braced_slice(class_content, after_close + rel_b))
                        .unwrap_or("");
                    constructor_params = Self::parse_method_params(
                        params_str,
                        ctor_doc,
                        &namespace,
                        &use_map,
                        ctor_body,
                        &regex_consts,
                        &class_name,
                    );
                }
            }

            // Static factories & Methods (with balanced parens so `= new DefaultContext()` in parameter defaults works)
            let method_header_re = Regex::new(
                r"(?m)(public|protected|private)?\s*(static\s+)?function\s+([A-Za-z0-9_]+)\s*\("
            ).unwrap();
            let ret_term_re = Regex::new(r"^\s*(?::\s*([?A-Za-z0-9_\\|&]+))?\s*(\{|;)").unwrap();

            for cap in method_header_re.captures_iter(class_content) {
                let full = cap.get(0).unwrap();
                let visibility = cap.get(1).map_or("public", |m| m.as_str());
                let is_static = cap.get(2).is_some();
                let method_name = cap[3].to_string();

                let Some((raw_params, after_close)) = Self::extract_balanced_parens_slice(class_content, full.end() - 1) else {
                    continue;
                };
                let Some(rt_cap) = ret_term_re.captures(&class_content[after_close..]) else {
                    continue;
                };
                let raw_rt_str = rt_cap.get(1).map(|m| m.as_str().trim());
                let native_rt = raw_rt_str.map(|s| Self::resolve_php_type(s, &namespace, &use_map));
                let terminator = &rt_cap[2];

                let m_start = full.start();
                let docblock = Self::extract_preceding_docblock(&class_content[..m_start]);

                let method_body = if terminator == "{" {
                    let brace_pos = after_close + rt_cap.get(2).unwrap().start();
                    Self::extract_braced_slice(class_content, brace_pos).unwrap_or("")
                } else {
                    ""
                };

                let params = Self::parse_method_params(
                    raw_params,
                    docblock,
                    &namespace,
                    &use_map,
                    method_body,
                    &regex_consts,
                    &class_name,
                );

                // Register public static factory if return type is self/static/ClassName
                if visibility == "public" && is_static {
                    if let Some(rt_raw) = raw_rt_str {
                        if rt_raw == "self" || rt_raw == "static" || rt_raw == class_name {
                            static_factories.push(FactorySignature {
                                name: method_name.clone(),
                                parameters: params.clone(),
                            });
                        }
                    }
                }

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

                // Non-deterministic sources (RNG / unique ids)
                let lname = method_name.to_lowercase();
                if method_body.contains("random_bytes(")
                    || method_body.contains("random_int(")
                    || method_body.contains("mt_rand(")
                    || method_body.contains("uniqid(")
                    || method_body.contains("str_shuffle(")
                    || method_body.contains("shuffle(")
                    || lname.contains("random")
                    || lname.contains("uuid1")
                    || lname.contains("uuid4")
                    || lname.contains("uuid6")
                    || lname.contains("uuid7")
                {
                    side_effects.push(DetectedSideEffect {
                        effect_type: "rng".to_string(),
                        target: "random".to_string(),
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

    #[test]
    fn test_php_standalone_functions_and_rng_side_effects() {
        let code = r#"<?php
namespace Ramsey\Uuid;

function uuid4(): string
{
    return bin2hex(random_bytes(16));
}

final class ByteString
{
    public static function fromRandom(int $length = 16): self
    {
        return new self();
    }
}
"#;
        let doc = PhpExtractor::extract_source("src/functions.php", code).unwrap();
        let fn_target = doc.targets.iter().find(|t| t.id == "Ramsey\\Uuid\\uuid4").unwrap();
        assert_eq!(fn_target.kind, autotest_core::ir::TargetKind::Function);
        assert!(fn_target.detected_side_effects.iter().any(|e| e.effect_type == "rng"));

        let from_random = doc.targets.iter().find(|t| t.id == "Ramsey\\Uuid\\ByteString::fromRandom").unwrap();
        assert!(from_random.detected_side_effects.iter().any(|e| e.effect_type == "rng"));
    }

    #[test]
    fn test_php_static_constraint_mechanisms_length_regex_lookup_and_forwarding() {
        let code = r#"<?php
namespace Demo;

final class Urn
{
    private const REGEXP_URN_PARTS = '/^urn:(?<nid>[a-z0-9](?:[a-z0-9-]{0,30}[a-z0-9])?):(?<nss>.*?)$/xi';

    private function __construct(private string $bytes)
    {
        if (strlen($this->bytes) !== 16) {
            throw new \InvalidArgumentException('must be 16 bytes');
        }
    }

    public static function new(string $urn): self
    {
        return self::fromString($urn);
    }

    public static function fromString(string $urn): self
    {
        1 === preg_match(self::REGEXP_URN_PARTS, $urn, $matches) || throw new \RuntimeException('bad');
        return new self('0000000000000000');
    }

    public static function of(int|string $amount, string $currency, Context $ctx = new DefaultContext()): self
    {
        if (!isset($this->currencyData[$currency])) {
            throw new \RuntimeException('unknown');
        }
        return new self('0000000000000000');
    }
}
"#;
        let doc = PhpExtractor::extract_source("src/Urn.php", code).unwrap();
        let from_str = doc.targets.iter().find(|t| t.id == "Demo\\Urn::fromString").unwrap();
        assert!(from_str.signature.parameters[0].constraints.contains(&"valid:urn:aa:a".to_string()));
        assert!(from_str.signature.parameters[0].constraints.contains(&"eq:urn:aa:a".to_string()));
        assert!(from_str.signature.constructor_args[0].constraints.contains(&"length == 16".to_string()));

        let new_fn = doc.targets.iter().find(|t| t.id == "Demo\\Urn::new").unwrap();
        assert!(new_fn.signature.parameters[0].constraints.contains(&"forward:Urn::fromString:0".to_string()));

        // Verify balanced-paren default initializer `= new DefaultContext()` in `of(...)` was parsed as both target and static factory
        let of_fn = doc.targets.iter().find(|t| t.id == "Demo\\Urn::of").unwrap();
        assert_eq!(of_fn.signature.parameters.len(), 3);
        assert!(of_fn.signature.parameters[1].constraints.contains(&"lookup_key:currencyData".to_string()));
        assert!(of_fn.signature.static_factories.iter().any(|f| f.name == "of"));
    }
}
