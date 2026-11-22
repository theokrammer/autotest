use std::collections::{BTreeMap, HashMap};
use regex::Regex;
use crate::ir::{FactorySignature, ParameterDefinition, ScenarioDocument, TargetKind};

#[derive(Debug, Clone, Default)]
pub struct ClassIndexEntry {
    pub name: String,
    pub short_name: String,
    pub source_file: String,
    pub implements: Vec<String>,
    pub private_constructor: bool,
    pub constructor_params: Vec<ParameterDefinition>,
    pub static_factories: Vec<FactorySignature>,
    pub mutators: Vec<(String, Vec<ParameterDefinition>)>,
    pub method_return_types: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Default)]
pub struct ProjectIndex {
    pub classes: BTreeMap<String, ClassIndexEntry>,
    pub short_name_map: BTreeMap<String, String>,
    pub interface_impls: BTreeMap<String, Vec<String>>,
    pub interface_methods: BTreeMap<String, BTreeMap<String, String>>,
    pub string_constants: BTreeMap<String, String>,
    pub lookup_table_keys: BTreeMap<String, Vec<String>>,
    pub method_param_constraints: BTreeMap<String, Vec<Vec<String>>>,
}

impl ProjectIndex {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn index_document(&mut self, doc: &ScenarioDocument) {
        let source_file = doc.source.file.clone();
        for target in &doc.targets {
            // Index parameter constraints for inter-procedural forwarding resolution
            let param_c: Vec<Vec<String>> = target
                .signature
                .parameters
                .iter()
                .map(|p| p.constraints.clone())
                .collect();
            if !param_c.is_empty() {
                self.record_method_constraints(&target.id, &param_c);
                if let Some((cls, m)) = split_class_method(&target.id) {
                    let short_cls = short_name_of(cls);
                    self.record_method_constraints(&format!("{}::{}", short_cls, m), &param_c);
                    self.record_method_constraints(&format!("{}.{}", short_cls, m), &param_c);
                    self.record_method_constraints(m, &param_c);
                } else {
                    let short_fn = short_name_of(&target.id);
                    self.record_method_constraints(short_fn, &param_c);
                }
            }
            if !target.signature.constructor_args.is_empty() {
                let ctor_c: Vec<Vec<String>> = target
                    .signature
                    .constructor_args
                    .iter()
                    .map(|p| p.constraints.clone())
                    .collect();
                if let Some((cls, _)) = split_class_method(&target.id) {
                    let short_cls = short_name_of(cls);
                    self.record_method_constraints(&format!("{}::__construct", cls), &ctor_c);
                    self.record_method_constraints(&format!("{}::__construct", short_cls), &ctor_c);
                }
            }
            for f in &target.signature.static_factories {
                let fc: Vec<Vec<String>> = f.parameters.iter().map(|p| p.constraints.clone()).collect();
                if !fc.is_empty() {
                    if let Some((cls, _)) = split_class_method(&target.id) {
                        let short_cls = short_name_of(cls);
                        self.record_method_constraints(&format!("{}::{}", cls, f.name), &fc);
                        self.record_method_constraints(&format!("{}::{}", short_cls, f.name), &fc);
                        self.record_method_constraints(&f.name, &fc);
                    }
                }
            }

            if target.kind != TargetKind::Method && target.signature.constructor_args.is_empty() && target.signature.static_factories.is_empty() {
                continue;
            }
            let Some((class_name, method_name)) = split_class_method(&target.id) else {
                continue;
            };
            let short = short_name_of(class_name).to_string();
            let entry = self.classes.entry(class_name.to_string()).or_insert_with(|| ClassIndexEntry {
                name: class_name.to_string(),
                short_name: short.clone(),
                source_file: source_file.clone(),
                implements: target.signature.implements.clone(),
                private_constructor: target.signature.private_constructor,
                constructor_params: target.signature.constructor_args.clone(),
                static_factories: target.signature.static_factories.clone(),
                mutators: Vec::new(),
                method_return_types: BTreeMap::new(),
            });

            if entry.source_file.is_empty() {
                entry.source_file = source_file.clone();
            }
            if entry.constructor_params.is_empty() && !target.signature.constructor_args.is_empty() {
                entry.constructor_params = target.signature.constructor_args.clone();
            }
            if entry.static_factories.is_empty() && !target.signature.static_factories.is_empty() {
                entry.static_factories = target.signature.static_factories.clone();
            }
            if entry.implements.is_empty() && !target.signature.implements.is_empty() {
                entry.implements = target.signature.implements.clone();
            }
            if target.signature.private_constructor {
                entry.private_constructor = true;
            }

            if let Some(ref rt) = target.signature.return_type {
                entry.method_return_types.insert(method_name.to_string(), rt.clone());
            }

            if is_mutator_method_name(method_name)
                && !entry.mutators.iter().any(|(m, _)| m == method_name)
            {
                entry.mutators.push((method_name.to_string(), target.signature.parameters.clone()));
            }

            self.short_name_map.entry(short).or_insert_with(|| class_name.to_string());

            for iface in &target.signature.implements {
                let list = self.interface_impls.entry(iface.clone()).or_default();
                if !list.contains(&class_name.to_string()) {
                    list.push(class_name.to_string());
                }
                let short_iface = short_name_of(iface).to_string();
                let short_list = self.interface_impls.entry(short_iface).or_default();
                if !short_list.contains(&class_name.to_string()) {
                    short_list.push(class_name.to_string());
                }
            }
        }
    }

