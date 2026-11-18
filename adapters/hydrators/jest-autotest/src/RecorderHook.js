const fs = require('fs');
const path = require('path');

/**
 * RecorderHook: stages recorded outcomes to .autotest.staging.json
 * and merges them back after verification.
 */
class RecorderHook {
  constructor() {
    this.stagingEntries = new Map();
  }

  /**
   * Record an actual outcome for a scenario.
   * @param {string} scenarioFile - Path to the .autotest.json file
   * @param {string} targetId - Target identifier (e.g. "OrderService.charge")
   * @param {string} scenarioName - Scenario name
   * @param {{ result: any, error: any }} outcome
   */
  recordOutcome(scenarioFile, targetId, scenarioName, outcome) {
    const key = scenarioFile;
    if (!this.stagingEntries.has(key)) {
      this.stagingEntries.set(key, []);
    }

    const entry = {
      target: targetId,
      scenario: scenarioName,
      recorded_at: new Date().toISOString(),
      mode: 'recorded',
    };

    if (outcome.error) {
      entry.expected = {
        exception: {
          type: outcome.error.constructor.name,
          message: outcome.error.message,
        },
      };
    } else {
      entry.expected = {
        value: this._serializeValue(outcome.result),
      };
    }

    this.stagingEntries.get(key).push(entry);
  }

  /**
   * Flush all staged entries to .autotest.staging.json files.
   * @returns {string[]} List of staging file paths written
   */
  flush() {
    const written = [];
    for (const [scenarioFile, entries] of this.stagingEntries) {
      const dir = path.dirname(scenarioFile);
      const base = path.basename(scenarioFile, '.autotest.json');
      const stagingPath = path.join(dir, `${base}.autotest.staging.json`);

      let existing = [];
      if (fs.existsSync(stagingPath)) {
        try {
          existing = JSON.parse(fs.readFileSync(stagingPath, 'utf8'));
        } catch (_) {
          existing = [];
        }
      }

      const merged = [...existing, ...entries];
      fs.writeFileSync(stagingPath, JSON.stringify(merged, null, 2), 'utf8');
      written.push(stagingPath);
    }
    this.stagingEntries.clear();
    return written;
  }

  /**
   * Merge verified staging entries back into the main .autotest.json.
   * @param {string} stagingPath - Path to .autotest.staging.json
   * @returns {boolean} Whether merge succeeded
   */
  mergeVerifiedStaging(stagingPath) {
    if (!fs.existsSync(stagingPath)) {
      return false;
    }

    const entries = JSON.parse(fs.readFileSync(stagingPath, 'utf8'));
    const mainPath = stagingPath.replace('.staging.json', '.json');

    if (!fs.existsSync(mainPath)) {
      return false;
    }

    const doc = JSON.parse(fs.readFileSync(mainPath, 'utf8'));

    for (const entry of entries) {
      const target = (doc.targets || []).find(t => t.id === entry.target);
      if (!target) continue;

      const existing = (target.scenarios || []).find(s => s.name === entry.scenario);
      if (existing) {
        existing.expected = entry.expected;
        existing.mode = 'recorded';
        delete existing.stale;
      } else {
        if (!target.scenarios) target.scenarios = [];
        target.scenarios.push({
          name: entry.scenario,
          mode: 'recorded',
          inputs: [],
          expected: entry.expected,
        });
      }
    }

    fs.writeFileSync(mainPath, JSON.stringify(doc, null, 2), 'utf8');
    fs.unlinkSync(stagingPath);
    return true;
  }

  _serializeValue(value) {
    if (value === null || value === undefined) return null;
    if (typeof value === 'function') return null;
    if (value instanceof Date) {
      return { $type: 'Date', value: value.toISOString() };
    }
    if (typeof value === 'object' && value.constructor && value.constructor.name !== 'Object' && !Array.isArray(value)) {
      const obj = { $type: value.constructor.name, $props: {} };
      for (const key of Object.keys(value)) {
        obj.$props[key] = this._serializeValue(value[key]);
      }
      return obj;
    }
    return value;
  }
}

module.exports = { RecorderHook };
