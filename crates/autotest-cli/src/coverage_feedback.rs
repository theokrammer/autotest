use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use regex::Regex;
use serde_json::{json, Map, Value};
use autotest_core::{
    FixtureDefinition, FixtureException, FixtureReturn, MockReferenceOrInline, ProjectIndex,
    ScenarioDefinition, ScenarioDocument, SetupCall, TargetDefinition,
};
use crate::{handle_approve, handle_record_execution, prepare_embedded_js_hydrator, should_run_language, AutotestConfig};

const MAX_SCENARIOS_PER_TARGET_PER_ITER: usize = 3;
const MAX_TOTAL_SCENARIOS_PER_ITER: usize = 150;

pub fn run_coverage_guided_loop(
    dir: &Path,
    config: &AutotestConfig,
    _index: &ProjectIndex,
    max_iterations: usize,
) -> anyhow::Result<()> {
    if max_iterations == 0 {
        return Ok(());
    }

    let scenario_dir = dir.join(&config.project.scenario_dir);
    if !scenario_dir.exists() {
        return Ok(());
    }

    // Ensure any pending staged recordings from initial --record are approved before measuring baseline coverage
    let _ = handle_approve(dir, None);

    let mut prev_uncovered_total: Option<usize> = None;

    for iter in 1..=max_iterations {
        println!("\n[coverage-guided] Iteration {}/{}: measuring coverage...", iter, max_iterations);
        let uncovered_by_file = collect_uncovered_lines(dir, config)?;
        let total_uncovered: usize = uncovered_by_file.values().map(|v| v.len()).sum();

        if uncovered_by_file.is_empty() || total_uncovered == 0 {
            println!("[coverage-guided] No uncovered lines detected (or no coverage provider available); stopping.");
            break;
        }

        if let Some(prev) = prev_uncovered_total {
            if total_uncovered >= prev {
                println!(
                    "[coverage-guided] Coverage plateaued ({} uncovered lines >= {}); stopping.",
                    total_uncovered, prev
                );
                break;
            }
        }
        prev_uncovered_total = Some(total_uncovered);

        println!(
            "[coverage-guided] Found {} uncovered lines across {} files. Synthesizing targeted scenarios...",
            total_uncovered,
            uncovered_by_file.len()
        );

        let added = synthesize_targeted_scenarios_from_coverage(dir, &scenario_dir, &uncovered_by_file)?;
        if added == 0 {
            println!("[coverage-guided] 0 new targeted scenarios synthesized; stopping.");
            break;
        }

        println!(
            "[coverage-guided] Added {} targeted scenarios; recording & approving...",
            added
        );
        handle_record_execution(dir, config)?;
        let _ = handle_approve(dir, None);
    }

    Ok(())
}

