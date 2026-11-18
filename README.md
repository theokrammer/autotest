# Autotest

**Autotest** is a deterministic, language-agnostic contract testing engine. It combines AST static analysis, symbolic boundary solving, non-destructive scenario merging, and OS-sandboxed recording to generate and maintain test suites for TypeScript and PHP projects.

---

## Key Features

- **Language-Agnostic Scenario Contracts**: Test cases are defined as declarative JSON documents (`*.autotest.json`) conforming to [schema/scenario.v1.schema.json](schema/scenario.v1.schema.json).
- **Static Analysis & Symbolic Boundary Solving**: Automatically identifies function and method signatures, injected dependencies, and evaluates boundary conditions (e.g. zero floats, fixed-length strings, negative guards, regex constraints).
- **Non-Destructive Scenario Merger**: Preserves curated developer scenarios (`locked: true`), flags contract drift (`stale: true`), and safely synchronizes generated test cases.
- **Dependency Graph & Cache Invalidation**: Uses Tarjan's strongly connected components (SCC) algorithm to handle cyclic dependencies and selectively invalidate affected scenarios.
- **Isolated OS Sandbox & Staged Recording**: Safely executes real test runners in a sandboxed subshell (blocking network, enforcing timeouts, and isolating temp storage) to record real outcomes into `.autotest.staging.json` before approval.
- **Native Test Runner Bridges**: Scaffolds seamless data-provider and `test.each` bridges for **PHPUnit**, **Vitest**, and **Jest**.

---

## Architecture Overview

```
.
├── schema/
│   └── scenario.v1.schema.json       # JSON Schema definition (v1.0)
├── crates/
│   ├── autotest-core/                # IR, SymbolicSolver, ScenarioMerger, DependencyGraph, Scaffolder
│   ├── autotest-proto/               # JSON-RPC 2.0 protocol specifications for language workers
│   ├── autotest-sandbox/             # Process sandbox runner & StagingManager lifecycle
│   ├── autotest-ts-precheck/         # AST extractor for TypeScript / TSX
│   ├── autotest-php-extractor/       # AST extractor for PHP (PHP 8.1+ & constructor promotion)
│   └── autotest-cli/                 # Main CLI binary (`autotest`)
├── adapters/
│   ├── extractors/
│   │   ├── ts-extractor/             # Node.js TypeScript compiler API worker
│   │   └── phpstan-extractor/        # PHP / PHPStan static analysis worker
│   └── hydrators/
│       ├── jest-autotest/            # Runtime hydrator & mock binder for Jest/Vitest (@autotest/jest)
│       └── phpunit-autotest/         # Runtime hydrator & mock binder for PHPUnit
├── conformance/
│   ├── core/                         # 25 reference contract scenarios (01..25)
│   └── profiles/                     # Language-specific edge case conformance profiles (PHP & TS)
└── tests/
    ├── autotest.bridge.test.ts       # TypeScript bridge test suite
    └── AutotestBridgeTest.php        # PHPUnit bridge test suite
```

---

## CLI Usage & Workflow

### 1. Initialize Project
```bash
autotest init
```
Generates `autotest.toml` with detected project ecosystems (PHP and/or TypeScript) and scaffolds `tests/scenarios`.

### 2. Generate Scenarios from Source
```bash
autotest generate
```
Scans `source_roots` (e.g. `src/`), extracts targets and dependencies, solves boundary contracts, and writes or merges scenarios in `tests/scenarios/`.

- `--force`: Overwrites unlocked generated scenarios.
- `--force-all`: Overwrites all scenarios, including locked developer assertions.

### 3. Generate Runner Bridges
```bash
autotest sync-tests
```
Generates native test bridges:
- `tests/autotest.bridge.test.ts` (Vitest / Jest)
- `tests/AutotestBridgeTest.php` (PHPUnit)

### 4. Record Outcomes in OS Sandbox
```bash
autotest record
# or: autotest generate --record
```
Executes the test suite inside an isolated sandbox environment with proxy restrictions and temporary filesystem boundaries. Recorded execution results are safely staged in `*.autotest.staging.json`.

### 5. Verify Scenario Integrity
```bash
autotest verify
```
Verifies scenario integrity in CI/CD pipelines. Exits with non-zero code if unverified staging files or unresolved stale assertions exist.

### 6. Approve Staged Outcomes
```bash
autotest approve
# or approve specific file:
autotest approve --file tests/scenarios/OrderService.autotest.json
```
Merges verified staged recordings back into the primary scenario files (`mode: "recorded"`) and removes the `.staging.json` file.

---

## Configuration (`autotest.toml`)

```toml
[project]
scenario_dir = "tests/scenarios"
hydrator_install = "package_manager" # or "vendor"
deduplicate_fixtures = true
canonical_sort = true

[solver]
max_depth = 8
max_loop_unroll = 2

[sandbox]
mode = "os_native"
timeout_ms = 500
block_network = true
allow_temp_fs_only = true

[languages.typescript]
source_roots = ["src"]
tsconfig = "tsconfig.json"
extractor = "node:ts-compiler-api"
test_framework = "vitest"
bridge_file = "tests/autotest.bridge.test.ts"

[languages.php]
source_roots = ["src"]
extractor = "builtin:mago"
fallback_extractor = "php:phpstan"
test_framework = "phpunit"
bridge_file = "tests/AutotestBridgeTest.php"
```

---

## Development & Testing

Run all Rust test suites (unit, conformance, and CLI integration tests):
```bash
cargo test
```

Run JavaScript adapter tests:
```bash
npm test --prefix adapters/extractors/ts-extractor
npm test --prefix adapters/hydrators/jest-autotest
```

---

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT License](LICENSE-MIT) at your option.
