/**
 * OutcomeAsserter: validates actual results against expected outcomes.
 */
class OutcomeAsserter {
  constructor(dtoHydrator) {
    this.dtoHydrator = dtoHydrator;
  }

  /**
   * @param {{ result: any, error: any }} outcome
   * @param {object} expected - { value?, exception?, properties?, type? }
   * @param {object} classRegistry
   * @param {function} expect - Jest/Vitest expect function
   */
  assert(outcome, expected, classRegistry, expect) {
    // Expect an exception
    if (expected.exception) {
      if (!outcome.error) {
        throw new Error(
          `Expected exception ${expected.exception.type || 'Error'} but no error was thrown`
        );
      }
      const error = outcome.error;

      if (expected.exception.type) {
        const ExpectedClass = classRegistry[expected.exception.type] || Error;
        if (expect) {
          expect(error).toBeInstanceOf(ExpectedClass);
        } else if (!(error instanceof ExpectedClass)) {
          throw new Error(
            `Expected error type ${expected.exception.type}, got ${error.constructor.name}`
          );
        }
      }
      if (expected.exception.message) {
        if (expect) {
          expect(error.message).toContain(expected.exception.message);
        } else if (!error.message.includes(expected.exception.message)) {
          throw new Error(
            `Expected error message containing "${expected.exception.message}", got "${error.message}"`
          );
        }
      }
      return;
    }

    // No exception expected but one was thrown
    if (outcome.error) {
      throw outcome.error;
    }

    const actual = outcome.result;

    // Check type
    if (expected.type) {
      const actualType = typeof actual;
      if (expected.type === 'null') {
        this._assert(actual === null || actual === undefined, `Expected null, got ${actual}`);
      } else if (expect) {
        expect(actualType).toBe(expected.type);
      } else {
        this._assert(actualType === expected.type, `Expected type ${expected.type}, got ${actualType}`);
      }
    }

    // Check exact value
    if (expected.value !== undefined) {
      const expectedValue = this.dtoHydrator.hydrate(expected.value, classRegistry);
      if (expect) {
        expect(actual).toEqual(expectedValue);
      } else {
        this._assertDeepEqual(actual, expectedValue);
      }
    }

    // Check properties
    if (expected.properties) {
      for (const [key, expectedPropVal] of Object.entries(expected.properties)) {
        const hydratedExpected = this.dtoHydrator.hydrate(expectedPropVal, classRegistry);
        const actualVal = actual[key];
        if (expect) {
          expect(actualVal).toEqual(hydratedExpected);
        } else {
          this._assertDeepEqual(actualVal, hydratedExpected,
            `Property "${key}": expected ${JSON.stringify(hydratedExpected)}, got ${JSON.stringify(actualVal)}`
          );
        }
      }
    }

    // Check truthy/falsy
    if (expected.truthy === true) {
      this._assert(!!actual, `Expected truthy value, got ${actual}`);
    }
    if (expected.falsy === true) {
      this._assert(!actual, `Expected falsy value, got ${actual}`);
    }
  }

  _assert(condition, message) {
    if (!condition) {
      throw new Error(`Assertion failed: ${message}`);
    }
  }

  _assertDeepEqual(actual, expected, message) {
    const a = JSON.stringify(actual);
    const b = JSON.stringify(expected);
    if (a !== b) {
      throw new Error(
        message || `Expected ${b}, got ${a}`
      );
    }
  }
}

module.exports = { OutcomeAsserter };
