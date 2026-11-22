pub struct EntrypointScaffolder;

impl EntrypointScaffolder {
    /// Generates PHPUnit test bridge with #[DataProvider]
    /// Supports targeted execution via AUTOTEST_FILE and optional AUTOTEST_INDEX env vars.
    pub fn generate_phpunit_bridge(scenario_dir: &str) -> String {
        format!(
            r#"<?php
declare(strict_types=1);

namespace Tests;

use PHPUnit\Framework\TestCase;
use PHPUnit\Framework\Attributes\DataProvider;
use Autotest\Hydrator\AutotestHydrator;

final class AutotestBridgeTest extends TestCase
{{
    #[DataProvider('scenarioProvider')]
    public function testScenario(string $file, array $scenario): void
    {{
        $hydrator = new AutotestHydrator();
        $hydrator->executeScenario($file, $scenario, $this);
    }}

    public static function scenarioProvider(): iterable
    {{
        $envFile = getenv('AUTOTEST_FILE');
        $fileFilter = ($envFile !== false && $envFile !== '') ? $envFile : null;

        $envIndex = getenv('AUTOTEST_INDEX');
        $indexFilter = ($envIndex !== false && $envIndex !== '' && is_numeric($envIndex)) ? (int)$envIndex : null;

        return self::collectScenarios($fileFilter, $indexFilter);
    }}

    /**
     * Collect scenarios from all files or a single targeted file (+ optional 0-based index).
     */
    public static function collectScenarios(?string $filePath = null, ?int $index = null): array
    {{
        $projectRoot = dirname(__DIR__);
        $dir = $projectRoot . '/{scenario_dir}';
        $files = [];

        if ($filePath !== null) {{
            $candidates = [$filePath, $projectRoot . '/' . ltrim($filePath, '/'), $dir . '/' . ltrim($filePath, '/')];
            foreach ($candidates as $candidate) {{
                if (is_file($candidate)) {{
                    $files[] = realpath($candidate) ?: $candidate;
                    break;
                }}
            }}
            if (empty($files)) {{
                throw new \RuntimeException("AUTOTEST_FILE not found: {{$filePath}}");
            }}
        }} else {{
            if (!is_dir($dir)) {{
                return [];
            }}
            $iterator = new \RecursiveIteratorIterator(
                new \RecursiveDirectoryIterator($dir, \FilesystemIterator::SKIP_DOTS)
            );
            foreach ($iterator as $item) {{
                if ($item->isFile() && str_ends_with($item->getFilename(), '.autotest.json') && !str_ends_with($item->getFilename(), '.staging.json')) {{
                    $files[] = $item->getPathname();
                }}
            }}
            sort($files);
        }}

        $entries = [];
        $currentIndex = 0;
        foreach ($files as $path) {{
            $data = json_decode((string)file_get_contents($path), true, 512, JSON_THROW_ON_ERROR);
            foreach ($data['targets'] ?? [] as $target) {{
                foreach ($target['scenarios'] ?? [] as $scenario) {{
                    if ($index === null || $currentIndex === $index) {{
                        $scenario['$target_id'] = $target['id'] ?? null;
                        $name = sprintf('[#%d] %s :: %s', $currentIndex, $target['id'], $scenario['name']);
                        $entries[$name] = [$path, $scenario];
                    }}
                    $currentIndex++;
                }}
            }}
        }}

        return $entries;
    }}
}}
"#,
            scenario_dir = scenario_dir.trim_start_matches('/')
        )
    }

    /// PHPUnit < 10 variant: uses the `@dataProvider` docblock instead of the `#[DataProvider]` attribute.
    pub fn generate_phpunit_bridge_legacy(scenario_dir: &str) -> String {
        Self::generate_phpunit_bridge(scenario_dir).replace(
            "#[DataProvider('scenarioProvider')]",
            "/**\n     * @dataProvider scenarioProvider\n     */",
        )
    }

