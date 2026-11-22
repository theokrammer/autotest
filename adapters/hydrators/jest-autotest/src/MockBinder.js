/**
 * MockBinder: creates mocks for vi.fn() (Vitest) or jest.fn() (Jest),
 * wiring argument matchers, sequential returns, and call count expectations.
 */
class MockBinder {
  constructor(dtoHydrator) {
    this.dtoHydrator = dtoHydrator;
  }

  /**
   * Build a mock object for a dependency interface/class.
   * @param {object[]} mockSpecs - Array of { target, when_args, returns, expect_calls }
   * @param {object} classRegistry - Map of type names to constructors for hydration
   * @returns {{ mock: object, verify: () => void }}
   */
  createMock(mockSpecs, classRegistry = {}, options = {}) {
    const mock = {};
    const verifiers = [];

    for (const spec of mockSpecs) {
      const target = spec.target || '';
      const parts = target.includes('::') ? target.split('::') : target.split('.');
      const methodName = parts[parts.length - 1];

      const whenArgs = spec.when_args || [];
      const returns = spec.returns || [];
      const expectCalls = spec.expect_calls !== undefined ? spec.expect_calls : null;

      let callIndex = 0;
      const callLog = [];

      // Create the mock function - works with both jest.fn() and vi.fn()
      const mockFn = (...actualArgs) => {
        callLog.push(actualArgs);

        // Verify when_args matchers
        for (let i = 0; i < whenArgs.length; i++) {
          if (actualArgs[i] !== undefined) {
            this._assertArgMatch(whenArgs[i], actualArgs[i]);
          }
        }

        // Apply mutate_args if configured
        if (spec.mutate_args) {
          for (const [idxStr, mutation] of Object.entries(spec.mutate_args)) {
            const idx = parseInt(idxStr, 10);
            if (actualArgs[idx] && typeof actualArgs[idx] === 'object' && typeof mutation === 'object') {
              Object.assign(actualArgs[idx], mutation);
            }
          }
        }

        if (returns.length === 0) {
          return undefined;
        }

        const retSpec = callIndex < returns.length ? returns[callIndex] : returns[returns.length - 1];
        callIndex++;

        // Handle exception returns
        if (retSpec && retSpec.exception) {
          const ErrorClass = classRegistry[retSpec.exception.type] || Error;
          throw new ErrorClass(retSpec.exception.message || '');
        }

        // Handle promise rejection
        if (retSpec && retSpec.reject) {
          const ErrorClass = classRegistry[retSpec.reject.type] || Error;
          return Promise.reject(new ErrorClass(retSpec.reject.message || ''));
        }

        // Handle value returns (supports both schema v1 'return' and legacy 'value')
        let value;
        if (retSpec && retSpec.return !== undefined) {
          value = retSpec.return;
        } else if (retSpec && retSpec.value !== undefined) {
          value = retSpec.value;
        } else {
          value = retSpec;
        }
        return this.dtoHydrator.hydrate(value, classRegistry);
      };

      // Attach metadata for inspection
      mockFn._autotestCallLog = callLog;
      mockFn._autotestSpec = spec;

      mock[methodName] = mockFn;

      // Build verifier for call count
      if (expectCalls !== null) {
        verifiers.push(() => {
          const actual = callLog.length;
          if (actual !== expectCalls) {
            throw new Error(
              `Mock ${target}: expected ${expectCalls} call(s) but got ${actual}`
            );
          }
        });
      }
    }

    const finalMock = options.recursive ? this._createRecursiveProxy(mock, 0, 4) : mock;

    return {
      mock: finalMock,
      verify: () => {
        for (const v of verifiers) {
          v();
        }
      }
    };
  }

