pub mod config;
pub mod coverage_feedback;

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use clap::{Parser, Subcommand};
use autotest_core::{
    EntrypointScaffolder, FixtureDeduplicator, MergeMode, ProjectIndex, ScenarioDocument,
    ScenarioMerger, SolverConfig, SymbolicSolver,
};
pub use config::AutotestConfig;

#[derive(Parser, Debug)]
#[command(name = "autotest", version, about = "Deterministic, language-agnostic contract testing engine")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Initialize autotest configuration and scaffold directories
    Init {
        /// Use offline vendored single-file hydrator instead of package manager
        #[arg(long)]
        vendor: bool,
    },
    /// Perform static analysis, symbolic evaluation, and merge scenarios
    Generate {
        /// Overwrite unlocked scenarios
        #[arg(long)]
        force: bool,
        /// Overwrite all scenarios including locked ones
        #[arg(long)]
        force_all: bool,
        /// Record real execution outcomes in OS sandbox
        #[arg(long)]
        record: bool,
        /// Non-interactive mode for CI/CD pipelines
        #[arg(long)]
        non_interactive: bool,
        /// Restrict generation to matching path prefixes or globs
        #[arg(long)]
        include: Vec<String>,
        /// Exclude files matching path prefixes or globs
        #[arg(long)]
        exclude: Vec<String>,
        /// Run iterative coverage-guided scenario synthesis after recording
        #[arg(long)]
        coverage_guided: bool,
        /// Maximum coverage-guided feedback iterations
        #[arg(long, default_value_t = 2)]
        max_iterations: usize,
    },
    /// Shortcut to record outcomes in OS sandbox
    Record {
        #[arg(long)]
        non_interactive: bool,
        /// Run iterative coverage-guided scenario synthesis after recording
        #[arg(long)]
        coverage_guided: bool,
        /// Maximum coverage-guided feedback iterations
        #[arg(long, default_value_t = 2)]
        max_iterations: usize,
    },
    /// Generate native test runner bridge files
    SyncTests,
    /// Verify scenario suite (fails on unresolved stale scenarios or pending staging files)
    Verify,
    /// Approve and merge staged recordings (.autotest.staging.json) into primary scenarios
    Approve {
        /// Optional specific staging or scenario file to approve
        #[arg(long)]
        file: Option<String>,
    },
}

pub fn run_cli(cli: Cli) -> anyhow::Result<()> {
    let current_dir = std::env::current_dir()?;

    match cli.command {
        Commands::Init { vendor } => {
            handle_init(&current_dir, vendor)?;
        }
        Commands::Generate {
            force,
            force_all,
            record,
            non_interactive,
            include,
            exclude,
            coverage_guided,
            max_iterations,
        } => {
            handle_generate_with_options(
                &current_dir,
                GenerateOptions {
                    force,
                    force_all,
                    record: record || coverage_guided,
                    non_interactive,
                    include,
                    exclude,
                    coverage_guided,
                    max_iterations,
                },
            )?;
        }
        Commands::Record {
            non_interactive,
            coverage_guided,
            max_iterations,
        } => {
            handle_generate_with_options(
                &current_dir,
                GenerateOptions {
                    force: false,
                    force_all: false,
                    record: true,
                    non_interactive,
                    include: Vec::new(),
                    exclude: Vec::new(),
                    coverage_guided,
                    max_iterations,
                },
            )?;
        }
        Commands::SyncTests => {
            handle_sync_tests(&current_dir)?;
        }
        Commands::Verify => {
            handle_verify(&current_dir)?;
        }
        Commands::Approve { file } => {
            handle_approve(&current_dir, file.as_deref())?;
        }
    }

    Ok(())
}

pub fn detect_source_roots(dir: &Path) -> Vec<String> {
    let mut roots = Vec::new();

    // 1. Monorepo pattern: packages/*/src or packages/*
    let packages_dir = dir.join("packages");
    if packages_dir.is_dir() {
        if let Ok(entries) = fs::read_dir(&packages_dir) {
            for entry in entries.filter_map(|e| e.ok()) {
                let p = entry.path();
                if p.is_dir() {
                    let pkg_name = entry.file_name().to_string_lossy().to_string();
                    if p.join("src").is_dir() {
                        roots.push(format!("packages/{}/src", pkg_name));
                    } else if p.join("lib").is_dir() {
                        roots.push(format!("packages/{}/lib", pkg_name));
                    } else {
                        roots.push(format!("packages/{}", pkg_name));
                    }
                }
            }
        }
    }

    // 2. Standard "src" directory
    if dir.join("src").is_dir() {
        roots.push("src".to_string());
    }

    // 3. Standard "lib" directory
    if dir.join("lib").is_dir() && !roots.iter().any(|r| r == "src") {
        roots.push("lib".to_string());
    }

    // 4. Fallback to root "." if source files exist at root
    if roots.is_empty() {
        if let Ok(entries) = fs::read_dir(dir) {
            let has_root_sources = entries.filter_map(|e| e.ok()).any(|e| {
                let p = e.path();
                if p.is_file() {
                    let ext = p.extension().and_then(|s| s.to_str()).unwrap_or("");
                    ["ts", "tsx", "js", "mjs", "php"].contains(&ext)
                } else {
                    false
                }
            });
            if has_root_sources {
                roots.push(".".to_string());
            }
        }
    }

    if roots.is_empty() {
        roots.push("src".to_string());
    }

    roots
}

