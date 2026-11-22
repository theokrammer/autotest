const fs = require('fs');
const path = require('path');
const { pathToFileURL } = require('url');
const { Loader } = require('./Loader');
const { DtoHydrator } = require('./DtoHydrator');
const { MockBinder } = require('./MockBinder');
const { TargetInvoker } = require('./TargetInvoker');
const { OutcomeAsserter } = require('./OutcomeAsserter');
const { RecorderHook } = require('./RecorderHook');

/**
 * AutotestHydrator: main entry-point for @autotest/jest.
 *
 * Usage in a test file (Jest or Vitest):
 *
 *   const { createAutotestSuite } = require('@autotest/jest');
 *   const { OrderService } = require('../src/OrderService');
 *
 *   createAutotestSuite('src/OrderService.autotest.json', {
 *     OrderService,
 *     PaymentGateway: class PaymentGateway {},
 *   });
 */
class AutotestHydrator {
  constructor() {
    this.loader = new Loader();
    this.dtoHydrator = new DtoHydrator();
    this.mockBinder = new MockBinder(this.dtoHydrator);
    this.targetInvoker = new TargetInvoker(this.dtoHydrator);
    this.outcomeAsserter = new OutcomeAsserter(this.dtoHydrator);
    this.recorderHook = new RecorderHook();
  }

  /**
   * Execute a single scenario.
   * @param {string} scenarioFilePath
   * @param {object} targetDef - { id, kind, signature }
   * @param {object} scenarioSpec - { name, inputs, mocks, expected }
   * @param {object} classRegistry
   * @param {function} expect - Jest/Vitest expect
   * @param {{ record?: boolean }} options
   */
  async executeScenario(scenarioFilePath, targetDef, scenarioSpec, classRegistry, expect, options = {}) {
    const mocks = scenarioSpec.mocks || [];
    const ctorArgs = targetDef.signature.constructor_args || [];

    // Build mock instances keyed by constructor parameter name
    const mockInstances = {};
    const primitiveTypes = new Set(['string', 'number', 'boolean', 'any', 'unknown', 'object', 'void', 'null', 'undefined']);
    for (const param of ctorArgs) {
      const matchingMocks = mocks.filter(m => {
        const target = m.target || '';
        return target.startsWith(param.name + '.');
      });

      const paramType = (param.type || param.param_type || '').trim();
      const needsAutoMock = matchingMocks.length === 0
        && (param.default_value === null || param.default_value === undefined)
        && paramType !== ''
        && !primitiveTypes.has(paramType.toLowerCase());

      if (matchingMocks.length > 0 || needsAutoMock) {
        const { mock, verify } = this.mockBinder.createMock(matchingMocks, classRegistry);
        mockInstances[param.name] = mock;
        // Store verifier for post-invocation
        mockInstances[`__verify_${param.name}`] = verify;
      }
    }

    // Invoke target
    const outcome = await this.targetInvoker.invoke(targetDef, scenarioSpec, mockInstances, classRegistry);

    // Record mode: stage the outcome and skip placeholder assertions
    if (options.record) {
      const effects = targetDef.detected_side_effects || [];
      const nonDeterministic = effects.some(
        e => e.type === 'rng' || e.effect_type === 'rng' || e.type === 'clock_read' || e.effect_type === 'clock_read'
      ) || (Array.isArray(scenarioSpec.invoke_returned_args) && /random|alphabet|nanoid/i.test(targetDef.id || ''));
      this.recorderHook.recordOutcome(scenarioFilePath, targetDef.id, scenarioSpec.name, outcome, { nonDeterministic });
      return;
    }

    // Assert expected
    if (scenarioSpec.expected) {
      this.outcomeAsserter.assert(outcome, scenarioSpec.expected, classRegistry, expect);
    }

    // Verify mock call counts
    for (const param of ctorArgs) {
      const verifier = mockInstances[`__verify_${param.name}`];
      if (verifier) {
        verifier();
      }
    }
  }

  /**
   * Flush recorded staging data.
   * @returns {string[]} Staging files written
   */
  flushRecordings() {
    return this.recorderHook.flush();
  }
}

/**
 * Create a test.each-style suite from an autotest scenario file.
 * Compatible with both Jest and Vitest.
 *
 * @param {string} scenarioFilePath - Path to .autotest.json
 * @param {object} classRegistry - Map of class names to constructors
 * @param {object} options - { record?: boolean }
 */
