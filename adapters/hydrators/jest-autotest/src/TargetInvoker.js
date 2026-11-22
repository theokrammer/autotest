/**
 * TargetInvoker: instantiates the target class and invokes the method under test.
 */
class TargetInvoker {
  constructor(dtoHydrator) {
    this.dtoHydrator = dtoHydrator;
  }

  /**
   * @param {object} targetDef - { id, kind, signature }
   * @param {object} scenarioSpec - { inputs, mocks, ... }
   * @param {object} mockInstances - Map of param names to mock objects
   * @param {object} classRegistry - Map of type names to constructors
   * @returns {Promise<{ result: any, error: any }>}
   */
  async invoke(targetDef, scenarioSpec, mockInstances, classRegistry) {
    const targetId = targetDef.id;
    const parts = targetId.split('.');
    const className = parts[0];
    const methodName = parts[1];

    const TargetClass = classRegistry[className];
    if (!TargetClass) {
      throw new Error(`Target class not found in registry: ${className}`);
    }

    const rawInputs = scenarioSpec.inputs;
    let inputs = [];
    if (Array.isArray(rawInputs)) {
      inputs = rawInputs.map(input => this.dtoHydrator.hydrate(input, classRegistry));
    } else if (rawInputs && typeof rawInputs === 'object') {
      const params = targetDef.signature?.parameters || [];
      inputs = params.map(param => {
        if (param.name in rawInputs) {
          return this.dtoHydrator.hydrate(rawInputs[param.name], classRegistry);
        }
        if (param.default_value !== null && param.default_value !== undefined) {
          return this.dtoHydrator.hydrate(param.default_value, classRegistry);
        }
        return undefined;
      });
    }

    try {
      let result;
      if (targetDef.kind === 'function') {
        if (methodName && typeof TargetClass[methodName] === 'function') {
          result = TargetClass[methodName](...inputs);
        } else if (typeof TargetClass === 'function') {
          result = TargetClass(...inputs);
        } else {
          throw new Error(`Function or static method ${targetId} not found in registry`);
        }
      } else {
        // Instance method - construct with mocks
        const ctorArgs = (targetDef.signature.constructor_args || []).map(param => {
          if (mockInstances[param.name]) {
            return mockInstances[param.name];
          }
          // Use default or hydrate from scenario
          if (param.default_value !== null && param.default_value !== undefined) {
            return this.dtoHydrator.hydrate(param.default_value, classRegistry);
          }
          return undefined;
        });

        const instance = new TargetClass(...ctorArgs);
        if (typeof instance[methodName] !== 'function') {
          throw new Error(`Method ${methodName} not found on ${className}`);
        }
        result = instance[methodName](...inputs);
      }

      // Await if result is a promise
      if (result && typeof result.then === 'function') {
        result = await result;
      }

      return { result, error: null };
    } catch (error) {
      return { result: null, error };
    }
  }
}

module.exports = { TargetInvoker };
