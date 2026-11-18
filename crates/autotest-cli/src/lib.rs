pub mod config;

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use clap::{Parser, Subcommand};
use autotest_core::{
    EntrypointScaffolder, FixtureDeduplicator, MergeMode, ScenarioDocument, ScenarioMerger,
    SolverConfig, SymbolicSolver,
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
    },
    /// Shortcut to record outcomes in OS sandbox
    Record {
        #[arg(long)]
        non_interactive: bool,
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
        } => {
            handle_generate(&current_dir, force, force_all, record, non_interactive)?;
        }
        Commands::Record { non_interactive } => {
            handle_generate(&current_dir, false, false, true, non_interactive)?;
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
    let mut detected = Vec::new();
    if dir.join("composer.json").exists() {
        detected.push("PHP (composer.json)");
    }
    if dir.join("package.json").exists() {
        detected.push("TypeScript/JavaScript (package.json)");
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

pub fn handle_sync_tests(dir: &Path) -> anyhow::Result<()> {
    let config = AutotestConfig::load_from_dir(dir)?;
    let tests_dir = dir.join("tests");
    fs::create_dir_all(&tests_dir)?;

    if let Some(php_cfg) = config.languages.get("php") {
        let bridge_code = EntrypointScaffolder::generate_phpunit_bridge(&config.project.scenario_dir);
        let bridge_path = dir.join(&php_cfg.bridge_file);
        if let Some(parent) = bridge_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&bridge_path, bridge_code)?;
        println!("Generated PHPUnit bridge: {}", bridge_path.display());
    }

    if let Some(ts_cfg) = config.languages.get("typescript") {
        let is_vitest = ts_cfg.test_framework.to_lowercase() == "vitest";
        let bridge_code = EntrypointScaffolder::generate_ts_bridge(&config.project.scenario_dir, is_vitest);
        let bridge_path = dir.join(&ts_cfg.bridge_file);
        if let Some(parent) = bridge_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&bridge_path, bridge_code)?;
        println!("Generated TypeScript bridge: {}", bridge_path.display());
    }

    Ok(())
}

