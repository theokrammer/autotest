pub struct EntrypointScaffolder;

impl EntrypointScaffolder {
    /// Generates PHPUnit test bridge with #[DataProvider]
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
        $dir = __DIR__ . '/../{scenario_dir}';
        if (!is_dir($dir)) {{
            return [];
        }}

        $iterator = new \RecursiveIteratorIterator(
            new \RecursiveDirectoryIterator($dir, \FilesystemIterator::SKIP_DOTS)
        );

        foreach ($iterator as $item) {{
            if ($item->isFile() && str_ends_with($item->getFilename(), '.autotest.json')) {{
                $data = json_decode(file_get_contents($item->getPathname()), true, 512, JSON_THROW_ON_ERROR);
                foreach ($data['targets'] ?? [] as $target) {{
                    foreach ($target['scenarios'] ?? [] as $scenario) {{
                        $name = sprintf('%s :: %s', $target['id'], $scenario['name']);
                        yield $name => [$item->getPathname(), $scenario];
                    }}
                }}
            }}
        }}
    }}
}}
"#,
            scenario_dir = scenario_dir.trim_start_matches('/')
        )
    }

    /// Generates TypeScript / Vitest / Jest test bridge with test.each
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

function collectScenarios(dir: string): Array<[string, any, string]> {{
  const results: Array<[string, any, string]> = [];
  if (!fs.existsSync(dir)) return results;

  function walk(current: string) {{
    for (const entry of fs.readdirSync(current, {{ withFileTypes: true }})) {{
      const full = path.join(current, entry.name);
      if (entry.isDirectory()) {{
        walk(full);
      }} else if (entry.isFile() && entry.name.endsWith('.autotest.json')) {{
        const content = JSON.parse(fs.readFileSync(full, 'utf-8'));
        for (const target of content.targets || []) {{
          for (const scenario of target.scenarios || []) {{
            results.push([`${{target.id}} :: ${{scenario.name}}`, scenario, full]);
          }}
        }}
      }}
    }}
  }}

  walk(dir);
  return results;
}}

const scenarioDir = path.resolve(__dirname, '../{scenario_dir}');
const scenarios = collectScenarios(scenarioDir);

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
