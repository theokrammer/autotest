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
}

impl ProjectIndex {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn index_document(&mut self, doc: &ScenarioDocument) {
        let source_file = doc.source.file.clone();
        for target in &doc.targets {
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

    pub fn index_source_metadata(&mut self, _source_file: &str, content: &str, is_php: bool) {
        if is_php {
            self.index_php_interfaces(content);
        } else {
            self.index_ts_constants_and_interfaces(content);
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