  _createRecursiveProxy(baseTarget = {}, depth = 0, maxDepth = 4) {
    const stubCache = new Map();
    const ignoredKeys = new Set([
      'then', 'catch', 'finally', '$$typeof', 'asymmetricMatch', 'nodeType',
      'toJSON', '@@__IMMUTABLE_ITERABLE__@@', '@@__IMMUTABLE_RECORD__@@',
      'constructor', 'prototype', '__esModule', 'default', 'inspect', 'valueOf',
      'length', 'size', 'toString', 'Symbol(nodejs.util.inspect.custom)'
    ]);

    const proxy = new Proxy(baseTarget, {
      get: (target, prop, receiver) => {
        if (Reflect.has(target, prop)) {
          return Reflect.get(target, prop, receiver);
        }
        if (prop === Symbol.iterator) {
          return function* () {};
        }
        if (typeof prop === 'symbol' || ignoredKeys.has(String(prop))) {
          return undefined;
        }
        const key = String(prop);
        if (stubCache.has(key)) {
          return stubCache.get(key);
        }

        const fn = (..._args) => {
          const kl = key.toLowerCase();
          if (key === 'values' || key === 'keys' || key === 'entries') {
            return [][Symbol.iterator]();
          }
          if (key === 'forEach') {
            return undefined;
          }
          if (key === 'has') {
            return false;
          }
          if (key === 'set' || key === 'add') {
            return proxy;
          }
          if (/^(set|add|with|use|register|pipe|on|once|off|emit|subscribe|unsubscribe|append|prepend|push|configure|enable|disable)[A-Z_]?/.test(key)) {
            return proxy;
          }
          if (/^(is|has|can|should|supports|contains|includes|exists)[A-Z]/.test(key)) {
            return false;
          }
          if (/^(getModules|getProviders|getControllers|getExports|getImports|getMiddlewares|getRoutes|getNonAliasProviders|getInjectables)$/.test(key)) {
            return new Map();
          }
          if (/^(getAll|getGlobal|getMetadata|scan|list|find|extract)/.test(key)
            || (kl.endsWith('s') && !['options', 'status', 'address', 'class', 'prefix', 'alias'].some(s => kl.endsWith(s)))) {
            return [];
          }
          if (/^(count|getCount|getLength|getStatus|getPort)$/.test(key)) {
            return 0;
          }
          if (/^(getPrefix|getGlobalPrefix|getName|getPath|getMethod|getType|getKey|getId|getToken|getVersion|getHost)$/.test(key)) {
            return '';
          }
          if (depth < maxDepth) {
            return this._createRecursiveProxy({}, depth + 1, maxDepth);
          }
          return {};
        };
        stubCache.set(key, fn);
        return fn;
      },
    });
    return proxy;
  }

  _assertArgMatch(matcher, actual) {
    if (matcher === null || matcher === undefined) return;

    if (typeof matcher === 'object' && matcher['$matcher']) {
      const type = matcher['$matcher'];
      switch (type) {
        case 'any':
          return;
        case 'type':
          if (typeof actual !== matcher.expected) {
            throw new Error(
              `Argument type mismatch: expected ${matcher.expected}, got ${typeof actual}`
            );
          }
          return;
        case 'gt':
          if (!(actual > matcher.value)) {
            throw new Error(`Expected > ${matcher.value}, got ${actual}`);
          }
          return;
        case 'gte':
          if (!(actual >= matcher.value)) {
            throw new Error(`Expected >= ${matcher.value}, got ${actual}`);
          }
          return;
        case 'lt':
          if (!(actual < matcher.value)) {
            throw new Error(`Expected < ${matcher.value}, got ${actual}`);
          }
          return;
        case 'regex':
          if (!new RegExp(matcher.pattern).test(actual)) {
            throw new Error(`Expected to match /${matcher.pattern}/, got ${actual}`);
          }
          return;
        case 'exact':
          if (actual !== matcher.value) {
            throw new Error(`Exact match failed: expected ${JSON.stringify(matcher.value)}, got ${JSON.stringify(actual)}`);
          }
          return;
        case 'range':
          const min = matcher.min !== undefined ? matcher.min : -Infinity;
          const max = matcher.max !== undefined ? matcher.max : Infinity;
          if (actual < min || actual > max) {
            throw new Error(`Range mismatch: ${actual} not in [${min}, ${max}]`);
          }
          return;
        case 'subset':
          if (!actual || typeof actual !== 'object') {
            throw new Error(`Subset mismatch: actual is not an object`);
          }
          for (const [k, v] of Object.entries(matcher.subset || {})) {
            if (!(k in actual) || actual[k] !== v) {
              throw new Error(`Subset mismatch on key "${k}": expected ${JSON.stringify(v)}, got ${JSON.stringify(actual[k])}`);
            }
          }
          return;
        default:
          throw new Error(`Unknown matcher type: ${type}`);
      }
    }

    // Exact match
    if (actual !== matcher) {
      throw new Error(`Argument mismatch: expected ${JSON.stringify(matcher)}, got ${JSON.stringify(actual)}`);
    }
  }
}

module.exports = { MockBinder };