fn collect_uncovered_lines(dir: &Path, config: &AutotestConfig) -> anyhow::Result<BTreeMap<String, Vec<usize>>> {
    let mut uncovered = BTreeMap::new();

    for (lang_name, lang_cfg) in &config.languages {
        if !should_run_language(dir, lang_name) {
            continue;
        }
        let bridge_path = dir.join(&lang_cfg.bridge_file);
        if !bridge_path.exists() {
            continue;
        }

        match lang_name.as_str() {
            "php" => {
                let local_phpunit = dir.join("vendor/bin/phpunit");
                if !local_phpunit.exists() || !php_has_coverage_driver() {
                    continue;
                }
                let clover_path = std::env::temp_dir().join("autotest_clover.xml");
                let _ = fs::remove_file(&clover_path);
                let out = Command::new(&local_phpunit)
                    .arg("--coverage-clover")
                    .arg(&clover_path)
                    .arg(&lang_cfg.bridge_file)
                    .current_dir(dir)
                    .output();
                if let Ok(_) = out {
                    if clover_path.exists() {
                        if let Ok(xml) = fs::read_to_string(&clover_path) {
                            parse_clover_uncovered(dir, &xml, &mut uncovered);
                        }
                        let _ = fs::remove_file(&clover_path);
                    }
                }
            }
            "typescript" | "javascript" => {
                let fw = lang_cfg.test_framework.to_lowercase();
                if fw == "node" {
                    let out = Command::new("node")
                        .arg("--test")
                        .arg("--experimental-test-coverage")
                        .arg("--test-reporter=lcov")
                        .arg(&lang_cfg.bridge_file)
                        .current_dir(dir)
                        .output();
                    if let Ok(res) = out {
                        let lcov = String::from_utf8_lossy(&res.stdout);
                        parse_lcov_uncovered(dir, &lcov, &mut uncovered);
                    }
                } else if fw == "vitest" {
                    let local_vitest = dir.join("node_modules/.bin/vitest");
                    if !local_vitest.exists() {
                        continue;
                    }
                    let cov_dir = std::env::temp_dir().join("autotest-vitest-cov");
                    let _ = fs::remove_dir_all(&cov_dir);
                    fs::create_dir_all(&cov_dir)?;
                    if let Ok(cfg_path) = prepare_vitest_coverage_config(dir, &lang_cfg.bridge_file, &cov_dir) {
                        let _ = Command::new(&local_vitest)
                            .arg("run")
                            .arg("--config")
                            .arg(&cfg_path)
                            .arg("--coverage")
                            .arg(&lang_cfg.bridge_file)
                            .current_dir(dir)
                            .output();
                        let cov_json = cov_dir.join("coverage-final.json");
                        if cov_json.exists() {
                            if let Ok(raw) = fs::read_to_string(&cov_json) {
                                parse_istanbul_uncovered(dir, &raw, &mut uncovered);
                            }
                        }
                    }
                    let _ = fs::remove_dir_all(&cov_dir);
                }
            }
            _ => {}
        }
    }

    Ok(uncovered)
}

fn php_has_coverage_driver() -> bool {
    if let Ok(out) = Command::new("php").arg("-m").output() {
        let mods = String::from_utf8_lossy(&out.stdout).to_lowercase();
        return mods.contains("pcov") || mods.contains("xdebug");
    }
    false
}

fn prepare_vitest_coverage_config(dir: &Path, bridge_file: &str, cov_dir: &Path) -> anyhow::Result<PathBuf> {
    let config_path = std::env::temp_dir().join("autotest.vitest.cov.config.mjs");
    let escaped_bridge = serde_json::to_string(bridge_file)?;
    let escaped_cov_dir = serde_json::to_string(&cov_dir.to_string_lossy().to_string())?;

    let needs_alias = !dir.join("node_modules/@autotest/jest/package.json").is_file();
    let alias_entry = if needs_alias {
        let embedded = prepare_embedded_js_hydrator()?;
        format!(r#""@autotest/jest": {}"#, serde_json::to_string(&embedded.to_string_lossy().to_string())?)
    } else {
        String::new()
    };

    let existing_candidates = [
        "vitest.config.ts",
        "vitest.config.mts",
        "vitest.config.js",
        "vitest.config.mjs",
        "vite.config.ts",
    ];
    let existing_cfg = existing_candidates
        .iter()
        .map(|name| dir.join(name))
        .find(|p| p.is_file());

    let content = if let Some(user_cfg_path) = existing_cfg {
        let abs = user_cfg_path.canonicalize().unwrap_or(user_cfg_path);
        let escaped_import = serde_json::to_string(&abs.to_string_lossy().to_string())?;
        format!(
            r#"import userConfigExport from {escaped_import};

export default async (env) => {{
  const base = typeof userConfigExport === "function"
    ? await userConfigExport(env)
    : (userConfigExport || {{}});
  const baseTest = base.test || {{}};
  const baseResolve = base.resolve || {{}};
  const baseAlias = baseResolve.alias || {{}};
  const existingInclude = Array.isArray(baseTest.include) ? baseTest.include : [];
  const mergedAlias = Array.isArray(baseAlias)
    ? [...baseAlias{array_alias}]
    : {{ ...baseAlias, {alias_entry} }};
  return {{
    ...base,
    resolve: {{ ...baseResolve, alias: mergedAlias }},
    test: {{
      ...baseTest,
      include: Array.from(new Set([...existingInclude, {escaped_bridge}])),
      coverage: {{
        ...(baseTest.coverage || {{}}),
        enabled: true,
        reportOnFailure: true,
        provider: "v8",
        reporter: ["json"],
        reportsDirectory: {escaped_cov_dir},
      }},
    }},
  }};
}};
"#,
            array_alias = if needs_alias {
                let embedded = prepare_embedded_js_hydrator()?;
                format!(
                    ", {{ find: \"@autotest/jest\", replacement: {} }}",
                    serde_json::to_string(&embedded.to_string_lossy().to_string())?
                )
            } else {
                String::new()
            }
        )
    } else {
        format!(
            r#"export default {{
  resolve: {{ alias: {{ {alias_entry} }} }},
  test: {{
    include: [{escaped_bridge}],
    coverage: {{
      enabled: true,
      reportOnFailure: true,
      provider: "v8",
      reporter: ["json"],
      reportsDirectory: {escaped_cov_dir},
    }},
  }},
}};
"#
        )
    };

    fs::write(&config_path, content)?;
    Ok(config_path)
}

