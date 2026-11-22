const fs = require('fs');
const path = require('path');
const { types: utilTypes } = require('util');

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
  recordOutcome(scenarioFile, targetId, scenarioName, outcome, options = {}) {
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
        outcome: 'exception',
        exception_type: outcome.error.constructor ? outcome.error.constructor.name : 'Error',
        message_contains: outcome.error.message || '',
      };
    } else if (typeof outcome.result === 'function') {
      entry.expected = {
        outcome: 'return',
        type: 'function',
      };
    } else if (options.nonDeterministic) {
      const res = outcome.result;
      const resType = (res !== null && res !== undefined && typeof res === 'object' && res.constructor)
        ? res.constructor.name
        : typeof res;
      entry.expected = {
        outcome: 'return',
        type: resType,
      };
    } else if (this._isOpaqueRuntimeObject(outcome.result)) {
      const ctorName = outcome.result.constructor && outcome.result.constructor.name;
      const safeType = (!ctorName || /^[0-9a-f]{12,}$/i.test(ctorName)) ? 'object' : ctorName;
      entry.expected = {
        outcome: 'return',
        type: safeType,
      };
    } else {
      entry.expected = {
        outcome: 'return',
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
          const parsed = JSON.parse(fs.readFileSync(stagingPath, 'utf8'));
          if (Array.isArray(parsed)) {
            existing = parsed;
          }
        } catch (_) {
          existing = [];
        }
      }

      const merged = [...existing];
      for (const entry of entries) {
        const idx = merged.findIndex(e => e.target === entry.target && e.scenario === entry.scenario);
        if (idx >= 0) {
          merged[idx] = entry;
        } else {
          merged.push(entry);
        }
      }
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

  _isOpaqueRuntimeObject(value) {
    if (
      !value
      || typeof value !== 'object'
      || Array.isArray(value)
      || value instanceof Date
      || Buffer.isBuffer(value)
      || ArrayBuffer.isView(value)
      || value instanceof Error
    ) {
      return false;
    }
    if (utilTypes.isProxy(value)) {
      return true;
    }
    const ctorName = value.constructor && value.constructor.name;
    if (ctorName && /^[0-9a-f]{12,}$/i.test(ctorName)) {
      return true;
    }
    if (ctorName === 'Object' || value.constructor === Map || value.constructor === Set) {
      return false;
    }
    // Subclasses of Map/Set (e.g. NestJS ModulesContainer extends Map) are opaque runtime containers
    if (value instanceof Map || value instanceof Set) {
      return true;
    }
    if (Object.getOwnPropertySymbols(value).length > 0) {
      return true;
    }
    const keys = Object.keys(value);
    if (keys.length > 12) {
      return true;
    }
    for (const key of keys) {
      const prop = value[key];
      if (
        typeof prop === 'function'
        || prop === value
        || (prop instanceof Map && prop.constructor !== Map)
        || (prop instanceof Set && prop.constructor !== Set)
      ) {
        return true;
      }
      if (prop && typeof prop === 'object' && !Array.isArray(prop)) {
        for (const subKey of Object.keys(prop)) {
          if (typeof prop[subKey] === 'function') {
            return true;
          }
        }
      }
    }
    return false;
  }

  _serializeValue(value, depth = 0, seen = new WeakSet()) {
    if (value === undefined) return { $type: 'Undefined' };
    if (value === null) return null;
    if (typeof value === 'function') return { $type: 'Function' };
    if (typeof value === 'symbol') return { $type: 'Symbol', description: value.description || '' };
    if (typeof value === 'bigint') return value.toString();
    if (typeof value === 'number' && !Number.isFinite(value)) {
      return { $type: 'Number', value: String(value) };
    }
    if (typeof value !== 'object') return value;
    if (depth > 8 || seen.has(value)) return null;

    if (value instanceof Date) {
      return { $type: 'Date', value: value.toISOString() };
    }
    if (Buffer.isBuffer(value)) {
      return { $type: 'Buffer', data: Array.from(value) };
    }
    if (ArrayBuffer.isView(value)) {
      return { $type: value.constructor.name, data: Array.from(value) };
    }

    seen.add(value);
    try {
      if (value.constructor === Map) {
        return {
          $type: 'Map',
          entries: Array.from(value.entries()).map(([k, v]) => [
            this._serializeValue(k, depth + 1, seen),
            this._serializeValue(v, depth + 1, seen),
          ]),
        };
      }

      if (value.constructor === Set) {
        return {
          $type: 'Set',
          values: Array.from(value.values()).map(v => this._serializeValue(v, depth + 1, seen)),
        };
      }

      if (Array.isArray(value)) {
        return value.map(item => this._serializeValue(item, depth + 1, seen));
      }

      if (value instanceof Error) {
        const props = { message: value.message, name: value.name };
        for (const key of Object.keys(value)) {
          props[key] = this._serializeValue(value[key], depth + 1, seen);
        }
        return { $type: value.constructor ? value.constructor.name : 'Error', $error: true, $props: props };
      }

      if (value.constructor && value.constructor.name !== 'Object') {
        const obj = { $type: value.constructor.name, $props: {} };
        for (const key of Object.keys(value)) {
          obj.$props[key] = this._serializeValue(value[key], depth + 1, seen);
        }
        return obj;
      }

      const plain = {};
      for (const key of Object.keys(value)) {
        plain[key] = this._serializeValue(value[key], depth + 1, seen);
      }
      return plain;
    } finally {
      seen.delete(value);
    }
  }
}

module.exports = { RecorderHook };
