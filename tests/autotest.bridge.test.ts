import { describe, it, expect } from 'vitest';
import * as fs from 'node:fs';
import * as path from 'node:path';
import { executeAutotestScenario } from '@autotest/jest';

function collectScenarios(dir: string): Array<[string, any, string]> {
  const results: Array<[string, any, string]> = [];
  if (!fs.existsSync(dir)) return results;

  function walk(current: string) {
    for (const entry of fs.readdirSync(current, { withFileTypes: true })) {
      const full = path.join(current, entry.name);
      if (entry.isDirectory()) {
        walk(full);
      } else if (entry.isFile() && entry.name.endsWith('.autotest.json')) {
        const content = JSON.parse(fs.readFileSync(full, 'utf-8'));
        for (const target of content.targets || []) {
          for (const scenario of target.scenarios || []) {
            results.push([`${target.id} :: ${scenario.name}`, scenario, full]);
          }
        }
      }
    }
  }

  walk(dir);
  return results;
}

const scenarioDir = path.resolve(__dirname, '../tests/scenarios');
const scenarios = collectScenarios(scenarioDir);

describe('Autotest Scenarios', () => {
  it.each(scenarios)('%s', async (_name, scenario, file) => {
    await executeAutotestScenario(file, scenario, { expect });
  });
});