pub fn handle_init(dir: &Path, vendor: bool) -> anyhow::Result<()> {
    let mut config = if dir.join("autotest.toml").exists() {
        AutotestConfig::load_from_dir(dir)?
    } else {
        AutotestConfig::default()
    };
    if vendor {
        config.project.hydrator_install = "vendor".to_string();
    }

    // Detect project files
    let has_composer = dir.join("composer.json").exists();
    let has_package_json = dir.join("package.json").exists();
    let mut detected = Vec::new();
    if has_composer {
        detected.push("PHP (composer.json)");
        if !has_package_json {
            config.languages.remove("typescript");
        }
        if let Some(php_cfg) = config.languages.get_mut("php") {
            if dir.join("src").exists() {
                php_cfg.source_roots = vec!["src".to_string()];
            } else if dir.join("app").exists() {
                php_cfg.source_roots = vec!["app".to_string()];
            }
        }
    }
    if has_package_json {
        detected.push("TypeScript/JavaScript (package.json)");
        if !has_composer {
            config.languages.remove("php");
        }
        if let Some(ts_cfg) = config.languages.get_mut("typescript") {
            let detected_roots = detect_source_roots(dir);
            if !detected_roots.is_empty() {
                ts_cfg.source_roots = detected_roots;
            }
            if let Ok(pkg_str) = fs::read_to_string(dir.join("package.json")) {
                if pkg_str.contains("\"vitest\"") {
                    ts_cfg.test_framework = "vitest".to_string();
                    ts_cfg.bridge_file = detect_ts_bridge_file(dir);
                } else if pkg_str.contains("\"jest\"") {
                    ts_cfg.test_framework = "jest".to_string();
                    ts_cfg.bridge_file = detect_ts_bridge_file(dir);
                } else {
                    ts_cfg.test_framework = "node".to_string();
                    ts_cfg.bridge_file = "tests/autotest.bridge.test.mjs".to_string();
                }
            } else {
                ts_cfg.bridge_file = detect_ts_bridge_file(dir);
            }
        }
    }

    config.save_to_dir(dir)?;
    fs::create_dir_all(dir.join(&config.project.scenario_dir))?;

    println!("Initialized autotest in {}", dir.display());
    if !detected.is_empty() {
        println!("Detected project ecosystems: {}", detected.join(", "));
    }
    println!("Configuration written to autotest.toml");
    Ok(())
}

fn detect_ts_bridge_file(dir: &Path) -> String {
    let config_candidates = [
        "vitest.config.ts",
        "vitest.config.mts",
        "vitest.config.js",
        "vitest.config.mjs",
        "vite.config.ts",
        "jest.config.ts",
        "jest.config.js",
        "jest.config.mjs",
        "package.json",
    ];
    for candidate in &config_candidates {
        let path = dir.join(candidate);
        if let Ok(content) = fs::read_to_string(&path) {
            if content.contains(".spec.") && !content.contains(".test.") {
                return "tests/autotest.bridge.spec.ts".to_string();
            }
        }
    }
    "tests/autotest.bridge.test.ts".to_string()
}

fn should_run_language(dir: &Path, lang_name: &str) -> bool {
    let has_composer = dir.join("composer.json").exists();
    let has_package_json = dir.join("package.json").exists();
    if lang_name == "php" && !has_composer && has_package_json {
        return false;
    }
    if (lang_name == "typescript" || lang_name == "javascript") && !has_package_json && has_composer {
        return false;
    }
    true
}

pub fn handle_sync_tests(dir: &Path) -> anyhow::Result<()> {
    let config = AutotestConfig::load_from_dir(dir)?;
    let tests_dir = dir.join("tests");
    fs::create_dir_all(&tests_dir)?;

    if should_run_language(dir, "php") {
        if let Some(php_cfg) = config.languages.get("php") {
            let bridge_code = EntrypointScaffolder::generate_phpunit_bridge(&config.project.scenario_dir);
            let bridge_path = dir.join(&php_cfg.bridge_file);
            if let Some(parent) = bridge_path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(&bridge_path, bridge_code)?;
            println!("Generated PHPUnit bridge: {}", bridge_path.display());
        }
    }

    if should_run_language(dir, "typescript") {
        if let Some(ts_cfg) = config.languages.get("typescript") {
            let fw = ts_cfg.test_framework.to_lowercase();
            let bridge_code = if fw == "node" {
                EntrypointScaffolder::generate_node_bridge(&config.project.scenario_dir)
            } else {
                EntrypointScaffolder::generate_ts_bridge(&config.project.scenario_dir, fw == "vitest")
            };
            let bridge_path = dir.join(&ts_cfg.bridge_file);
            if let Some(parent) = bridge_path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(&bridge_path, bridge_code)?;
            println!("Generated TypeScript/JS bridge: {}", bridge_path.display());
        }
    }

    Ok(())
}

fn classify_empty_source(content: &str, is_php: bool) -> &'static str {
    if is_php {
        let has_class = content.contains("class ");
        if !has_class && content.contains("interface ") {
            "interface only (not directly executable)"
        } else if !has_class && content.contains("trait ") {
            "trait only (not directly executable)"
        } else if !has_class && content.contains("enum ") {
            "enum only (no testable methods)"
        } else if content.contains("abstract class ") {
            "abstract class (no concrete public methods)"
        } else if has_class {
            "class with no public non-magic methods (e.g. exception/DTO/marker class)"
        } else {
            "no extractable classes or functions"
        }
    } else if !content.contains("class ") && !content.contains("function ") && !content.contains("=>") {
        if content.contains("interface ") || content.contains("type ") {
            "type/interface definitions only"
        } else if content.contains("export *") || content.contains("export {") {
            "re-export / barrel file"
        } else {
            "no extractable public classes or functions"
        }
    } else {
        "no extractable public classes or functions"
    }
}