    /// Plain-JavaScript (CommonJS) Jest bridge for repos without TypeScript/Babel-TS support.
    pub fn generate_jest_js_bridge(scenario_dir: &str) -> String {
        format!(
            r#"const fs = require('node:fs');
const path = require('node:path');
const {{ executeAutotestScenario }} = require('@autotest/jest');

function collectScenarios(dir, fileFilter, indexFilter) {{
  const files = [];
  if (fileFilter) {{
    const projectRoot = path.resolve(__dirname, '..');
    const candidates = [fileFilter, path.resolve(projectRoot, fileFilter), path.resolve(dir, fileFilter)];
    const found = candidates.find(c => fs.existsSync(c) && fs.statSync(c).isFile());
    if (!found) {{
      throw new Error(`AUTOTEST_FILE not found: ${{fileFilter}}`);
    }}
    files.push(found);
  }} else if (fs.existsSync(dir)) {{
    function walk(current) {{
      for (const entry of fs.readdirSync(current, {{ withFileTypes: true }})) {{
        const full = path.join(current, entry.name);
        if (entry.isDirectory()) {{
          walk(full);
        }} else if (entry.isFile() && entry.name.endsWith('.autotest.json') && !entry.name.endsWith('.staging.json')) {{
          files.push(full);
        }}
      }}
    }}
    walk(dir);
    files.sort();
  }}

  const results = [];
  let currentIndex = 0;
  for (const full of files) {{
    const content = JSON.parse(fs.readFileSync(full, 'utf-8'));
    for (const target of content.targets || []) {{
      for (const scenario of target.scenarios || []) {{
        if (indexFilter === undefined || currentIndex === indexFilter) {{
          results.push([`[#${{currentIndex}}] ${{target.id}} :: ${{scenario.name}}`, {{ ...scenario, $target_id: target.id }}, full]);
        }}
        currentIndex++;
      }}
    }}
  }}
  return results;
}}

const scenarioDir = path.resolve(__dirname, '../{scenario_dir}');
const fileFilter = process.env.AUTOTEST_FILE || undefined;
const indexFilter = process.env.AUTOTEST_INDEX !== undefined && process.env.AUTOTEST_INDEX !== ''
  ? Number.parseInt(process.env.AUTOTEST_INDEX, 10)
  : undefined;
const scenarios = collectScenarios(scenarioDir, fileFilter, Number.isNaN(indexFilter) ? undefined : indexFilter);

describe('Autotest Scenarios', () => {{
  it.each(scenarios)('%s', async (_name, scenario, file) => {{
    await executeAutotestScenario(file, scenario, {{
      expect,
      loadModule: (p) => Promise.resolve(require(p)),
    }});
  }});
}});
"#,
            scenario_dir = scenario_dir.trim_start_matches('/')
        )
    }

    /// Generates TypeScript / Vitest / Jest test bridge with test.each
    /// Supports targeted execution via AUTOTEST_FILE and optional AUTOTEST_INDEX env vars.
    pub fn generate_ts_bridge(scenario_dir: &str, is_vitest: bool) -> String {
        let runner_import = if is_vitest {
            "import { describe, it, expect, vi } from 'vitest';"
        } else {
            "// Jest globals\n/* global describe, it, expect */"
        };
        let load_module_expr = if is_vitest {
            "(p: string) => vi.importActual(p)"
        } else {
            "(p: string) => Promise.resolve(require(p))"
        };

        format!(
            r#"{runner_import}
import * as fs from 'node:fs';
import * as path from 'node:path';
import {{ executeAutotestScenario }} from '@autotest/jest';

function collectScenarios(dir: string, fileFilter?: string, indexFilter?: number): Array<[string, any, string]> {{
  const files: string[] = [];
  if (fileFilter) {{
    const projectRoot = path.resolve(__dirname, '..');
    const candidates = [fileFilter, path.resolve(projectRoot, fileFilter), path.resolve(dir, fileFilter)];
    const found = candidates.find(c => fs.existsSync(c) && fs.statSync(c).isFile());
    if (!found) {{
      throw new Error(`AUTOTEST_FILE not found: ${{fileFilter}}`);
    }}
    files.push(found);
  }} else if (fs.existsSync(dir)) {{
    function walk(current: string) {{
      for (const entry of fs.readdirSync(current, {{ withFileTypes: true }})) {{
        const full = path.join(current, entry.name);
        if (entry.isDirectory()) {{
          walk(full);
        }} else if (entry.isFile() && entry.name.endsWith('.autotest.json') && !entry.name.endsWith('.staging.json')) {{
          files.push(full);
        }}
      }}
    }}
    walk(dir);
    files.sort();
  }}

  const results: Array<[string, any, string]> = [];
  let currentIndex = 0;
  for (const full of files) {{
    const content = JSON.parse(fs.readFileSync(full, 'utf-8'));
    for (const target of content.targets || []) {{
      for (const scenario of target.scenarios || []) {{
        if (indexFilter === undefined || currentIndex === indexFilter) {{
          results.push([`[#${{currentIndex}}] ${{target.id}} :: ${{scenario.name}}`, {{ ...scenario, $target_id: target.id }}, full]);
        }}
        currentIndex++;
      }}
    }}
  }}
  return results;
}}

const scenarioDir = path.resolve(__dirname, '../{scenario_dir}');
const fileFilter = process.env.AUTOTEST_FILE || undefined;
const indexFilter = process.env.AUTOTEST_INDEX !== undefined && process.env.AUTOTEST_INDEX !== ''
  ? Number.parseInt(process.env.AUTOTEST_INDEX, 10)
  : undefined;
const scenarios = collectScenarios(scenarioDir, fileFilter, Number.isNaN(indexFilter) ? undefined : indexFilter);

describe('Autotest Scenarios', () => {{
  it.each(scenarios)('%s', async (_name, scenario, file) => {{
    await executeAutotestScenario(file, scenario, {{
      expect,
      loadModule: {load_module_expr},
    }});
  }});
}});
"#,
            runner_import = runner_import,
            load_module_expr = load_module_expr,
            scenario_dir = scenario_dir.trim_start_matches('/')
        )
    }

