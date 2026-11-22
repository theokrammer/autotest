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
    const isExceptionOutcome = expected.outcome === 'exception' || Boolean(expected.exception);

    // Expect an exception
    if (isExceptionOutcome) {
      const expectedType = expected.exception_type || (expected.exception && expected.exception.type);
      const expectedMsg = expected.message_contains || (expected.exception && expected.exception.message);

      if (!outcome.error) {
        throw new Error(
          `Expected exception ${expectedType || 'Error'} but no error was thrown`
        );
      }
      const error = outcome.error;

      if (expectedType) {
        const ExpectedClass = classRegistry[expectedType] || (expectedType === 'Object' ? Object : Error);
        if (expect && error instanceof Error && ExpectedClass !== Object) {
          expect(error).toBeInstanceOf(ExpectedClass);
        } else if (expect) {
          expect(error && error.constructor ? error.constructor.name : typeof error).toBe(expectedType);
        } else if (!(error instanceof ExpectedClass) && error.name !== expectedType && error.constructor.name !== expectedType) {
          throw new Error(
            `Expected error type ${expectedType}, got ${error.constructor.name}`
          );
        }
      }
      if (expectedMsg && !/cov_[0-9a-z]+\(/i.test(error.message || '')) {
        if (expect) {
          expect(error.message).toContain(expectedMsg);
        } else if (!error.message.includes(expectedMsg)) {
          throw new Error(
            `Expected error message containing "${expectedMsg}", got "${error.message}"`
          );
        }
      }
      return;
    }

    // No crash expected
    if (expected.outcome === 'no_crash') {
      if (outcome.error) {
        throw new Error(`Expected no_crash, but caught error: ${outcome.error.message || outcome.error}`);
      }
      return;
    }

    // No exception expected but one was thrown
    if (outcome.error) {
      throw outcome.error;
    }

    const actual = outcome.result;

    // Check type (only for unambiguous JS runtime types; TS-only types like void, Promise<T>, unions, interfaces are not typeof values)
    if (expected.type) {
      const normalizedType = expected.type.trim().replace(/^Promise<(.+)>$/, '$1').trim();
      const actualType = typeof actual;
      if (normalizedType === 'null' || normalizedType === 'void' || normalizedType === 'undefined') {
        this._assert(actual === null || actual === undefined, `Expected ${normalizedType}, got ${actual}`);
      } else if (normalizedType === 'object') {
        this._assert(actual !== null && typeof actual === 'object', `Expected object, got ${actualType}`);
      } else if (['string', 'number', 'boolean', 'bigint', 'symbol', 'function'].includes(normalizedType)) {
        if (expect) {
          expect(actualType).toBe(normalizedType);
        } else {
          this._assert(actualType === normalizedType, `Expected type ${normalizedType}, got ${actualType}`);
        }
      } else if (normalizedType === 'array' || normalizedType.endsWith('[]') || normalizedType.startsWith('Array<')) {
        this._assert(Array.isArray(actual), `Expected array (${normalizedType}), got ${actualType}`);
      } else if (
        expected.value === undefined
        && /^[A-Z][A-Za-z0-9_]*$/.test(normalizedType)
        && actual !== null
        && actual !== undefined
        && typeof actual === 'object'
        && actual.constructor
      ) {
        if (expect) {
          expect(actual.constructor.name).toBe(normalizedType);
        } else {
          this._assert(
            actual.constructor.name === normalizedType,
            `Expected instance of ${normalizedType}, got ${actual.constructor.name}`
          );
        }
      }
    }

    // Check exact value
    if (expected.value !== undefined) {
      const normalizedActual = this._normalizeNegativeZero(actual);
      const expectedValue = this._hydrateExpectedForActual(expected.value, normalizedActual, classRegistry, expect);
      if (expectedValue === null && normalizedActual === undefined) {
        // JSON has no undefined; older recordings serialized undefined returns as null
        if (expect) {
          expect(normalizedActual == null).toBe(true);
        }
      } else if (
        normalizedActual instanceof Date
        && Number.isNaN(normalizedActual.getTime())
        && expectedValue instanceof Date
        && Number.isNaN(expectedValue.getTime())
      ) {
        if (expect) {
          expect(Number.isNaN(normalizedActual.getTime())).toBe(true);
        }
      } else if (expect) {
        expect(normalizedActual).toEqual(expectedValue);
      } else {
        this._assertDeepEqual(normalizedActual, expectedValue);
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

  _hydrateExpectedForActual(rawExpected, actual, classRegistry, expect) {
    if (rawExpected === null || rawExpected === undefined || typeof rawExpected !== 'object') {
      return rawExpected;
    }

    if (Array.isArray(rawExpected)) {
      return rawExpected.map((item, idx) =>
        this._hydrateExpectedForActual(item, Array.isArray(actual) ? actual[idx] : undefined, classRegistry, expect)
      );
    }

    if (typeof rawExpected['$type'] === 'string') {
      const type = rawExpected['$type'];
      if (type === 'Undefined') {
        return undefined;
      }
      if (type === 'Function') {
        if (expect && typeof expect.any === 'function') {
          return expect.any(Function);
        }
        return typeof actual === 'function' ? actual : undefined;
      }
      if (type === 'Symbol') {
        if (typeof actual === 'symbol' && (actual.description || '') === (rawExpected.description || '')) {
          return actual;
        }
        return Symbol(rawExpected.description || '');
      }
      if (type === 'RegExp') {
        return new RegExp(rawExpected.source || '', rawExpected.flags || '');
      }
      if (type === 'Number') {
        return Number(rawExpected.value);
      }
      if (type === 'Date' || type === 'DateTimeImmutable') {
        if (rawExpected.value === 'NaN') return new Date(NaN);
        return new Date(rawExpected.value || Date.now());
      }
      if (type === 'Buffer') {
        return Buffer.from(Array.isArray(rawExpected.data) ? rawExpected.data : []);
      }
      if (['Uint8Array', 'Int8Array', 'Uint16Array', 'Int16Array', 'Uint32Array', 'Int32Array', 'Float32Array', 'Float64Array'].includes(type)) {
        const Ctor = globalThis[type];
        return new Ctor(Array.isArray(rawExpected.data) ? rawExpected.data : []);
      }
      if (type === 'Map') {
        const entries = Array.isArray(rawExpected.entries)
          ? rawExpected.entries.map(([k, v]) => [
              this._hydrateExpectedForActual(k, undefined, classRegistry, expect),
              this._hydrateExpectedForActual(v, undefined, classRegistry, expect),
            ])
          : [];
        return new Map(entries);
      }
      if (type === 'Set') {
        const vals = Array.isArray(rawExpected.values)
          ? rawExpected.values.map(v => this._hydrateExpectedForActual(v, undefined, classRegistry, expect))
          : [];
        return new Set(vals);
      }

      // If actual is an instance of the exact recorded class (even if not in classRegistry),
      // hydrate into its prototype so Vitest/Jest toEqual matches class instances cleanly.
      const TargetClass = classRegistry[type]
        || (actual && typeof actual === 'object' && actual.constructor && actual.constructor.name === type
          ? actual.constructor
          : null);

      if (rawExpected['$error'] && TargetClass && typeof TargetClass === 'function') {
        const msg = rawExpected['$props'] ? rawExpected['$props'].message : '';
        const errInstance = new TargetClass(msg);
        if (rawExpected['$props'] && typeof rawExpected['$props'] === 'object') {
          for (const [k, v] of Object.entries(rawExpected['$props'])) {
            if (k === 'message') continue;
            if (k === 'name' && errInstance.name === v) continue;
            errInstance[k] = this._hydrateExpectedForActual(v, actual ? actual[k] : undefined, classRegistry, expect);
          }
        }
        return errInstance;
      }

      if (TargetClass && typeof TargetClass === 'function' && TargetClass.prototype && typeof TargetClass.prototype === 'object') {
        if (TargetClass.prototype instanceof Map || TargetClass.prototype instanceof Set) {
          if (expect && typeof expect.any === 'function') {
            return expect.any(TargetClass);
          }
          return actual;
        }
        const instance = Array.isArray(rawExpected['$ctor'])
          ? new TargetClass(...rawExpected['$ctor'].map(a => this.dtoHydrator.hydrate(a, classRegistry)))
          : Object.create(TargetClass.prototype);
        if (rawExpected['$props'] && typeof rawExpected['$props'] === 'object') {
          for (const [k, v] of Object.entries(rawExpected['$props'])) {
            instance[k] = this._hydrateExpectedForActual(v, actual ? actual[k] : undefined, classRegistry, expect);
          }
        }
        return instance;
      }

      return this.dtoHydrator.hydrate(rawExpected, classRegistry);
    }

    const out = {};
    for (const [k, v] of Object.entries(rawExpected)) {
      out[k] = this._hydrateExpectedForActual(
        v,
        actual && typeof actual === 'object' ? actual[k] : undefined,
        classRegistry,
        expect
      );
    }
    return out;
  }

  _normalizeNegativeZero(val, depth = 0) {
    if (typeof val === 'number' && Object.is(val, -0)) {
      return 0;
    }
    if (!val || typeof val !== 'object' || depth > 5) {
      return val;
    }
    if (Array.isArray(val)) {
      for (let i = 0; i < val.length; i++) {
        if (typeof val[i] === 'number' && Object.is(val[i], -0)) {
          val[i] = 0;
        }
      }
      return val;
    }
    if (val.constructor === Object) {
      for (const k of Object.keys(val)) {
        if (typeof val[k] === 'number' && Object.is(val[k], -0)) {
          val[k] = 0;
        }
      }
    }
    return val;
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