#[derive(Debug, Clone, Default)]
pub struct GenerateOptions {
    pub force: bool,
    pub force_all: bool,
    pub record: bool,
    pub non_interactive: bool,
    pub include: Vec<String>,
    pub exclude: Vec<String>,
    pub coverage_guided: bool,
    pub max_iterations: usize,
}

pub fn matches_scope_pattern(rel_path: &str, pattern: &str) -> bool {
    let norm_path = rel_path.replace('\\', "/");
    let norm_pat = pattern.trim().trim_start_matches("./").trim_end_matches('/');
    if norm_pat.is_empty() {
        return false;
    }
    if !norm_pat.contains('*') && !norm_pat.contains('?') {
        return norm_path == norm_pat || norm_path.starts_with(&format!("{}/", norm_pat));
    }
    let mut rx = String::from("^");
    let chars: Vec<char> = norm_pat.chars().collect();
    let mut i = 0usize;
    while i < chars.len() {
        match chars[i] {
            '*' => {
                if i + 1 < chars.len() && chars[i + 1] == '*' {
                    rx.push_str(".*");
                    i += 2;
                    if i < chars.len() && chars[i] == '/' {
                        i += 1;
                    }
                } else {
                    rx.push_str("[^/]*");
                    i += 1;
                }
            }
            '?' => {
                rx.push_str("[^/]");
                i += 1;
            }
            c => {
                rx.push_str(&regex::escape(&c.to_string()));
                i += 1;
            }
        }
    }
    rx.push('$');
    regex::Regex::new(&rx).map(|r| r.is_match(&norm_path)).unwrap_or(false)
}

pub fn handle_generate(
    dir: &Path,
    force: bool,
    force_all: bool,
    record: bool,
    non_interactive: bool,
) -> anyhow::Result<()> {
    handle_generate_with_options(
        dir,
        GenerateOptions {
            force,
            force_all,
            record,
            non_interactive,
            ..Default::default()
        },
    )
}