fn normalize_rel_path(dir: &Path, raw_file: &str) -> String {
    let p = Path::new(raw_file);
    if let Ok(rel) = p.strip_prefix(dir) {
        return rel.to_string_lossy().replace('\\', "/");
    }
    if let Ok(canon_dir) = dir.canonicalize() {
        if let Ok(rel) = p.strip_prefix(&canon_dir) {
            return rel.to_string_lossy().replace('\\', "/");
        }
    }
    raw_file.trim_start_matches("./").replace('\\', "/")
}

fn parse_clover_uncovered(dir: &Path, xml: &str, out: &mut BTreeMap<String, Vec<usize>>) {
    let file_re = Regex::new(r#"<file\s+name="([^"]+)""#).unwrap();
    let line_re = Regex::new(r#"<line\s+num="(\d+)"[^>]*count="0""#).unwrap();

    let mut current_file: Option<String> = None;
    for raw_line in xml.lines() {
        if let Some(cap) = file_re.captures(raw_line) {
            current_file = Some(normalize_rel_path(dir, &cap[1]));
        } else if raw_line.contains("</file>") {
            current_file = None;
        } else if let Some(ref f) = current_file {
            if let Some(cap) = line_re.captures(raw_line) {
                if let Ok(num) = cap[1].parse::<usize>() {
                    out.entry(f.clone()).or_default().push(num);
                }
            }
        }
    }
}

fn parse_lcov_uncovered(dir: &Path, lcov: &str, out: &mut BTreeMap<String, Vec<usize>>) {
    let mut current_file: Option<String> = None;
    for line in lcov.lines() {
        let trimmed = line.trim();
        if let Some(sf) = trimmed.strip_prefix("SF:") {
            current_file = Some(normalize_rel_path(dir, sf));
        } else if trimmed == "end_of_record" {
            current_file = None;
        } else if let Some(da) = trimmed.strip_prefix("DA:") {
            if let Some(ref f) = current_file {
                let mut parts = da.split(',');
                if let (Some(l_str), Some(c_str)) = (parts.next(), parts.next()) {
                    if c_str == "0" {
                        if let Ok(num) = l_str.parse::<usize>() {
                            out.entry(f.clone()).or_default().push(num);
                        }
                    }
                }
            }
        }
    }
}

fn parse_istanbul_uncovered(dir: &Path, raw_json: &str, out: &mut BTreeMap<String, Vec<usize>>) {
    let Ok(Value::Object(files_map)) = serde_json::from_str::<Value>(raw_json) else {
        return;
    };
    for (abs_path, file_cov) in files_map {
        let rel = normalize_rel_path(dir, &abs_path);
        let mut lines = HashSet::new();
        if let (Some(stmt_map), Some(s_counts)) = (
            file_cov.get("statementMap").and_then(|v| v.as_object()),
            file_cov.get("s").and_then(|v| v.as_object()),
        ) {
            for (id, count_val) in s_counts {
                if count_val.as_u64() == Some(0) {
                    if let Some(line_num) = stmt_map
                        .get(id)
                        .and_then(|st| st.get("start"))
                        .and_then(|s| s.get("line"))
                        .and_then(|l| l.as_u64())
                    {
                        lines.insert(line_num as usize);
                    }
                }
            }
        }
        if !lines.is_empty() {
            let mut sorted: Vec<usize> = lines.into_iter().collect();
            sorted.sort_unstable();
            out.insert(rel, sorted);
        }
    }
}

fn synthesize_targeted_scenarios_from_coverage(
    dir: &Path,
    scenario_dir: &Path,
    uncovered_by_file: &BTreeMap<String, Vec<usize>>,
) -> anyhow::Result<usize> {
    let mut total_added = 0usize;

    for entry in walkdir::WalkDir::new(scenario_dir)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        if total_added >= MAX_TOTAL_SCENARIOS_PER_ITER {
            break;
        }
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let fname = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
        if !fname.ends_with(".autotest.json") || fname.contains(".staging.") {
            continue;
        }

        let content = fs::read_to_string(path)?;
        let Ok(mut doc) = serde_json::from_str::<ScenarioDocument>(&content) else {
            continue;
        };
        let src_rel = doc.source.file.replace('\\', "/");
        let Some(uncovered_lines) = uncovered_by_file.get(&src_rel) else {
            continue;
        };
        if uncovered_lines.is_empty() {
            continue;
        }

        let src_path = dir.join(&src_rel);
        let Ok(src_content) = fs::read_to_string(&src_path) else {
            continue;
        };
        let src_lines: Vec<&str> = src_content.lines().collect();
        let method_ranges = index_method_line_ranges(&src_lines);

        let mut doc_modified = false;
        for target in &mut doc.targets {
            if total_added >= MAX_TOTAL_SCENARIOS_PER_ITER {
                break;
            }
            if target.scenarios.is_empty() {
                continue;
            }

            let method_name = target
                .id
                .rsplit(|c| c == ':' || c == '.')
                .next()
                .unwrap_or(&target.id);
            let Some(&(start_line, end_line)) = method_ranges.get(method_name) else {
                continue;
            };

            let target_uncovered: Vec<usize> = uncovered_lines
                .iter()
                .copied()
                .filter(|&ln| ln >= start_line && ln <= end_line)
                .collect();
            if target_uncovered.is_empty() {
                continue;
            }

            let mut added_for_target = 0usize;
            for ln in target_uncovered {
                if added_for_target >= MAX_SCENARIOS_PER_TARGET_PER_ITER
                    || total_added >= MAX_TOTAL_SCENARIOS_PER_ITER
                {
                    break;
                }
                let idx = ln.saturating_sub(1);
                if idx >= src_lines.len() {
                    continue;
                }
                let lo = idx.saturating_sub(1);
                let hi = (idx + 2).min(src_lines.len());
                let snippet = src_lines[lo..hi].join("\n");

                if let Some(sc) = synthesize_scenario_for_snippet(target, &snippet, ln) {
                    if !target.scenarios.iter().any(|s| s.name == sc.name) {
                        target.scenarios.push(sc);
                        added_for_target += 1;
                        total_added += 1;
                        doc_modified = true;
                    }
                }
            }
        }

        if doc_modified {
            fs::write(path, serde_json::to_string_pretty(&doc)?)?;
        }
    }

    Ok(total_added)
}

fn index_method_line_ranges(lines: &[&str]) -> BTreeMap<String, (usize, usize)> {
    let mut ranges = BTreeMap::new();
    let fn_re = Regex::new(
        r"(?:public|protected|private|static|async|export|function|\s)*\b([A-Za-z_][A-Za-z0-9_]*)\s*\([^)]*\)"
    ).unwrap();

    let mut i = 0usize;
    while i < lines.len() {
        let line = lines[i].trim();
        if (line.contains("function ") || line.starts_with("public ") || line.starts_with("async ") || line.starts_with("export "))
            && !line.starts_with("if")
            && !line.starts_with("for")
            && !line.starts_with("while")
            && !line.starts_with("switch")
        {
            if let Some(cap) = fn_re.captures(line) {
                let name = cap[1].to_string();
                if name != "if" && name != "for" && name != "while" && name != "switch" && name != "catch" && name != "__construct" {
                    let start = i + 1;
                    let mut depth = 0isize;
                    let mut seen_open = false;
                    let mut end = start;
                    for j in i..lines.len() {
                        for ch in lines[j].chars() {
                            if ch == '{' {
                                depth += 1;
                                seen_open = true;
                            } else if ch == '}' {
                                depth -= 1;
                            }
                        }
                        if seen_open && depth <= 0 {
                            end = j + 1;
                            break;
                        }
                    }
                    if seen_open {
                        ranges.entry(name).or_insert((start, end));
                    }
                }
            }
        }
        i += 1;
    }
    ranges
}

fn synthesize_scenario_for_snippet(
    target: &TargetDefinition,
    snippet: &str,
    line_num: usize,
) -> Option<ScenarioDefinition> {
    let base = target.scenarios.first()?.clone();
    let target_prefix = target.id.replace("::", "_").replace('\\', "_").replace('.', "_");

    // 1. Uncovered `catch (...)` block -> force a mockable dependency to throw an exception
    if snippet.contains("catch (") || snippet.contains("catch(") {
        if let Some(eff) = target
            .detected_side_effects
            .iter()
            .find(|e| e.mockable && (e.effect_type == "interface_call" || e.effect_type == "service_call"))
        {
            let mut sc = base.clone();
            sc.name = format!("{}_cov_catch_L{}", target_prefix, line_num);
            sc.locked = false;
            sc.stale = false;
            sc.tags = vec!["coverage_guided".to_string(), "exception_path".to_string()];
            sc.mocks = vec![MockReferenceOrInline::Inline(FixtureDefinition {
                target: eff.target.clone(),
                when_args: Vec::new(),
                returns: vec![FixtureReturn {
                    return_value: None,
                    exception: Some(FixtureException {
                        exception_type: if target.id.contains("::") {
                            "RuntimeException".to_string()
                        } else {
                            "Error".to_string()
                        },
                        message: Some("simulated coverage failure".to_string()),
                    }),
                }],
                mutate_args: BTreeMap::new(),
                expect_calls: None,
            })];
            return Some(sc);
        }
    }

    // 2. Uncovered literal comparison (`=== 'lit'` or `'lit' =>` or `case 'lit':`)
    let lit_re = Regex::new(r#"(?:===?\s*|case\s+|['"]\s*=>|includes\(|startsWith\()\s*['"]([A-Za-z0-9_./:-]{1,32})['"]"#).unwrap();
    if let Some(cap) = lit_re.captures(snippet) {
        let lit = &cap[1];
        for p in &target.signature.parameters {
            if p.param_type.to_lowercase().contains("string") || p.param_type == "mixed" || p.param_type == "any" {
                let mut sc = base.clone();
                sc.name = format!("{}_cov_lit_{}_L{}", target_prefix, sanitize_tag(lit), line_num);
                sc.locked = false;
                sc.stale = false;
                sc.tags = vec!["coverage_guided".to_string(), "literal_branch".to_string()];
                if let Value::Object(mut map) = sc.inputs.clone() {
                    map.insert(p.name.clone(), json!(lit));
                    sc.inputs = Value::Object(map);
                    return Some(sc);
                }
            }
        }
        // Also check if a mocked dependency method (like request->getMethod()) is compared to lit
        if let Some(eff) = target.detected_side_effects.iter().find(|e| {
            e.mockable
                && (e.effect_type == "interface_call" || e.effect_type == "service_call")
                && snippet.contains(e.target.split('.').last().unwrap_or(""))
        }) {
            let mut sc = base.clone();
            sc.name = format!("{}_cov_mocklit_{}_L{}", target_prefix, sanitize_tag(lit), line_num);
            sc.locked = false;
            sc.stale = false;
            sc.tags = vec!["coverage_guided".to_string(), "mock_literal".to_string()];
            sc.mocks.push(MockReferenceOrInline::Inline(FixtureDefinition {
                target: eff.target.clone(),
                when_args: Vec::new(),
                returns: vec![FixtureReturn {
                    return_value: Some(json!(lit)),
                    exception: None,
                }],
                mutate_args: BTreeMap::new(),
                expect_calls: None,
            }));
            return Some(sc);
        }
    }

    // 3. Uncovered repeated-call / "already set" / cache hit guard
    if (snippet.contains("already") || snippet.contains("Cache") || snippet.contains("cache"))
        && target.kind == autotest_core::TargetKind::Method
    {
        if let Some(method_name) = target.id.rsplit(|c| c == ':' || c == '.').next() {
            let mut sc = base.clone();
            sc.name = format!("{}_cov_repeat_L{}", target_prefix, line_num);
            sc.locked = false;
            sc.stale = false;
            sc.tags = vec!["coverage_guided".to_string(), "stateful_repeat".to_string()];
            let args: Vec<Value> = if let Value::Object(ref map) = sc.inputs {
                target
                    .signature
                    .parameters
                    .iter()
                    .filter_map(|p| map.get(&p.name).cloned())
                    .collect()
            } else {
                Vec::new()
            };
            sc.setup_calls.push(SetupCall {
                method: method_name.to_string(),
                args,
            });
            return Some(sc);
        }
    }

    // 4. Uncovered null check (`=== null` or `=== undefined`)
    if snippet.contains("=== null") || snippet.contains("=== undefined") || snippet.contains("isNull(") {
        if let Some(p) = target.signature.parameters.first() {
            let mut sc = base.clone();
            sc.name = format!("{}_cov_null_L{}", target_prefix, line_num);
            sc.locked = false;
            sc.stale = false;
            sc.tags = vec!["coverage_guided".to_string(), "null_branch".to_string()];
            if let Value::Object(mut map) = sc.inputs.clone() {
                map.insert(p.name.clone(), Value::Null);
                sc.inputs = Value::Object(map);
                return Some(sc);
            }
        }
    }

    // 5. Uncovered property access on object parameter (`param.field`)
    for p in &target.signature.parameters {
        let prop_re = Regex::new(&format!(r"\b{}\??\.([A-Za-z_][A-Za-z0-9_]*)", regex::escape(&p.name))).unwrap();
        if let Some(cap) = prop_re.captures(snippet) {
            let field = &cap[1];
            let mut sc = base;
            sc.name = format!("{}_cov_prop_{}_L{}", target_prefix, field, line_num);
            sc.locked = false;
            sc.stale = false;
            sc.tags = vec!["coverage_guided".to_string(), "prop_branch".to_string()];
            if let Value::Object(mut map) = sc.inputs.clone() {
                let mut obj = match map.get(&p.name) {
                    Some(Value::Object(m)) => m.clone(),
                    _ => Map::new(),
                };
                if !obj.contains_key(field) {
                    obj.insert(field.to_string(), json!("test"));
                    map.insert(p.name.clone(), Value::Object(obj));
                    sc.inputs = Value::Object(map);
                    return Some(sc);
                }
            }
            break;
        }
    }

    None
}

fn sanitize_tag(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .collect()
}