pub fn handle_generate(
    dir: &Path,
    force: bool,
    force_all: bool,
    record: bool,
    _non_interactive: bool,
) -> anyhow::Result<()> {
    let config = AutotestConfig::load_from_dir(dir)?;
    let scenario_dir = dir.join(&config.project.scenario_dir);
    fs::create_dir_all(&scenario_dir)?;

    let mode = if force_all {
        MergeMode::ForceAll
    } else if force {
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

    let solver = SymbolicSolver::new(SolverConfig {
        max_depth: config.solver.max_depth,
        max_loop_unroll: config.solver.max_loop_unroll,
    });

    // 1. Scan configured source roots for each language and extract targets
    for (lang_name, lang_cfg) in &config.languages {
        for root in &lang_cfg.source_roots {
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

                // Skip non-source, test files, and common dependency dirs
                let path_str = path.to_string_lossy();
                if path_str.contains("/node_modules/")
                    || path_str.contains("/vendor/")
                    || path_str.contains("/target/")
                {
                    continue;
                }

                // Skip hidden files or hidden subdirectories inside root
                if let Ok(rel_inside_root) = path.strip_prefix(&root_path) {
                    if rel_inside_root.components().any(|c| c.as_os_str().to_string_lossy().starts_with('.')) {
                        continue;
                    }
                }

                let is_ts = lang_name == "typescript"
                    && (ext == "ts" || ext == "tsx")
                    && !file_name.ends_with(".d.ts")
                    && !file_name.contains(".test.")
                    && !file_name.contains(".spec.");

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
                        continue;
                    }
                };

                let rel_path = path.strip_prefix(dir).unwrap_or(path);
                let rel_path_str = rel_path.to_string_lossy();

                let extraction_result = if is_ts {
                    autotest_ts_precheck::TsPrecheckExtractor::extract_source(&rel_path_str, &content)
                        .map_err(|e| anyhow::anyhow!("{}", e))
                } else {
                    autotest_php_extractor::PhpExtractor::extract_source(&rel_path_str, &content)
                        .map_err(|e| anyhow::anyhow!("{}", e))
                };

                let mut extracted_doc = match extraction_result {
                    Ok(doc) => doc,
                    Err(e) => {
                        tracing::warn!("Extraction error for {}: {}", rel_path_str, e);
                        continue;
                    }
                };

                if extracted_doc.targets.is_empty() {
                    continue;
                }

                total_source_files += 1;

                // Determine target .autotest.json path in scenario_dir
                // Example: src/order/OrderService.ts with root "src" -> tests/scenarios/order/OrderService.autotest.json
                let rel_from_root = path.strip_prefix(&root_path).unwrap_or(rel_path);
                let rel_without_ext = rel_from_root.with_extension("");
                let scenario_filename = format!("{}.autotest.json", rel_without_ext.display());
                let target_scenario_path = scenario_dir.join(scenario_filename);

                if let Some(parent) = target_scenario_path.parent() {
                    fs::create_dir_all(parent)?;
                }

                // Check for existing scenario file
                let existing_doc = if target_scenario_path.exists() {
                    let existing_content = fs::read_to_string(&target_scenario_path)?;
                    serde_json::from_str::<ScenarioDocument>(&existing_content).ok()
                } else {
                    None
                };

                // Solve symbolic contracts for extracted targets
                for target in &mut extracted_doc.targets {
                    let new_scenarios = solver.solve_target(target);
                    if !new_scenarios.is_empty() {
                        target.scenarios = new_scenarios;
                    }
                }

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
        "Generated: {} scenarios added across {} source files, {} locked scenarios preserved",
        total_added, total_source_files, total_locked
    );

    if !all_stale.is_empty() {
        println!("\n⚠ Veraltete kuratierte Assertions (locked: true):");
        for stale in all_stale {
            println!("  - {} :: \"{}\" ({})", stale.file, stale.scenario_name, stale.reason);
        }
    }

    // 3. If record requested, run the test runners in isolated OS sandbox to record outcomes
    if record {
        handle_record_execution(dir, &config)?;
    }

    Ok(())
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
        let bridge_path = dir.join(&lang_cfg.bridge_file);

        // Ensure bridge file exists
        if !bridge_path.exists() {
            println!("Generating missing test bridge for {}: {}", lang_name, lang_cfg.bridge_file);
            let bridge_code = if lang_name == "typescript" {
                let is_vitest = lang_cfg.test_framework.to_lowercase() == "vitest";
                EntrypointScaffolder::generate_ts_bridge(&config.project.scenario_dir, is_vitest)
            } else {
                EntrypointScaffolder::generate_phpunit_bridge(&config.project.scenario_dir)
            };
            if let Some(parent) = bridge_path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(&bridge_path, bridge_code)?;
        }

        println!("Running {} test runner in sandbox (record mode)...", lang_name);

        let (program, args): (&str, Vec<&str>) = match lang_name.as_str() {
            "typescript" => {
                if lang_cfg.test_framework.to_lowercase() == "vitest" {
                    ("npx", vec!["vitest", "run", &lang_cfg.bridge_file])
                } else {
                    ("npx", vec!["jest", &lang_cfg.bridge_file])
                }
            }
            "php" => {
                let phpunit_bin = dir.join("vendor/bin/phpunit");
                if phpunit_bin.exists() {
                    ("vendor/bin/phpunit", vec![lang_cfg.bridge_file.as_str()])
                } else {
                    ("phpunit", vec![lang_cfg.bridge_file.as_str()])
                }
            }
            _ => continue,
        };

        let envs = [("AUTOTEST_RECORD", "1")];
        match sandbox_runner.run_command_with_env(program, &args, dir, &envs) {
            Ok(result) => {
                println!(
                    "  Execution completed in {:?} (exit code: {:?})",
                    result.duration,
                    result.exit_code
                );
                if result.exit_code != Some(0) && !result.stderr.is_empty() {
                    tracing::debug!("Sandbox stderr: {}", result.stderr);
                }
            }
            Err(e) => {
                println!("  Sandbox execution note for {}: {}", lang_name, e);
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