pub fn handle_generate_with_options(dir: &Path, opts: GenerateOptions) -> anyhow::Result<()> {
    let config = AutotestConfig::load_from_dir(dir)?;
    let scenario_dir = dir.join(&config.project.scenario_dir);
    fs::create_dir_all(&scenario_dir)?;

    let mode = if opts.force_all {
        MergeMode::ForceAll
    } else if opts.force {
        MergeMode::Force
    } else {
        MergeMode::NonDestructive
    };

    println!("Running autotest generate (mode: {:?})...", mode);

    let mut total_added = 0;
    let mut total_locked = 0;
    let mut all_stale = Vec::new();
    let mut processed_scenario_files: HashSet<PathBuf> = HashSet::new();
    let mut total_source_files = 0;
    let mut total_scanned_files = 0;
    let mut skipped_by_reason: std::collections::BTreeMap<&'static str, Vec<String>> =
        std::collections::BTreeMap::new();

    let solver = SymbolicSolver::new(SolverConfig {
        max_depth: config.solver.max_depth,
        max_loop_unroll: config.solver.max_loop_unroll,
    });

    let mut project_index = ProjectIndex::new();
    let mut pending_docs: Vec<(PathBuf, ScenarioDocument)> = Vec::new();

    // Pass 1: Scan configured source roots, index metadata/interfaces/classes, and extract targets
    for (lang_name, lang_cfg) in &config.languages {
        if !should_run_language(dir, lang_name) {
            continue;
        }
        let mut roots_to_scan = lang_cfg.source_roots.clone();
        let any_exists = roots_to_scan.iter().any(|r| dir.join(r).exists());
        if !any_exists {
            let discovered = detect_source_roots(dir);
            if !discovered.is_empty() && discovered.iter().any(|r| dir.join(r).exists()) {
                println!(
                    "Notice: Configured source_roots {:?} not found for {}. Auto-detected source roots: {:?}",
                    roots_to_scan, lang_name, discovered
                );
                roots_to_scan = discovered;
            } else {
                println!(
                    "Warning: Configured source_roots {:?} for {} do not exist in {}",
                    roots_to_scan, lang_name, dir.display()
                );
            }
        }

        let effective_includes: Vec<String> = if !opts.include.is_empty() {
            opts.include.clone()
        } else {
            lang_cfg.include_paths.clone()
        };
        let mut effective_excludes = lang_cfg.exclude_paths.clone();
        effective_excludes.extend(opts.exclude.iter().cloned());

        for root in &roots_to_scan {
            let root_path = dir.join(root);
            if !root_path.exists() {
                continue;
            }

            for entry in walkdir::WalkDir::new(&root_path)
                .into_iter()
                .filter_map(|e| e.ok())
            {
                let path = entry.path();
                if !path.is_file() {
                    continue;
                }

                let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
                let file_name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");

                let rel_path = path.strip_prefix(dir).unwrap_or(path);
                let rel_path_str = rel_path.to_string_lossy();

                // Skip non-source, test files, and common dependency/build dirs
                let path_str = path.to_string_lossy();
                if path_str.contains("/node_modules/")
                    || path_str.contains("/vendor/")
                    || path_str.contains("/target/")
                    || path_str.contains("/dist/")
                    || path_str.contains("/build/")
                    || path_str.contains("/coverage/")
                    || path_str.contains("/.git/")
                    || path_str.contains("/test/")
                    || path_str.contains("/tests/")
                    || path_str.contains("/__tests__/")
                {
                    continue;
                }

                // Skip test & scenario directories when scanning project root "."
                if root == "."
                    && (rel_path_str.starts_with("tests/")
                        || rel_path_str.starts_with("test/")
                        || rel_path_str.starts_with("__tests__/")
                        || rel_path_str.starts_with(&config.project.scenario_dir))
                {
                    continue;
                }

                // Skip hidden files or hidden subdirectories inside root
                if let Ok(rel_inside_root) = path.strip_prefix(&root_path) {
                    if rel_inside_root.components().any(|c| c.as_os_str().to_string_lossy().starts_with('.')) {
                        continue;
                    }
                }

                let is_ts = (lang_name == "typescript" || lang_name == "javascript")
                    && (ext == "ts" || ext == "tsx" || ext == "js" || ext == "mjs" || ext == "cjs")
                    && !file_name.ends_with(".d.ts")
                    && !file_name.contains(".test.")
                    && !file_name.contains(".spec.")
                    && !file_name.contains(".min.");

                let is_php = lang_name == "php"
                    && ext == "php"
                    && !file_name.ends_with("Test.php");

                if !is_ts && !is_php {
                    continue;
                }

                let content = match fs::read_to_string(path) {
                    Ok(c) => c,
                    Err(e) => {
                        tracing::warn!("Failed to read {}: {}", path.display(), e);
                        skipped_by_reason
                            .entry("unreadable file")
                            .or_default()
                            .push(rel_path_str.to_string());
                        continue;
                    }
                };

                // Always index interfaces/constants even if excluded from target generation
                project_index.index_source_metadata(&rel_path_str, &content, is_php);

                // Scope filtering
                if !effective_includes.is_empty()
                    && !effective_includes.iter().any(|pat| matches_scope_pattern(&rel_path_str, pat))
                {
                    skipped_by_reason
                        .entry("excluded by scope filter")
                        .or_default()
                        .push(rel_path_str.to_string());
                    continue;
                }
                if effective_excludes.iter().any(|pat| matches_scope_pattern(&rel_path_str, pat)) {
                    skipped_by_reason
                        .entry("excluded by scope filter")
                        .or_default()
                        .push(rel_path_str.to_string());
                    continue;
                }

                total_scanned_files += 1;

                // Skip executable CLI scripts (shebang)
                if content.starts_with("#!") {
                    skipped_by_reason
                        .entry("executable CLI script (shebang)")
                        .or_default()
                        .push(rel_path_str.to_string());
                    continue;
                }

                let extraction_result = if is_ts {
                    autotest_ts_precheck::TsPrecheckExtractor::extract_source(&rel_path_str, &content)
                        .map_err(|e| anyhow::anyhow!("{}", e))
                } else {
                    autotest_php_extractor::PhpExtractor::extract_source(&rel_path_str, &content)
                        .map_err(|e| anyhow::anyhow!("{}", e))
                };

                let extracted_doc = match extraction_result {
                    Ok(doc) => doc,
                    Err(e) => {
                        tracing::warn!("Extraction error for {}: {}", rel_path_str, e);
                        skipped_by_reason
                            .entry("parse/extraction error")
                            .or_default()
                            .push(rel_path_str.to_string());
                        continue;
                    }
                };

                if extracted_doc.targets.is_empty() {
                    let reason = classify_empty_source(&content, is_php);
                    skipped_by_reason
                        .entry(reason)
                        .or_default()
                        .push(rel_path_str.to_string());
                    continue;
                }

                total_source_files += 1;
                project_index.index_document(&extracted_doc);

                let scenario_filename = if root == "." || root.starts_with("packages/") {
                    format!("{}.autotest.json", rel_path.with_extension("").display())
                } else {
                    let rel_from_root = path.strip_prefix(&root_path).unwrap_or(rel_path);
                    let rel_without_ext = rel_from_root.with_extension("");
                    format!("{}.autotest.json", rel_without_ext.display())
                };
                let target_scenario_path = scenario_dir.join(scenario_filename);
                pending_docs.push((target_scenario_path, extracted_doc));
            }
        }
    }

    // Pass 2: Solve all buffered documents with cross-file ProjectIndex
    for (target_scenario_path, mut extracted_doc) in pending_docs {
        if let Some(parent) = target_scenario_path.parent() {
            fs::create_dir_all(parent)?;
        }

        let existing_doc = if target_scenario_path.exists() {
            let existing_content = fs::read_to_string(&target_scenario_path)?;
            serde_json::from_str::<ScenarioDocument>(&existing_content).ok()
        } else {
            None
        };

        solver.solve_document_with_index(&mut extracted_doc, &project_index);

        if config.project.deduplicate_fixtures {
            FixtureDeduplicator::deduplicate(&mut extracted_doc);
        }

        let (merged, report) = ScenarioMerger::merge(existing_doc.as_ref(), &extracted_doc, mode);
        total_added += report.added_scenarios;
        total_locked += report.retained_locked;
        all_stale.extend(report.stale_assertions);

        let output = if config.project.canonical_sort {
            serde_json::to_string_pretty(&merged)?
        } else {
            serde_json::to_string(&merged)?
        };

        fs::write(&target_scenario_path, output)?;
        if let Ok(canonical) = target_scenario_path.canonicalize() {
            processed_scenario_files.insert(canonical);
        } else {
            processed_scenario_files.insert(target_scenario_path.clone());
        }
    }

    // 2. Also re-evaluate any pre-existing scenarios in scenario_dir not processed from sources
    if scenario_dir.exists() {
        for entry in walkdir::WalkDir::new(&scenario_dir)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            let path = entry.path();
            if path.is_file() && path.extension().and_then(|s| s.to_str()) == Some("json") {
                let filename = path.file_name().unwrap().to_string_lossy();
                if filename.ends_with(".autotest.json") && !filename.contains(".staging.") {
                    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
                    if processed_scenario_files.contains(&canonical) {
                        continue;
                    }

                    let content = fs::read_to_string(path)?;
                    if let Ok(existing_doc) = serde_json::from_str::<ScenarioDocument>(&content) {
                        let mut generated_doc = existing_doc.clone();

                        for target in &mut generated_doc.targets {
                            let new_scenarios = solver.solve_target(target);
                            if !new_scenarios.is_empty() {
                                target.scenarios = new_scenarios;
                            }
                        }

                        if config.project.deduplicate_fixtures {
                            FixtureDeduplicator::deduplicate(&mut generated_doc);
                        }

                        let (merged, report) = ScenarioMerger::merge(Some(&existing_doc), &generated_doc, mode);
                        total_added += report.added_scenarios;
                        total_locked += report.retained_locked;
                        all_stale.extend(report.stale_assertions);

                        let output = if config.project.canonical_sort {
                            serde_json::to_string_pretty(&merged)?
                        } else {
                            serde_json::to_string(&merged)?
                        };

                        fs::write(path, output)?;
                    }
                }
            }
        }
    }

    println!(
        "Generated: {} scenarios added across {} source files (out of {} scanned), {} locked scenarios preserved",
        total_added, total_source_files, total_scanned_files, total_locked
    );

    if !skipped_by_reason.is_empty() {
        let total_skipped: usize = skipped_by_reason.values().map(|v| v.len()).sum();
        println!("Skipped {} source files with no testable targets:", total_skipped);
        for (reason, files) in &skipped_by_reason {
            let preview: Vec<&str> = files.iter().take(3).map(|s| s.as_str()).collect();
            let suffix = if files.len() > 3 {
                format!(", ... (+{} more)", files.len() - 3)
            } else {
                String::new()
            };
            println!(
                "  - {}: {} ({}{})",
                reason,
                files.len(),
                preview.join(", "),
                suffix
            );
        }
    }

    if !all_stale.is_empty() {
        println!("\n⚠ Veraltete kuratierte Assertions (locked: true):");
        for stale in all_stale {
            println!("  - {} :: \"{}\" ({})", stale.file, stale.scenario_name, stale.reason);
        }
    }

    // 3. If record requested, run the test runners in isolated OS sandbox to record outcomes
    if opts.record || opts.coverage_guided {
        handle_record_execution(dir, &config)?;
    }

    // 4. Optional coverage-guided feedback loop
    if opts.coverage_guided {
        coverage_feedback::run_coverage_guided_loop(dir, &config, &project_index, opts.max_iterations)?;
    }

    Ok(())
}

