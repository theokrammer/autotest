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

    const inputs = (scenarioSpec.inputs || []).map(input =>
      this.dtoHydrator.hydrate(input, classRegistry)
    );

    try {
      let result;
      if (targetDef.kind === 'function') {
        // Static method
        if (typeof TargetClass[methodName] !== 'function') {
          throw new Error(`Static method ${className}.${methodName} not found`);
        }
        result = TargetClass[methodName](...inputs);
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