    pub fn index_source_metadata(&mut self, source_file: &str, content: &str, is_php: bool) {
        self.index_source_metadata_with_dir(None, source_file, content, is_php);
    }

    pub fn index_source_metadata_with_dir(
        &mut self,
        project_dir: Option<&std::path::Path>,
        source_file: &str,
        content: &str,
        is_php: bool,
    ) {
        if is_php {
            self.index_php_interfaces(content);
            self.index_php_lookup_tables(project_dir, source_file, content);
        } else {
            self.index_ts_constants_and_interfaces(content);
            self.index_ts_lookup_tables(content);
        }
    }

    fn record_method_constraints(&mut self, key: &str, params: &[Vec<String>]) {
        let entry = self
            .method_param_constraints
            .entry(key.to_string())
            .or_insert_with(|| vec![Vec::new(); params.len()]);
        if entry.len() < params.len() {
            entry.resize(params.len(), Vec::new());
        }
        for (idx, c_list) in params.iter().enumerate() {
            for c in c_list {
                if !entry[idx].contains(c) {
                    entry[idx].push(c.clone());
                }
            }
        }
    }

    pub fn resolve_dynamic_constraints(&self, constraints: &[String], depth: usize) -> Vec<String> {
        let mut out = Vec::new();
        for c in constraints {
            if !c.starts_with("lookup_key:") && !c.starts_with("forward:") && !out.contains(c) {
                out.push(c.clone());
            }
        }
        if depth > 3 {
            return out;
        }
        for c in constraints {
            if let Some(table) = c.strip_prefix("lookup_key:") {
                if let Some(keys) = self.lookup_table_keys.get(table) {
                    if let Some(first) = keys.first() {
                        let v = format!("valid:{}", first);
                        if !out.contains(&v) {
                            out.push(v);
                        }
                    }
                    for k in keys.iter().take(3) {
                        let eq = format!("eq:{}", k);
                        if !out.contains(&eq) {
                            out.push(eq);
                        }
                    }
                }
            } else if let Some(rest) = c.strip_prefix("forward:") {
                if let Some((callee, idx_str)) = rest.rsplit_once(':') {
                    if let Ok(arg_idx) = idx_str.parse::<usize>() {
                        if let Some(callee_params) = self.method_param_constraints.get(callee) {
                            if let Some(target_c) = callee_params.get(arg_idx) {
                                let resolved = self.resolve_dynamic_constraints(target_c, depth + 1);
                                for rc in resolved {
                                    if !rc.starts_with("forward:")
                                        && !rc.starts_with("lookup_key:")
                                        && rc != "variadic"
                                        && !out.contains(&rc)
                                    {
                                        out.push(rc);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        out
    }

    fn index_php_lookup_tables(
        &mut self,
        project_dir: Option<&std::path::Path>,
        source_file: &str,
        content: &str,
    ) {
        let key_re = Regex::new(r#"['"]([A-Za-z0-9_./:-]{1,32})['"]\s*=>"#).unwrap();

        // 1. Inline array constants / properties: const FOO = ['KEY' => ...] or $foo = ['KEY' => ...]
        let arr_decl_re = Regex::new(
            r"(?m)(?:const\s+([A-Za-z0-9_]+)|\$([A-Za-z0-9_]+))\s*=\s*\[([^\]]{4,4000})\]"
        ).unwrap();
        for cap in arr_decl_re.captures_iter(content) {
            let name = cap
                .get(1)
                .or_else(|| cap.get(2))
                .map(|m| m.as_str().to_string())
                .unwrap_or_default();
            let body = &cap[3];
            let mut keys = Vec::new();
            for k_cap in key_re.captures_iter(body) {
                let k = k_cap[1].to_string();
                if !keys.contains(&k) {
                    keys.push(k);
                    if keys.len() >= 4 {
                        break;
                    }
                }
            }
            if !name.is_empty() && !keys.is_empty() {
                self.lookup_table_keys.insert(name, keys);
            }
        }

        // 2. External required data arrays: `$var = require __DIR__ . '/path.php';`
        if let Some(root_dir) = project_dir {
            let req_re = Regex::new(
                r#"\$([A-Za-z0-9_]+)\s*=\s*require(?:_once)?\s+__DIR__\s*\.\s*['"]([^'"]+\.php)['"]"#
            ).unwrap();
            for cap in req_re.captures_iter(content) {
                let var_name = cap[1].to_string();
                let rel_req = cap[2].trim_start_matches('/');
                let base_dir = root_dir
                    .join(source_file)
                    .parent()
                    .map(|p| p.to_path_buf())
                    .unwrap_or_else(|| root_dir.to_path_buf());
                let req_path = base_dir.join(rel_req);
                if let Ok(req_content) = std::fs::read_to_string(&req_path) {
                    let mut keys = Vec::new();
                    for k_cap in key_re.captures_iter(&req_content) {
                        let k = k_cap[1].to_string();
                        if !keys.contains(&k) {
                            keys.push(k);
                            if keys.len() >= 4 {
                                break;
                            }
                        }
                    }
                    if !keys.is_empty() {
                        self.lookup_table_keys.insert(var_name.clone(), keys.clone());
                        // Also check if `$this->prop = $varName;` assigns it to a property
                        let assign_re = Regex::new(&format!(
                            r"\$this->([A-Za-z0-9_]+)\s*=\s*\${}\b",
                            regex::escape(&var_name)
                        ))
                        .unwrap();
                        for a_cap in assign_re.captures_iter(content) {
                            self.lookup_table_keys
                                .insert(a_cap[1].to_string(), keys.clone());
                        }
                    }
                }
            }
        }
    }

    fn index_ts_lookup_tables(&mut self, content: &str) {
        let obj_re = Regex::new(
            r"(?m)^\s*(?:export\s+)?(?:const|let|var)\s+([A-Za-z0-9_]+)(?:\s*:[^=]+)?\s*=\s*\{"
        ).unwrap();
        let key_re = Regex::new(r#"(?m)^\s*(?:['"]([A-Za-z0-9_./:-]{1,32})['"]|([A-Za-z_][A-Za-z0-9_]{0,31}))\s*:"#).unwrap();
        for cap in obj_re.captures_iter(content) {
            let name = cap[1].to_string();
            let start = cap.get(0).unwrap().end() - 1;
            if let Some(block) = extract_braced_block(&content[start..]) {
                let mut keys = Vec::new();
                for k_cap in key_re.captures_iter(&block) {
                    if let Some(k) = k_cap.get(1).or_else(|| k_cap.get(2)) {
                        let ks = k.as_str().to_string();
                        if !keys.contains(&ks) {
                            keys.push(ks);
                            if keys.len() >= 4 {
                                break;
                            }
                        }
                    }
                }
                if !keys.is_empty() {
                    self.lookup_table_keys.insert(name, keys);
                }
            }
        }
    }

    fn index_php_interfaces(&mut self, content: &str) {
        let ns_re = Regex::new(r"namespace\s+([^;{\s]+);").unwrap();
        let namespace = ns_re
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
                .unwrap_or_else(|| short_name_of(&fqcn).to_string());
            use_map.insert(alias, fqcn);
        }

        let iface_re = Regex::new(r"(?m)^\s*interface\s+([A-Za-z0-9_]+)[^{]*\{").unwrap();
        for cap in iface_re.captures_iter(content) {
            let iface_short = cap[1].to_string();
            let full_iface = if namespace.is_empty() {
                iface_short.clone()
            } else {
                format!("{}\\{}", namespace, iface_short)
            };
            let start = cap.get(0).unwrap().end() - 1;
            let body = extract_braced_block(&content[start..]).unwrap_or_default();

            let method_re = Regex::new(
                r"(?m)public\s+function\s+([A-Za-z0-9_]+)\s*\([^)]*\)\s*(?::\s*([?A-Za-z0-9_\\|&]+))?"
            ).unwrap();

            let mut methods = BTreeMap::new();
            for m_cap in method_re.captures_iter(&body) {
                let m_name = m_cap[1].to_string();
                let m_start = m_cap.get(0).unwrap().start();
                let docblock = extract_preceding_docblock(&body[..m_start]);

                let native_rt = m_cap.get(2).map(|m| m.as_str().trim().to_string());
                let doc_rt = docblock.as_deref().and_then(extract_phpdoc_return);

                let chosen = match (native_rt.as_deref(), doc_rt) {
                    (Some("array") | Some("iterable") | None, Some(d)) => d,
                    (Some(n), _) => n.to_string(),
                    (None, None) => continue,
                };
                let resolved = resolve_php_type_str(&chosen, &namespace, &use_map);
                methods.insert(m_name, resolved);
            }

            if !methods.is_empty() {
                self.interface_methods.insert(full_iface, methods.clone());
                self.interface_methods.entry(iface_short).or_insert(methods);
            }
        }
    }

    fn index_ts_constants_and_interfaces(&mut self, content: &str) {
        // export const FOO = 'bar';
        let const_re = Regex::new(
            r#"(?m)^\s*export\s+const\s+([A-Z0-9_]+)\s*(?::[^=]+)?=\s*['"]([^'"]+)['"]"#
        ).unwrap();
        for cap in const_re.captures_iter(content) {
            self.string_constants.insert(cap[1].to_string(), cap[2].to_string());
        }

        // export const OBJ = { KEY: 'val', ... }
        let obj_re = Regex::new(r"(?m)^\s*export\s+const\s+([A-Z0-9_]+)\s*=\s*\{").unwrap();
        for cap in obj_re.captures_iter(content) {
            let obj_name = &cap[1];
            let start = cap.get(0).unwrap().end() - 1;
            if let Some(block) = extract_braced_block(&content[start..]) {
                let field_re = Regex::new(r#"([A-Z0-9_]+)\s*:\s*['"]([^'"]+)['"]"#).unwrap();
                for f_cap in field_re.captures_iter(&block) {
                    self.string_constants.insert(
                        format!("{}.{}", obj_name, &f_cap[1]),
                        f_cap[2].to_string(),
                    );
                }
            }
        }

        // export interface Foo { bar(...): Baz; }
        let iface_re = Regex::new(r"(?m)^\s*export\s+interface\s+([A-Za-z0-9_]+)[^{]*\{").unwrap();
        for cap in iface_re.captures_iter(content) {
            let iface_name = cap[1].to_string();
            let start = cap.get(0).unwrap().end() - 1;
            let body = extract_braced_block(&content[start..]).unwrap_or_default();
            let method_re = Regex::new(r"(?m)^\s*([A-Za-z0-9_]+)\s*\([^)]*\)\s*:\s*([^;\n{]+)").unwrap();
            let mut methods = BTreeMap::new();
            for m_cap in method_re.captures_iter(&body) {
                methods.insert(m_cap[1].to_string(), m_cap[2].trim().to_string());
            }
            if !methods.is_empty() {
                self.interface_methods.insert(iface_name, methods);
            }
        }
    }

    pub fn resolve_class(&self, type_name: &str) -> Option<&ClassIndexEntry> {
        let clean = clean_type_token(type_name);
        if let Some(entry) = self.classes.get(clean) {
            return Some(entry);
        }
        let short = short_name_of(clean);
        if let Some(full) = self.short_name_map.get(short) {
            return self.classes.get(full);
        }
        None
    }

    pub fn resolve_implementer(&self, iface_name: &str) -> Option<&ClassIndexEntry> {
        let clean = clean_type_token(iface_name);
        if let Some(impls) = self.interface_impls.get(clean) {
            for cls in impls {
                if let Some(entry) = self.classes.get(cls) {
                    return Some(entry);
                }
            }
        }
        let short = short_name_of(clean);
        if let Some(impls) = self.interface_impls.get(short) {
            for cls in impls {
                if let Some(entry) = self.classes.get(cls) {
                    return Some(entry);
                }
            }
        }
        None
    }

    pub fn resolve_method_return_type(&self, type_name: &str, method: &str) -> Option<&str> {
        let clean = clean_type_token(type_name);
        if let Some(methods) = self.interface_methods.get(clean) {
            if let Some(rt) = methods.get(method) {
                return Some(rt.as_str());
            }
        }
        let short = short_name_of(clean);
        if let Some(methods) = self.interface_methods.get(short) {
            if let Some(rt) = methods.get(method) {
                return Some(rt.as_str());
            }
        }
        if let Some(cls) = self.resolve_class(clean) {
            if let Some(rt) = cls.method_return_types.get(method) {
                return Some(rt.as_str());
            }
        }
        None
    }

    pub fn resolve_constant(&self, key: &str) -> String {
        let trimmed = key.trim();
        if (trimmed.starts_with('\'') && trimmed.ends_with('\''))
            || (trimmed.starts_with('"') && trimmed.ends_with('"'))
        {
            return trimmed[1..trimmed.len() - 1].to_string();
        }
        if let Some(val) = self.string_constants.get(trimmed) {
            return val.clone();
        }
        trimmed.to_string()
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

fn short_name_of(name: &str) -> &str {
    name.rsplit('\\')
        .next()
        .unwrap_or(name)
        .rsplit('.')
        .next()
        .unwrap_or(name)
}

fn clean_type_token(type_name: &str) -> &str {
    type_name
        .trim()
        .trim_start_matches('?')
        .trim_start_matches('\\')
}

fn is_mutator_method_name(method: &str) -> bool {
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

fn extract_braced_block(source: &str) -> Option<String> {
    let mut depth = 0usize;
    let mut started = false;
    let mut start_idx = 0usize;
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

fn extract_preceding_docblock(before: &str) -> Option<String> {
    let trimmed = before.trim_end();
    if !trimmed.ends_with("*/") {
        return None;
    }
    let start = trimmed.rfind("/**")?;
    Some(trimmed[start..].to_string())
}

fn extract_phpdoc_return(doc: &str) -> Option<String> {
    let re = Regex::new(r"@(?:psalm-)?return\s+([^\s*]+)").ok()?;
    for cap in re.captures_iter(doc) {
        let ty = cap[1].trim();
        if !ty.is_empty() && ty != "mixed" {
            return Some(ty.to_string());
        }
    }
    None
}

fn resolve_php_type_str(raw: &str, namespace: &str, use_map: &HashMap<String, String>) -> String {
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
            } else if !base.contains('\\') && !namespace.is_empty() {
                format!("{}\\{}", namespace, base)
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

/// Deterministic Regex-to-Sample String Generator.
/// Parses PCRE / JS regular expressions (including `/x` free-spacing mode, character classes,
/// named/non-capturing groups, quantifiers, and alternations) and synthesizes a minimal matching string.
pub fn sample_from_regex(raw_pattern: &str) -> Option<String> {
    let trimmed = raw_pattern.trim().trim_matches('\'').trim_matches('"');
    if trimmed.is_empty() {
        return None;
    }

    let (body, flags) = unwrap_regex_delimiters(trimmed);
    let extended = flags.contains('x') || (body.contains('\n') && body.contains('#'));
    let cleaned = if extended {
        strip_extended_regex_whitespace_and_comments(body)
    } else {
        body.to_string()
    };

    let chars: Vec<char> = cleaned.chars().collect();
    let mut pos = 0usize;
    let out = gen_regex_alternation(&chars, &mut pos, false);
    let res = out.trim().to_string();
    if res.is_empty() || res.len() > 128 {
        None
    } else {
        Some(res)
    }
}

fn unwrap_regex_delimiters(s: &str) -> (&str, &str) {
    let bytes = s.as_bytes();
    if bytes.len() >= 2 {
        let first = bytes[0];
        if matches!(first, b'/' | b'#' | b'~' | b'%') {
            if let Some(last_idx) = s.rfind(first as char) {
                if last_idx > 0 {
                    let tail = &s[last_idx + 1..];
                    if tail.chars().all(|c| c.is_ascii_alphabetic()) {
                        return (&s[1..last_idx], tail);
                    }
                }
            }
        }
    }
    (s, "")
}

fn strip_extended_regex_whitespace_and_comments(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0usize;
    let mut in_bracket = false;
    while i < chars.len() {
        let c = chars[i];
        if c == '\\' && i + 1 < chars.len() {
            out.push(c);
            out.push(chars[i + 1]);
            i += 2;
            continue;
        }
        if c == '[' && !in_bracket {
            in_bracket = true;
            out.push(c);
            i += 1;
            continue;
        }
        if c == ']' && in_bracket {
            in_bracket = false;
            out.push(c);
            i += 1;
            continue;
        }
        if !in_bracket {
            if c == '#' {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
                continue;
            }
            if c.is_whitespace() {
                i += 1;
                continue;
            }
        }
        out.push(c);
        i += 1;
    }
    out
}

#[derive(Clone, Debug)]
enum AtomKind {
    Digit,
    Dot,
    Group(String),
    Literal(String),
}

fn gen_regex_alternation(chars: &[char], pos: &mut usize, in_group: bool) -> String {
    let mut branches = Vec::new();
    loop {
        let branch = gen_regex_sequence(chars, pos, in_group);
        branches.push(branch);
        if *pos < chars.len() && chars[*pos] == '|' {
            *pos += 1;
            continue;
        }
        break;
    }
    if in_group && *pos < chars.len() && chars[*pos] == ')' {
        *pos += 1;
    }
    // Prefer a non-empty alphanumeric branch (e.g. `\d{4}` -> "2024" over `[+-]\d{6}`)
    if let Some(best) = branches
        .iter()
        .find(|b| !b.is_empty() && b.chars().next().map(|c| c.is_ascii_alphanumeric()).unwrap_or(false))
    {
        return best.clone();
    }
    branches.into_iter().find(|b| !b.is_empty()).unwrap_or_default()
}

fn gen_regex_sequence(chars: &[char], pos: &mut usize, in_group: bool) -> String {
    let mut out = String::new();
    while *pos < chars.len() {
        let c = chars[*pos];
        if c == '|' || (in_group && c == ')') {
            break;
        }
        if c == '^' || c == '$' {
            *pos += 1;
            continue;
        }

        // Parse one atom
        let atom = if c == '(' {
            *pos += 1;
            parse_group_atom(chars, pos)
        } else if c == '[' {
            *pos += 1;
            let ch = parse_char_class(chars, pos);
            AtomKind::Literal(ch.to_string())
        } else if c == '.' {
            *pos += 1;
            AtomKind::Dot
        } else if c == '\\' {
            *pos += 1;
            parse_escape_atom(chars, pos)
        } else if matches!(c, '?' | '*' | '+' | '{' | ')') {
            *pos += 1;
            continue;
        } else {
            *pos += 1;
            AtomKind::Literal(c.to_string())
        };

        // Parse optional quantifier after atom
        let (min_rep, is_star_or_plus) = parse_quantifier(chars, pos);
        if min_rep == 0 {
            match &atom {
                AtomKind::Dot if is_star_or_plus => {
                    // For `.*` / `.*?` inside a capture group (e.g. `(?<nss>.*?)`), emit "a" so capture is non-empty
                    out.push('a');
                }
                AtomKind::Group(g) => {
                    // Include short alphanumeric/date optional groups (e.g. `(?:[a-z0-9-]{0,30}[a-z0-9])?` -> "a" or `(?:-?(\d\d))?` -> "-01")
                    // while skipping optional query/fragment/bracket extensions starting with `?`, `#`, `[`
                    if !g.is_empty()
                        && g.len() <= 6
                        && !g.starts_with('?')
                        && !g.starts_with('#')
                        && !g.starts_with('[')
                        && g.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '-')
                    {
                        out.push_str(g);
                    }
                }
                AtomKind::Literal(s) if s == "-" && !is_star_or_plus => {
                    out.push('-');
                }
                _ => {}
            }
        } else {
            let count = min_rep.min(64);
            match atom {
                AtomKind::Digit => {
                    // Coalesce consecutive `\d\d` into 2-digit "01"
                    if count == 1 && *pos + 1 < chars.len() && chars[*pos] == '\\' && chars[*pos + 1] == 'd' {
                        let next_after = *pos + 2;
                        let has_quant = next_after < chars.len() && matches!(chars[next_after], '?' | '*' | '+' | '{');
                        if !has_quant {
                            *pos += 2;
                            out.push_str("01");
                            continue;
                        }
                    }
                    match count {
                        4 => out.push_str("2024"),
                        2 => out.push_str("01"),
                        3 => out.push_str("001"),
                        _ => out.push_str(&"1".repeat(count)),
                    }
                }
                AtomKind::Dot => out.push_str(&"a".repeat(count)),
                AtomKind::Group(g) | AtomKind::Literal(g) => {
                    out.push_str(&g.repeat(count));
                }
            }
        }
    }
    out
}

fn parse_group_atom(chars: &[char], pos: &mut usize) -> AtomKind {
    if *pos < chars.len() && chars[*pos] == '?' {
        *pos += 1;
        if *pos >= chars.len() {
            return AtomKind::Literal(String::new());
        }
        // Lookarounds: `(?=...)`, `(?!...)`, `(?<=...)`, `(?<!...)` -> consume 0 width
        if chars[*pos] == '='
            || chars[*pos] == '!'
            || (*pos + 1 < chars.len() && chars[*pos] == '<' && (chars[*pos + 1] == '=' || chars[*pos + 1] == '!'))
        {
            let mut depth = 1usize;
            while *pos < chars.len() && depth > 0 {
                if chars[*pos] == '\\' {
                    *pos += 2;
                    continue;
                }
                if chars[*pos] == '(' {
                    depth += 1;
                } else if chars[*pos] == ')' {
                    depth -= 1;
                }
                *pos += 1;
            }
            return AtomKind::Literal(String::new());
        }
        // Named groups: `(?<name>...)` or `(?P<name>...)` or `(?'name'...)`
        if chars[*pos] == '<' || (*pos + 1 < chars.len() && chars[*pos] == 'P' && chars[*pos + 1] == '<') {
            while *pos < chars.len() && chars[*pos] != '>' {
                *pos += 1;
            }
            if *pos < chars.len() {
                *pos += 1;
            }
        } else if chars[*pos] == '\'' {
            *pos += 1;
            while *pos < chars.len() && chars[*pos] != '\'' {
                *pos += 1;
            }
            if *pos < chars.len() {
                *pos += 1;
            }
        } else {
            // Non-capturing `(?:...)` or modifier `(?i:...)` or `(?i)`
            while *pos < chars.len() && chars[*pos] != ':' && chars[*pos] != ')' {
                *pos += 1;
            }
            if *pos < chars.len() && chars[*pos] == ')' {
                *pos += 1;
                return AtomKind::Literal(String::new());
            }
            if *pos < chars.len() && chars[*pos] == ':' {
                *pos += 1;
            }
        }
    }
    let inner = gen_regex_alternation(chars, pos, true);
    AtomKind::Group(inner)
}

fn parse_char_class(chars: &[char], pos: &mut usize) -> char {
    let negated = if *pos < chars.len() && chars[*pos] == '^' {
        *pos += 1;
        true
    } else {
        false
    };
    let start = *pos;
    while *pos < chars.len() {
        if chars[*pos] == '\\' && *pos + 1 < chars.len() {
            *pos += 2;
            continue;
        }
        if chars[*pos] == ']' && *pos > start {
            break;
        }
        *pos += 1;
    }
    let slice: String = chars[start..(*pos).min(chars.len())].iter().collect();
    if *pos < chars.len() && chars[*pos] == ']' {
        *pos += 1;
    }
    if negated {
        for cand in ['a', '1', 'x'] {
            if !slice.contains(cand) {
                return cand;
            }
        }
        return 'a';
    }
    if slice.contains("a-z") || slice.contains("a-f") || slice.contains("\\w") {
        return 'a';
    }
    if slice.contains("A-Z") || slice.contains("A-F") {
        return 'A';
    }
    if slice.contains("0-9") || slice.contains("\\d") {
        return '1';
    }
    // Check explicit numeric range like `1-8`
    let s_chars: Vec<char> = slice.chars().collect();
    for i in 0..s_chars.len() {
        if i + 2 < s_chars.len() && s_chars[i + 1] == '-' && s_chars[i].is_ascii_alphanumeric() {
            return s_chars[i];
        }
    }
    s_chars
        .into_iter()
        .find(|c| *c != '\\' && *c != '-')
        .unwrap_or('a')
}

fn parse_escape_atom(chars: &[char], pos: &mut usize) -> AtomKind {
    if *pos >= chars.len() {
        return AtomKind::Literal(String::new());
    }
    let c = chars[*pos];
    *pos += 1;
    match c {
        'd' => AtomKind::Digit,
        'w' | 'D' | 'S' => AtomKind::Literal("a".to_string()),
        's' => AtomKind::Literal(" ".to_string()),
        'A' | 'z' | 'Z' | 'b' | 'B' => AtomKind::Literal(String::new()),
        'n' => AtomKind::Literal("\n".to_string()),
        'r' => AtomKind::Literal("\r".to_string()),
        't' => AtomKind::Literal("\t".to_string()),
        'x' => {
            if *pos + 2 <= chars.len() {
                let hex: String = chars[*pos..*pos + 2].iter().collect();
                if let Ok(byte) = u8::from_str_radix(&hex, 16) {
                    *pos += 2;
                    let ch = (byte as char).is_ascii_graphic().then_some(byte as char).unwrap_or('a');
                    return AtomKind::Literal(ch.to_string());
                }
            }
            AtomKind::Literal("a".to_string())
        }
        other => AtomKind::Literal(other.to_string()),
    }
}

fn parse_quantifier(chars: &[char], pos: &mut usize) -> (usize, bool) {
    if *pos >= chars.len() {
        return (1, false);
    }
    match chars[*pos] {
        '?' => {
            *pos += 1;
            if *pos < chars.len() && chars[*pos] == '?' {
                *pos += 1;
            }
            (0, false)
        }
        '*' => {
            *pos += 1;
            if *pos < chars.len() && chars[*pos] == '?' {
                *pos += 1;
            }
            (0, true)
        }
        '+' => {
            *pos += 1;
            if *pos < chars.len() && chars[*pos] == '?' {
                *pos += 1;
            }
            (1, true)
        }
        '{' => {
            let start = *pos + 1;
            let mut end = start;
            while end < chars.len() && (chars[end].is_ascii_digit() || chars[end] == ',') {
                end += 1;
            }
            if end < chars.len() && chars[end] == '}' && end > start {
                let inner: String = chars[start..end].iter().collect();
                *pos = end + 1;
                if *pos < chars.len() && chars[*pos] == '?' {
                    *pos += 1;
                }
                let min_str = inner.split(',').next().unwrap_or("1");
                let min_val = min_str.parse::<usize>().unwrap_or(1);
                return (min_val, false);
            }
            (1, false)
        }
        _ => (1, false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sample_from_regex_urn_uuid_and_iso_dates() {
        let urn_regex = r#"/^
            urn:
            (?<nid>[a-z0-9](?:[a-z0-9-]{0,30}[a-z0-9])?): # NID
            (?<nss>.*?)                                   # NSS
            (?<frc>\?\+(?<rcomponent>.*?))?               # r-component
            (?<fqc>\?\=(?<qcomponent>.*?))?               # q-component
            (?:\#(?<fcomponent>.*))?                      # f-component
        $/xi"#;
        assert_eq!(sample_from_regex(urn_regex).as_deref(), Some("urn:aa:a"));

        let nid_regex = "/^[a-z0-9]([a-z0-9-]{0,30})[a-z0-9]$/xi";
        assert_eq!(sample_from_regex(nid_regex).as_deref(), Some("aa"));

        let uuid_regex = "/^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i";
        assert_eq!(
            sample_from_regex(uuid_regex).as_deref(),
            Some("aaaaaaaa-aaaa-1aaa-8aaa-aaaaaaaaaaaa")
        );

        let sql_ymd = "/(\\d{4})-(\\d\\d)-(\\d\\d)/";
        assert_eq!(sample_from_regex(sql_ymd).as_deref(), Some("2024-01-01"));

        let iso_ymd = "/([+-]\\d{6}|\\d{4})(?:-?(\\d\\d)(?:-?(\\d\\d))?)?/";
        assert_eq!(sample_from_regex(iso_ymd).as_deref(), Some("2024-01-01"));

        let iso_currency = "/^[A-Z]{3}$/";
        assert_eq!(sample_from_regex(iso_currency).as_deref(), Some("AAA"));
    }
}