fn prepare_standalone_php_runner() -> anyhow::Result<PathBuf> {
    let runner_dir = std::env::temp_dir().join("autotest-php-runner");
    fs::create_dir_all(&runner_dir)?;

    let files: &[(&str, &str)] = &[
        ("Loader.php", include_str!("../../../adapters/hydrators/phpunit-autotest/src/Loader.php")),
        ("DtoHydrator.php", include_str!("../../../adapters/hydrators/phpunit-autotest/src/DtoHydrator.php")),
        ("MockBinder.php", include_str!("../../../adapters/hydrators/phpunit-autotest/src/MockBinder.php")),
        ("TargetInvoker.php", include_str!("../../../adapters/hydrators/phpunit-autotest/src/TargetInvoker.php")),
        ("OutcomeAsserter.php", include_str!("../../../adapters/hydrators/phpunit-autotest/src/OutcomeAsserter.php")),
        ("RecorderHook.php", include_str!("../../../adapters/hydrators/phpunit-autotest/src/RecorderHook.php")),
        ("AutotestHydrator.php", include_str!("../../../adapters/hydrators/phpunit-autotest/src/AutotestHydrator.php")),
    ];

    for (name, content) in files {
        fs::write(runner_dir.join(name), content)?;
    }

    let runner_script = r#"<?php
declare(strict_types=1);

$projectDir = getcwd();
$scenarioDir = $argv[1] ?? ($projectDir . '/tests/scenarios');

if (file_exists($projectDir . '/vendor/autoload.php')) {
    require_once $projectDir . '/vendor/autoload.php';
}

require_once __DIR__ . '/Loader.php';
require_once __DIR__ . '/DtoHydrator.php';
require_once __DIR__ . '/MockBinder.php';
require_once __DIR__ . '/TargetInvoker.php';
require_once __DIR__ . '/OutcomeAsserter.php';
require_once __DIR__ . '/RecorderHook.php';
require_once __DIR__ . '/AutotestHydrator.php';

$hydrator = new \Autotest\Hydrator\AutotestHydrator();
$dummyTestCase = new class {
    public function assertEquals($expected, $actual): void {}
};

if (!is_dir($scenarioDir)) {
    fwrite(STDERR, "Scenario directory not found: {$scenarioDir}\n");
    exit(1);
}

$iterator = new RecursiveIteratorIterator(new RecursiveDirectoryIterator($scenarioDir));
$recordedCount = 0;
$errorCount = 0;

foreach ($iterator as $file) {
    if (!$file->isFile()) continue;
    $path = $file->getPathname();
    if (!str_ends_with($path, '.autotest.json') || str_ends_with($path, '.staging.json')) {
        continue;
    }

    $data = json_decode((string)file_get_contents($path), true);
    if (!is_array($data)) continue;

    $sourceFile = $data['source']['file'] ?? null;
    if ($sourceFile) {
        $fullSource = $projectDir . '/' . ltrim($sourceFile, '/');
        if (file_exists($fullSource)) {
            require_once $fullSource;
        }
    }

    foreach ($data['targets'] ?? [] as $target) {
        foreach ($target['scenarios'] ?? [] as $scenario) {
            try {
                $hydrator->executeScenario($path, $scenario, $dummyTestCase, [
                    'record' => true,
                    'target_id' => $target['id'] ?? null,
                ]);
                $recordedCount++;
            } catch (\Throwable $e) {
                $errorCount++;
                fwrite(STDERR, "[autotest] {$target['id']} :: {$scenario['name']}: {$e->getMessage()}\n");
            }
        }
    }
}

echo "Recorded {$recordedCount} scenarios ({$errorCount} errors)\n";
"#;

    let run_path = runner_dir.join("run.php");
    fs::write(&run_path, runner_script)?;
    Ok(run_path)
}

