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

    /// Generates TypeScript / Vitest / Jest test bridge with test.each
    /// Supports targeted execution via AUTOTEST_FILE and optional AUTOTEST_INDEX env vars.
    pub fn generate_ts_bridge(scenario_dir: &str, is_vitest: bool) -> String {
        let runner_import = if is_vitest {
            "import { describe, it, expect } from 'vitest';"
        } else {
            "// Jest globals\n/* global describe, it, expect */"
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
          results.push([`[#${{currentIndex}}] ${{target.id}} :: ${{scenario.name}}`, scenario, full]);
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
    await executeAutotestScenario(file, scenario, {{ expect }});
  }});
}});
"#,
            runner_import = runner_import,
            scenario_dir = scenario_dir.trim_start_matches('/')
        )
    }
}
