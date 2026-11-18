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
  createMock(mockSpecs, classRegistry = {}) {
    const mock = {};
    const verifiers = [];

    for (const spec of mockSpecs) {
      const target = spec.target || '';
      const parts = target.split('.');
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

        // Handle value returns
        const value = retSpec && retSpec.value !== undefined ? retSpec.value : retSpec;
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

    return {
      mock,
      verify: () => {
        for (const v of verifiers) {
          v();
        }
      }
    };
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