fn has_scenarios_for_language(scenario_dir: &Path, lang_name: &str) -> bool {
    if !scenario_dir.exists() {
        return false;
    }
    for entry in walkdir::WalkDir::new(scenario_dir)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let path = entry.path();
        if path.is_file() && path.to_string_lossy().ends_with(".autotest.json") {
            if let Ok(content) = fs::read_to_string(path) {
                if let Ok(doc) = serde_json::from_str::<ScenarioDocument>(&content) {
                    let src = doc.source.file.to_lowercase();
                    let is_php = src.ends_with(".php");
                    let is_ts = src.ends_with(".ts")
                        || src.ends_with(".tsx")
                        || src.ends_with(".js")
                        || src.ends_with(".mjs")
                        || src.ends_with(".cjs");
                    if lang_name == "php" && is_php {
                        return true;
                    }
                    if (lang_name == "typescript" || lang_name == "javascript") && is_ts {
                        return true;
                    }
                }
            }
        }
    }
    false
}

fn prepare_embedded_js_hydrator() -> anyhow::Result<PathBuf> {
    let hydrator_dir = std::env::temp_dir().join("autotest-js-hydrator");
    fs::create_dir_all(&hydrator_dir)?;

    let files: &[(&str, &str)] = &[
        ("Loader.js", include_str!("../../../adapters/hydrators/jest-autotest/src/Loader.js")),
        ("DtoHydrator.js", include_str!("../../../adapters/hydrators/jest-autotest/src/DtoHydrator.js")),
        ("MockBinder.js", include_str!("../../../adapters/hydrators/jest-autotest/src/MockBinder.js")),
        ("TargetInvoker.js", include_str!("../../../adapters/hydrators/jest-autotest/src/TargetInvoker.js")),
        ("OutcomeAsserter.js", include_str!("../../../adapters/hydrators/jest-autotest/src/OutcomeAsserter.js")),
        ("RecorderHook.js", include_str!("../../../adapters/hydrators/jest-autotest/src/RecorderHook.js")),
        ("index.js", include_str!("../../../adapters/hydrators/jest-autotest/src/index.js")),
    ];

    for (name, content) in files {
        fs::write(hydrator_dir.join(name), content)?;
    }

    Ok(hydrator_dir.join("index.js"))
}

fn prepare_vitest_bridge_config(dir: &Path, bridge_file: &str) -> anyhow::Result<PathBuf> {
    let config_path = std::env::temp_dir().join("autotest.vitest.config.mjs");
    let escaped_bridge = serde_json::to_string(bridge_file)?;

    // Ensure @autotest/jest resolves even if not installed (or broken symlink) in target node_modules
    let needs_alias = !dir.join("node_modules/@autotest/jest/package.json").is_file();
    let alias_entry = if needs_alias {
        let embedded = prepare_embedded_js_hydrator()?;
        let escaped_hydrator = serde_json::to_string(&embedded.to_string_lossy().to_string())?;
        format!(r#""@autotest/jest": {}"#, escaped_hydrator)
    } else {
        String::new()
    };

    let existing_candidates = [
        "vitest.config.ts",
        "vitest.config.mts",
        "vitest.config.js",
        "vitest.config.mjs",
        "vite.config.ts",
        "vite.config.mts",
        "vite.config.js",
        "vite.config.mjs",
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
    resolve: {{
      ...baseResolve,
      alias: mergedAlias,
    }},
    test: {{
      ...baseTest,
      include: Array.from(new Set([...existingInclude, {escaped_bridge}])),
    }},
  }};
}};
"#,
            escaped_import = escaped_import,
            escaped_bridge = escaped_bridge,
            alias_entry = alias_entry,
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
            "export default {{\n  resolve: {{\n    alias: {{ {alias_entry} }},\n  }},\n  test: {{\n    include: [{escaped_bridge}],\n  }},\n}};\n",
            alias_entry = alias_entry,
            escaped_bridge = escaped_bridge
        )
    };

    fs::write(&config_path, content)?;
    Ok(config_path)
}

