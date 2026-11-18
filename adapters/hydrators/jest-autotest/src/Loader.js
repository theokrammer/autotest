const fs = require('fs');
const path = require('path');

class Loader {
  loadScenarioFile(filePath) {
    const fullPath = path.resolve(filePath);
    if (!fs.existsSync(fullPath)) {
      throw new Error(`Scenario file not found: ${fullPath}`);
    }
    const content = fs.readFileSync(fullPath, 'utf8');
    const doc = JSON.parse(content);
    return this.resolveFixtures(doc);
  }

  resolveFixtures(doc) {
    const fixtures = doc.fixtures || {};

    const resolveValue = (val) => {
      if (val === null || val === undefined) return val;
      if (typeof val === 'object') {
        if (Array.isArray(val)) {
          return val.map(resolveValue);
        }
        if (typeof val['$ref'] === 'string' && val['$ref'].startsWith('#/fixtures/')) {
          const key = val['$ref'].replace('#/fixtures/', '');
          if (fixtures[key] !== undefined) {
            return resolveValue(fixtures[key]);
          }
          throw new Error(`Unresolved fixture reference: ${val['$ref']}`);
        }
        const resolvedObj = {};
        for (const [k, v] of Object.entries(val)) {
          resolvedObj[k] = resolveValue(v);
        }
        return resolvedObj;
      }
      return val;
    };

    return resolveValue(doc);
  }
}

module.exports = { Loader };