function createAutotestSuite(scenarioFilePath, classRegistry, options = {}) {
  const hydrator = new AutotestHydrator();
  const doc = hydrator.loader.loadScenarioFile(scenarioFilePath);

  // Build test.each data
  const testCases = [];
  for (const target of doc.targets || []) {
    for (const scenario of target.scenarios || []) {
      testCases.push({
        name: `${target.id} :: ${scenario.name}`,
        targetDef: target,
        scenarioSpec: scenario,
      });
    }
  }

  if (testCases.length === 0) {
    return;
  }

  // Use global `describe` and `test` (or `it`) from Jest/Vitest
  const describeFn = typeof describe === 'function' ? describe : (name, fn) => fn();
  const testFn = typeof test === 'function' ? test : typeof it === 'function' ? it : (name, fn) => fn();
  const expectFn = typeof expect === 'function' ? expect : null;

  describeFn(`autotest: ${scenarioFilePath}`, () => {
    for (const tc of testCases) {
      testFn(tc.name, async () => {
        await hydrator.executeScenario(
          scenarioFilePath,
          tc.targetDef,
          tc.scenarioSpec,
          classRegistry,
          expectFn,
          options
        );
      });
    }

    if (options.record) {
      // After all tests, flush recordings
      const afterFn = typeof afterAll === 'function' ? afterAll : (fn) => fn();
      afterFn(() => {
        hydrator.flushRecordings();
      });
    }
  });
}

const docCache = new Map();
const moduleCache = new Map();

/**
 * Execute a single scenario from a scenario file and specification object.
 * Designed for programmatic invocation and test.each runner bridges (Vitest/Jest).
 *
 * @param {string} scenarioFilePath - Path to .autotest.json
 * @param {object} scenarioSpec - The scenario definition object (name, inputs, expected, etc.)
 * @param {object} [options] - Optional settings: { expect, classRegistry, record, loadModule, targetId }
 */
async function executeAutotestScenario(scenarioFilePath, scenarioSpec, options = {}) {
  const hydrator = new AutotestHydrator();
  const resolvedPath = path.resolve(scenarioFilePath);
  let doc = docCache.get(resolvedPath);
  if (!doc) {
    doc = hydrator.loader.loadScenarioFile(resolvedPath);
    docCache.set(resolvedPath, doc);
  }

  // Find matching target definition for this scenario
  const requestedTargetId = options.targetId || scenarioSpec.$target_id || null;
  let matchedTarget = null;
  for (const target of doc.targets || []) {
    if (requestedTargetId && target.id !== requestedTargetId) {
      continue;
    }
    for (const sc of target.scenarios || []) {
      if (sc.name === scenarioSpec.name) {
        matchedTarget = target;
        break;
      }
    }
    if (matchedTarget) break;
  }

  if (!matchedTarget && doc.targets && doc.targets.length === 1) {
    matchedTarget = doc.targets[0];
  }

  if (!matchedTarget) {
    throw new Error(`Target not found for scenario "${scenarioSpec.name}" in ${scenarioFilePath}`);
  }

  const classRegistry = Object.assign({}, options.classRegistry || {});
  const parts = matchedTarget.id.split('.');
  const className = parts[0];

  if (!classRegistry[className]) {
    if (typeof globalThis !== 'undefined' && globalThis[className]) {
      classRegistry[className] = globalThis[className];
    } else if (doc.source && doc.source.file) {
      const candidates = Array.from(new Set([
        path.resolve(process.cwd(), doc.source.file),
        path.resolve(path.dirname(resolvedPath), doc.source.file),
        path.resolve(process.cwd(), doc.source.file.replace(/\.tsx?$/, '.js')),
      ]));
      let lastLoadError = null;
      for (const candidate of candidates) {
        if (!fs.existsSync(candidate)) {
          continue;
        }
        try {
          let mod = moduleCache.get(candidate);
          if (!mod) {
            if (typeof options.loadModule === 'function') {
              mod = await options.loadModule(candidate);
            } else {
              try {
                mod = require(candidate);
              } catch (reqErr) {
                mod = await import(pathToFileURL(candidate).href);
              }
            }
            if (mod) {
              moduleCache.set(candidate, mod);
            }
          }
          if (!mod) continue;
          if (mod[className]) {
            classRegistry[className] = mod[className];
            break;
          } else if (typeof mod === 'function' && (mod.name === className || parts.length === 1)) {
            classRegistry[className] = mod;
            break;
          } else if (mod.default && (mod.default.name === className || typeof mod.default === 'function')) {
            classRegistry[className] = mod.default;
            break;
          }
        } catch (err) {
          lastLoadError = err;
        }
      }
      if (!classRegistry[className] && lastLoadError) {
        throw new Error(
          `Failed to load target "${className}" from ${doc.source.file}: ${lastLoadError.message}`
        );
      }
    }
  }

  const expectFn = options.expect || (typeof expect === 'function' ? expect : null);
  const record = options.record !== undefined ? options.record : Boolean(process.env.AUTOTEST_RECORD);

  await hydrator.executeScenario(
    resolvedPath,
    matchedTarget,
    scenarioSpec,
    classRegistry,
    expectFn,
    { record }
  );

  if (record) {
    hydrator.flushRecordings();
  }
}

module.exports = {
  AutotestHydrator,
  createAutotestSuite,
  executeAutotestScenario,
  Loader,
  DtoHydrator,
  MockBinder,
  TargetInvoker,
  OutcomeAsserter,
  RecorderHook,
};