pub fn handle_record_execution(dir: &Path, config: &AutotestConfig) -> anyhow::Result<()> {
    println!("\nRecording execution outcomes in OS sandbox...");
    let sandbox_runner = autotest_sandbox::SandboxRunner::new(autotest_sandbox::SandboxConfig {
        mode: config.sandbox.mode.clone(),
        timeout_ms: config.sandbox.timeout_ms,
        block_network: config.sandbox.block_network,
        allow_temp_fs_only: config.sandbox.allow_temp_fs_only,
    });

    let scenario_dir = dir.join(&config.project.scenario_dir);
    let mut total_staged_entries = 0;

    for (lang_name, lang_cfg) in &config.languages {
        if !should_run_language(dir, lang_name) {
            continue;
        }
        if !has_scenarios_for_language(&scenario_dir, lang_name) {
            continue;
        }

        let bridge_path = dir.join(&lang_cfg.bridge_file);

        // Ensure bridge file exists
        if !bridge_path.exists() {
            println!("Generating missing test bridge for {}: {}", lang_name, lang_cfg.bridge_file);
            let bridge_code = if lang_name == "typescript" || lang_name == "javascript" {
                let fw = lang_cfg.test_framework.to_lowercase();
                if fw == "node" {
                    EntrypointScaffolder::generate_node_bridge(&config.project.scenario_dir)
                } else {
                    EntrypointScaffolder::generate_ts_bridge(&config.project.scenario_dir, fw == "vitest")
                }
            } else {
                EntrypointScaffolder::generate_phpunit_bridge(&config.project.scenario_dir)
            };
            if let Some(parent) = bridge_path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(&bridge_path, bridge_code)?;
        }

        println!("Running {} test runner in sandbox (record mode)...", lang_name);

        let local_vitest = dir.join("node_modules/.bin/vitest");
        let local_jest = dir.join("node_modules/.bin/jest");
        let local_phpunit = dir.join("vendor/bin/phpunit");
        let mut extra_env_owned: Vec<(String, String)> = vec![("AUTOTEST_RECORD".to_string(), "1".to_string())];

        let (program, args): (String, Vec<String>) = match lang_name.as_str() {
            "typescript" | "javascript" => {
                let fw = lang_cfg.test_framework.to_lowercase();
                if fw == "node" {
                    let embedded = prepare_embedded_js_hydrator()?;
                    extra_env_owned.push((
                        "AUTOTEST_JS_HYDRATOR".to_string(),
                        embedded.to_string_lossy().to_string(),
                    ));
                    (
                        "node".to_string(),
                        vec!["--test".to_string(), lang_cfg.bridge_file.clone()],
                    )
                } else if fw == "vitest" {
                    let vitest_cfg = prepare_vitest_bridge_config(dir, &lang_cfg.bridge_file)?;
                    let vitest_cfg_str = vitest_cfg.to_string_lossy().to_string();
                    if local_vitest.exists() {
                        (
                            local_vitest.to_string_lossy().to_string(),
                            vec![
                                "run".to_string(),
                                "--config".to_string(),
                                vitest_cfg_str,
                                lang_cfg.bridge_file.clone(),
                            ],
                        )
                    } else {
                        (
                            "npx".to_string(),
                            vec![
                                "--no-install".to_string(),
                                "vitest".to_string(),
                                "run".to_string(),
                                "--config".to_string(),
                                vitest_cfg_str,
                                lang_cfg.bridge_file.clone(),
                            ],
                        )
                    }
                } else {
                    let jest_match = format!("<rootDir>/{}", lang_cfg.bridge_file.trim_start_matches('/'));
                    if local_jest.exists() {
                        (
                            local_jest.to_string_lossy().to_string(),
                            vec![
                                "--testMatch".to_string(),
                                jest_match,
                                lang_cfg.bridge_file.clone(),
                            ],
                        )
                    } else {
                        (
                            "npx".to_string(),
                            vec![
                                "--no-install".to_string(),
                                "jest".to_string(),
                                "--testMatch".to_string(),
                                jest_match,
                                lang_cfg.bridge_file.clone(),
                            ],
                        )
                    }
                }
            }
            "php" => {
                let has_hydrator_pkg = dir.join("vendor/autotest/phpunit-autotest").exists();
                if local_phpunit.exists() && has_hydrator_pkg {
                    (
                        local_phpunit.to_string_lossy().to_string(),
                        vec![lang_cfg.bridge_file.clone()],
                    )
                } else {
                    let runner_path = prepare_standalone_php_runner()?;
                    (
                        "php".to_string(),
                        vec![
                            runner_path.to_string_lossy().to_string(),
                            scenario_dir.to_string_lossy().to_string(),
                        ],
                    )
                }
            }
            _ => continue,
        };

        let arg_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        let env_refs: Vec<(&str, &str)> = extra_env_owned
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        match sandbox_runner.run_command_with_env(&program, &arg_refs, dir, &env_refs) {
            Ok(result) => {
                println!(
                    "  Execution completed in {:?} (exit code: {:?})",
                    result.duration,
                    result.exit_code
                );
                if result.exit_code != Some(0) {
                    let err_output = if !result.stderr.trim().is_empty() {
                        result.stderr.trim()
                    } else {
                        result.stdout.trim()
                    };
                    if !err_output.is_empty() {
                        let preview: String = err_output.lines().take(10).collect::<Vec<_>>().join("\n  ");
                        println!("  Runner output:\n  {}", preview);
                    }
                }
            }
            Err(e) => {
                println!("  Sandbox execution note for {}: {}", lang_name, e);
                if config.sandbox.timeout_ms < 5_000 && e.to_string().contains("timed out") {
                    println!(
                        "  Hint: sandbox.timeout_ms in autotest.toml is {} ms. Increase it (e.g. timeout_ms = 10000).",
                        config.sandbox.timeout_ms
                    );
                }
            }
        }
    }

    // Inspect staged files
    if let Ok(staged_files) = autotest_sandbox::StagingManager::find_staging_files(&scenario_dir) {
        for sf in &staged_files {
            if let Ok(entries) = autotest_sandbox::StagingManager::read_staging(sf) {
                total_staged_entries += entries.len();
                println!("  Staged {} recorded scenarios in {}", entries.len(), sf.display());
            }
        }
    }

    if total_staged_entries > 0 {
        println!(
            "Recorded {} outcomes staged for review. Verify with `autotest verify`.",
            total_staged_entries
        );
    } else {
        println!("No new outcomes were staged.");
    }

    Ok(())
}