    /// Generates built-in Node.js test runner (`node --test`) ESM bridge
    /// Supports targeted execution via AUTOTEST_FILE and optional AUTOTEST_INDEX env vars.
    pub fn generate_node_bridge(scenario_dir: &str) -> String {
        format!(
            r#"import {{ describe, it }} from 'node:test';
import * as fs from 'node:fs';
import * as path from 'node:path';
import {{ createRequire }} from 'node:module';
import {{ fileURLToPath, pathToFileURL }} from 'node:url';

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);
const require = createRequire(import.meta.url);

let executeAutotestScenario;
try {{
  ({{ executeAutotestScenario }} = require('@autotest/jest'));
}} catch (_) {{
  const fallback = process.env.AUTOTEST_JS_HYDRATOR || '/tmp/autotest-js-hydrator/index.js';
  ({{ executeAutotestScenario }} = require(fallback));
}}

function collectScenarios(dir, fileFilter, indexFilter) {{
  const files = [];
  if (fileFilter) {{
    const projectRoot = path.resolve(__dirname, '..');
    const candidates = [fileFilter, path.resolve(projectRoot, fileFilter), path.resolve(dir, fileFilter)];
    const found = candidates.find(c => fs.existsSync(c) && fs.statSync(c).isFile());
    if (!found) {{
      throw new Error(`AUTOTEST_FILE not found: ${{fileFilter}}`);
    }}
    files.push(found);
  }} else if (fs.existsSync(dir)) {{
    function walk(current) {{
      for (const entry of fs.readdirSync(current, {{ withFileTypes: true }})) {{
        const full = path.join(current, entry.name);
        if (entry.isDirectory()) {{
          walk(full);
        }} else if (entry.isFile() && entry.name.endsWith('.autotest.json') && !entry.name.endsWith('.staging.json')) {{
          files.push(full);
        }}
      }}
    }}
    walk(dir);
    files.sort();
  }}

  const results = [];
  let currentIndex = 0;
  for (const full of files) {{
    const content = JSON.parse(fs.readFileSync(full, 'utf-8'));
    for (const target of content.targets || []) {{
      for (const scenario of target.scenarios || []) {{
        if (indexFilter === undefined || currentIndex === indexFilter) {{
          results.push([`[#${{currentIndex}}] ${{target.id}} :: ${{scenario.name}}`, {{ ...scenario, $target_id: target.id }}, full]);
        }}
        currentIndex++;
      }}
    }}
  }}
  return results;
}}

const scenarioDir = path.resolve(__dirname, '../{scenario_dir}');
const fileFilter = process.env.AUTOTEST_FILE || undefined;
const indexFilter = process.env.AUTOTEST_INDEX !== undefined && process.env.AUTOTEST_INDEX !== ''
  ? Number.parseInt(process.env.AUTOTEST_INDEX, 10)
  : undefined;
const scenarios = collectScenarios(scenarioDir, fileFilter, Number.isNaN(indexFilter) ? undefined : indexFilter);

describe('Autotest Scenarios', () => {{
  for (const [name, scenario, file] of scenarios) {{
    it(name, async () => {{
      await executeAutotestScenario(file, scenario, {{
        loadModule: (p) => import(pathToFileURL(p).href),
      }});
    }});
  }}
}});
"#,
            scenario_dir = scenario_dir.trim_start_matches('/')
        )
    }
}
