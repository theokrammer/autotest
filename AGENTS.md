# AGENTS.md — Autotest Project Handbook

**Autotest** is a deterministic, language-agnostic contract testing engine written in Rust (`crates/`) with runtime hydrator/mock adapters (`adapters/hydrators/`) for **PHP** (PHPUnit 10.5 / PHP 8.5) and **TypeScript/JavaScript** (Vitest, Jest, `node --test`).

For step-by-step instructions on adding a new language or adapter, see [LANGUAGE_INTEGRATION.md](LANGUAGE_INTEGRATION.md).

---

## 1. Repository Map

| Path | Purpose |
| --- | --- |
| `crates/autotest-core/src/ir.rs` | Scenario IR v1.0 (`ScenarioDocument`, `TargetDefinition`, `ScenarioDefinition`, `SetupCall`, `FactorySignature`, `MetadataSeed`) |
| `crates/autotest-core/src/project_index.rs` | Cross-file `ProjectIndex` (classes, `implements`, `static_factories`, `mutators`, interface return types, exported string constants) |
| `crates/autotest-core/src/solver.rs` | `SymbolicSolver`: boundary, branch, static factory, higher-order, type-aware mock, metadata-seed, and producer-chain synthesis |
| `crates/autotest-core/src/merger.rs` | `ScenarioMerger`: preserves `locked: true` scenarios and detects `stale: true` signature/mock drift |
| `crates/autotest-core/src/cache.rs` | AST hashes (`sha256:` / `contract_sha256:`) and Tarjan SCC `DependencyGraph` invalidation |
| `crates/autotest-core/src/scaffolder.rs` | `EntrypointScaffolder`: emits PHPUnit (`#[DataProvider]`), Vitest/Jest (`it.each`), and `node --test` bridges |
| `crates/autotest-proto/src/lib.rs` | JSON-RPC 2.0 protocol types (`ping`, `extract_file`) for external extractor workers |
| `crates/autotest-sandbox/` | `SandboxRunner` (Landlock/network isolation + timeouts) and `StagingManager` (`*.autotest.staging.json`) |
| `crates/autotest-php-extractor/src/lib.rs` | Built-in PHP AST extractor (PHP 8.1+ promotion, `match` arms, PHPDoc array types, variadics, private ctors, static factories) |
| `crates/autotest-ts-precheck/src/lib.rs` | Built-in TS/JS extractor (classes, generics, standalone functions, decorators, `Reflect.getMetadata`, `prop:*` shapes) |
| `crates/autotest-cli/src/` | CLI binary (`autotest`): `config.rs`, 2-pass `lib.rs` (`--include`/`--exclude`), `coverage_feedback.rs` (`--coverage-guided`) |
| `adapters/extractors/` | External JSON-RPC 2.0 workers (`ts-extractor/`, `phpstan-extractor/`) |
| `adapters/hydrators/phpunit-autotest/` | 7-module PHP runtime hydrator (`Loader`, `DtoHydrator`, `MockBinder`, `TargetInvoker`, `OutcomeAsserter`, `RecorderHook`, `AutotestHydrator`) |
| `adapters/hydrators/jest-autotest/` | 7-module JS/TS runtime hydrator (`@autotest/jest`) |
| `schema/scenario.v1.schema.json` | JSON Schema (v1.0) |
| `conformance/` | 25 core conformance specs (`conformance/core/01..25`) + PHP/TS profile fixtures (`conformance/profiles/`) |
| `.tmp-repos/` | Gitignored local testbench containing `router` (PHP), `nanoid` (ESM JS), and `nest` (TS monorepo) |

---

## 2. Build, Test & Verification Commands

Run all workspace unit, conformance, and CLI integration tests:
```bash
cargo test
```

Build the release CLI binary (`./target/release/autotest`):
```bash
cargo build --release
```
> **Important:** `crates/autotest-cli/src/lib.rs` embeds both runtime hydrators at compile time via `include_str!`. **Always run `cargo build --release` after editing `adapters/hydrators/`** so `autotest record` and Vitest/standalone PHP runners use the updated hydrator code.

Run runtime adapter test suites:
```bash
php adapters/hydrators/phpunit-autotest/tests/hydrator_test.php
npm --prefix adapters/hydrators/jest-autotest test
npm --prefix adapters/extractors/ts-extractor test
```

---

## 3. Local Testbench (`.tmp-repos/`) & Coverage Baselines

Standard end-to-end regeneration workflow inside any target repo:
```bash
/home/theok/src/autotest/target/release/autotest sync-tests
/home/theok/src/autotest/target/release/autotest generate --force-all --record --coverage-guided
/home/theok/src/autotest/target/release/autotest approve
/home/theok/src/autotest/target/release/autotest verify
```

Targeted debugging of a single file or scenario index:
```bash
AUTOTEST_FILE=tests/scenarios/Route.autotest.json AUTOTEST_INDEX=0 vendor/bin/phpunit tests/AutotestBridgeTest.php
AUTOTEST_FILE=tests/scenarios/index.autotest.json AUTOTEST_INDEX=10 node --test tests/autotest.bridge.test.mjs
```

### Verified Coverage Baselines