pub fn handle_verify(dir: &Path) -> anyhow::Result<()> {
    let config = AutotestConfig::load_from_dir(dir)?;
    let scenario_dir = dir.join(&config.project.scenario_dir);
    let conformance_dir = dir.join("conformance");

    let mut staging_files = Vec::new();
    let mut stale_scenarios = Vec::new();
    let mut total_scenarios = 0u64;
    let mut total_targets = 0u64;

    // Scan both scenario_dir and conformance directories
    let scan_dirs: Vec<std::path::PathBuf> = vec![scenario_dir, conformance_dir]
        .into_iter()
        .filter(|d| d.exists())
        .collect();

    for scan_dir in &scan_dirs {
        // Use StagingManager to find staging files
        let found_staging = autotest_sandbox::StagingManager::find_staging_files(scan_dir)
            .unwrap_or_default();
        staging_files.extend(found_staging);

        for entry in walkdir::WalkDir::new(scan_dir)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            let path = entry.path();
            if path.is_file() {
                let name = path.file_name().unwrap_or_default().to_string_lossy();
                if name.ends_with(".autotest.json") && !name.contains(".staging.") {
                    let content = fs::read_to_string(path)?;
                    if let Ok(doc) = serde_json::from_str::<ScenarioDocument>(&content) {
                        for target in &doc.targets {
                            total_targets += 1;
                            for scenario in &target.scenarios {
                                total_scenarios += 1;
                                if scenario.stale {
                                    stale_scenarios.push((
                                        path.to_path_buf(),
                                        target.id.clone(),
                                        scenario.name.clone(),
                                    ));
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    let mut has_errors = false;
    if !staging_files.is_empty() {
        has_errors = true;
        eprintln!("Verification failed: Unverified staging files found:");
        for sf in &staging_files {
            eprintln!("  - {}", sf.display());
        }
        eprintln!("Hint: Run `autotest approve` to review and merge staged outcomes into primary scenarios.");
    }

    if !stale_scenarios.is_empty() {
        has_errors = true;
        eprintln!("Verification failed: Unresolved stale scenarios found:");
        for (file, target, sc_name) in &stale_scenarios {
            eprintln!("  - {} :: {} ({})", file.display(), target, sc_name);
        }
        eprintln!("Hint: Run `autotest generate --force` or `--force-all` to re-synchronize stale assertions.");
    }

    if has_errors {
        anyhow::bail!("autotest verify failed: please resolve stale assertions or commit verified staging files.");
    }

    println!(
        "autotest verify: All scenarios and staging checks passed cleanly.\n  {} targets, {} scenarios across {} directories",
        total_targets,
        total_scenarios,
        scan_dirs.len()
    );
    Ok(())
}

pub fn handle_approve(dir: &Path, file_filter: Option<&str>) -> anyhow::Result<()> {
    let config = AutotestConfig::load_from_dir(dir)?;
    let scenario_dir = dir.join(&config.project.scenario_dir);

    let staging_files = if let Some(target_file) = file_filter {
        let p = PathBuf::from(target_file);
        let path = if p.is_absolute() { p } else { dir.join(p) };
        if !path.exists() {
            anyhow::bail!("Specified file does not exist: {}", path.display());
        }
        let staging_path = if path.file_name().unwrap_or_default().to_string_lossy().contains(".staging.") {
            path.clone()
        } else {
            autotest_sandbox::StagingManager::staging_path_for(&path)
        };
        if !staging_path.exists() {
            anyhow::bail!("No staging file found for {}", path.display());
        }
        vec![staging_path]
    } else {
        autotest_sandbox::StagingManager::find_staging_files(&scenario_dir)?
    };

    if staging_files.is_empty() {
        println!("No pending staging files found to approve in {}", scenario_dir.display());
        return Ok(());
    }

    let mut total_scenarios = 0;
    let mut total_files = 0;

    for sf in staging_files {
        let count = autotest_sandbox::StagingManager::merge_verified_staging(&sf)?;
        println!("  Approved and merged {} scenario(s) from {}", count, sf.display());
        total_files += 1;
        total_scenarios += count;
    }

    println!(
        "\nSuccessfully approved: {} scenarios merged across {} file(s).",
        total_scenarios, total_files
    );
    Ok(())
}
