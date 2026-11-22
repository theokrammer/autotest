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

    const seededTargets = [];
    if (Array.isArray(scenarioSpec.metadata_seeds) && typeof Reflect !== 'undefined' && typeof Reflect.defineMetadata === 'function') {
      const params = targetDef.signature?.parameters || [];
      for (const seed of scenarioSpec.metadata_seeds) {
        if (!seed || !seed.key) continue;
        const val = this.dtoHydrator.hydrate(seed.value, classRegistry);
        if (seed.target_param) {
          const pIdx = params.findIndex(p => p.name === seed.target_param);
          if (pIdx >= 0 && inputs[pIdx] && (typeof inputs[pIdx] === 'object' || typeof inputs[pIdx] === 'function')) {
            Reflect.defineMetadata(seed.key, val, inputs[pIdx]);
          }
        } else if (TargetClass) {
          Reflect.defineMetadata(seed.key, val, TargetClass);
          seededTargets.push([seed.key, TargetClass]);
        }
      }
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
        let instance;
        if (scenarioSpec.factory && scenarioSpec.factory.method && typeof TargetClass[scenarioSpec.factory.method] === 'function') {
          const factoryArgs = Array.isArray(scenarioSpec.factory.args)
            ? scenarioSpec.factory.args.map(a => this.dtoHydrator.hydrate(a, classRegistry))
            : [];
          instance = TargetClass[scenarioSpec.factory.method](...factoryArgs);
        } else {
          // Instance method - construct with mocks or constructor_overrides
          const ctorArgs = (targetDef.signature.constructor_args || []).map(param => {
            if (mockInstances[param.name] !== undefined) {
              return mockInstances[param.name];
            }
            // Use default or hydrate from scenario
            if (param.default_value !== null && param.default_value !== undefined) {
              return this.dtoHydrator.hydrate(param.default_value, classRegistry);
            }
            return undefined;
          });
          instance = new TargetClass(...ctorArgs);
        }

        if (Array.isArray(scenarioSpec.setup_calls)) {
          for (const call of scenarioSpec.setup_calls) {
            if (call && typeof instance[call.method] === 'function') {
              const callArgs = Array.isArray(call.args)
                ? call.args.map(a => this.dtoHydrator.hydrate(a, classRegistry))
                : [];
              const setupRes = await instance[call.method](...callArgs);
              if (setupRes instanceof TargetClass) {
                instance = setupRes;
              }
            }
          }
        }
        if (typeof instance[methodName] !== 'function') {
          throw new Error(`Method ${methodName} not found on ${className}`);
        }
        result = instance[methodName](...inputs);
      }

      // Await if result is a promise (with fast timeout guard for unresolving stub callbacks)
      if (result && typeof result.then === 'function') {
        result = await this._awaitWithTimeout(result, 250);
      }

      // Higher-order function invocation (e.g. factory/decorator/middleware creators)
      if (Array.isArray(scenarioSpec.invoke_returned_args) && typeof result === 'function') {
        const retArgs = scenarioSpec.invoke_returned_args.map(a => this.dtoHydrator.hydrate(a, classRegistry));
        try {
          result = result(...retArgs);
        } catch (invokeErr) {
          if (invokeErr instanceof TypeError && /cannot be invoked without 'new'/i.test(invokeErr.message)) {
            result = new result(...retArgs);
          } else {
            throw invokeErr;
          }
        }
        if (result && typeof result.then === 'function') {
          result = await this._awaitWithTimeout(result, 250);
        }
      }

      return { result, error: null };
    } catch (error) {
      return { result: null, error };
    } finally {
      if (typeof Reflect !== 'undefined' && typeof Reflect.deleteMetadata === 'function') {
        for (const [k, tgt] of seededTargets) {
          Reflect.deleteMetadata(k, tgt);
        }
      }
    }
  }

  _awaitWithTimeout(promise, ms) {
    let timer;
    return Promise.race([
      promise,
      new Promise((_, reject) => {
        timer = setTimeout(() => reject(new Error(`Autotest async timeout after ${ms}ms`)), ms);
      }),
    ]).finally(() => clearTimeout(timer));
  }
}

module.exports = { TargetInvoker };