| Repo | Setup & Coverage Command | Current Verified Baseline |
| --- | --- | --- |
| `.tmp-repos/router` *(PHP 8.5, PHPUnit 10.5 + PCOV)* | `vendor/autotest/phpunit` → symlink to `adapters/hydrators/phpunit-autotest`<br>`vendor/bin/phpunit --coverage-text tests/AutotestBridgeTest.php` | **222/222 passing** (276 assertions)<br>**62.33% lines** (268/430), **78.41% methods** (69/88), **31.58% classes** (6/19) |
| `.tmp-repos/nanoid` *(ESM JS, `node --test`)* | Uses embedded JS hydrator via `AUTOTEST_JS_HYDRATOR`<br>`node --test --experimental-test-coverage tests/autotest.bridge.test.mjs` | **32/32 passing** (90ms)<br>`index.js`: **97.70% lines / 85.71% branches / 100% funcs**<br>`index.browser.js`, `non-secure/index.js`, `url-alphabet/index.js`: **100% lines / 100% funcs** |
| `.tmp-repos/nest` *(TS monorepo, Vitest 5.1 + V8)* | `node_modules/@autotest/jest` → symlink to `adapters/hydrators/jest-autotest`<br>`./node_modules/.bin/vitest run --config /tmp/autotest.vitest.config.mjs --coverage --coverage.provider=v8 --coverage.reporter=text tests/autotest.bridge.test.ts` | **2,506/2,506 passing** (8.40s)<br>Full monorepo: **37.52% lines / 37.23% stmts / 42.05% funcs / 25.40% branches**<br>Unit-scoped (`packages/{common,core,websockets}`): **49.99% lines / 54.71% funcs / 34.46% branches** |

---

## 4. Architectural Invariants

1. **2-Pass Generation Pipeline** (`crates/autotest-cli/src/lib.rs`):
   - **Pass 1**: Scan all in-scope files, call `ProjectIndex::index_source_metadata` on *every* source file (including interface-only and constants-only files), extract `ScenarioDocument`s, and call `ProjectIndex::index_document`.
   - **Pass 2**: Solve each buffered document via `SymbolicSolver::solve_document_with_index`, deduplicate fixtures, merge with existing files, and write `*.autotest.json`.
   - **Pass 3 & 4**: Optionally run `handle_record_execution` (`--record`) and `coverage_feedback::run_coverage_guided_loop` (`--coverage-guided`, `--max-iterations`).
2. **Backwards-Compatible IR**:
   - All new fields in `crates/autotest-core/src/ir.rs` must use `#[serde(default, skip_serializing_if = ...)]`.
   - When adding struct fields to `ScenarioDefinition`, `SignatureDefinition`, or `TargetDefinition`, update `crates/autotest-core/src/bin/gen_conformance.rs` and `crates/autotest-core/tests/conformance_test.rs`.
3. **Public CLI Wrapper**:
   - Keep `autotest_cli::handle_generate(dir, force, force_all, record, non_interactive)` intact for `crates/autotest-cli/tests/pipeline_test.rs`; delegate to `handle_generate_with_options`.

---

## 5. Critical Pitfalls & Environment Notes

- **PHP Record vs. PHPUnit Replay Parity**:
  - Standalone `run.php` uses `MockBinder::createDynamicMockInterface()`, while `vendor/bin/phpunit` uses `$testCase->createMock()`. Both must return `$this` for fluent `self`/`static` methods and validly initialized objects (via static factories or constructors) for final classes like `MatchingResult`.
  - Never pass `newInstanceWithoutConstructor()` for classes with uninitialized `readonly` properties without invoking a constructor or factory.
  - `MockBinder` must handle `BackedEnum` / `UnitEnum` with a real PHP `enum` stub (PHP forbids `class ... implements BackedEnum`).
  - `RecorderHook::detectMockInterface` only treats `AutotestMock_` and `MockObject_` as type-only mocks (do **not** match `@anonymous`, which breaks anonymous-class tests).
  - `RecorderHook` + `OutcomeAsserter` use `ClosureMarker` to compare objects containing `Closure` properties across record and replay.
- **PHP Extractor**:
  - Strip leading `...` on variadics and tag with `"variadic"`.
  - In `resolve_php_type`, only expand types present in `use_map` (do not prefix un-imported short names with the namespace; `ProjectIndex::short_name_map` and `AutotestHydrator.php` resolve them).
- **JS/TS Hydrator**:
  - `MockBinder._createRecursiveProxy` must return `undefined` for `then`, `catch`, `finally`, `$$typeof`, `asymmetricMatch`, `toJSON`, and inspection symbols so `await mock` never hangs; keep `recursive: false` by default unless `{ recursive: true }` is passed by `AutotestHydrator`.
  - `TargetInvoker._awaitWithTimeout(promise, 250)` prevents unresolving stub callback promises (e.g. `IoAdapter.close`) from hanging Vitest for 5s.
  - `RecorderHook._isOpaqueRuntimeObject` treats random-hex mixin class names (`/^[0-9a-f]{12,}$/i`) and objects with nested function properties as opaque `'object'` (type-only).
- **Symbolic Solver & TS Extractor**:
  - In `synthesize_value_for_type`, resolve primitives (`string`, `int`, `float`, `bool`) *before* checking `depth > 3`, and check array/collection types (`callable[]`) *before* `Callback`.
  - In `extract_branch_constraints`, skip method calls (`param.foo(`) when extracting `prop:*` object-shape constraints.
- **Sandbox Execution**:
  - `.git` is read-only inside the default command sandbox; `git add` / `git commit` require `BypassSandbox: true`. All `cargo`, `php`, `node`, and `vitest` commands run inside the sandbox.
