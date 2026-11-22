class DtoHydrator {
  constructor(mockBinder = null) {
    this.customFactories = new Map();
    this.mockBinder = mockBinder;
  }

  setMockBinder(mockBinder) {
    this.mockBinder = mockBinder;
  }

  registerCustomFactory(typeName, factoryFn) {
    this.customFactories.set(typeName, factoryFn);
  }

  hydrate(value, classRegistry = {}) {
    if (value === null || value === undefined) {
      return value;
    }

    if (Array.isArray(value)) {
      return value.map(item => this.hydrate(item, classRegistry));
    }

    if (typeof value === 'object') {
      // Check for $type
      if (typeof value['$type'] === 'string') {
        const type = value['$type'];

        // Tier 1: Custom Factory Registry
        if (this.customFactories.has(type)) {
          return this.customFactories.get(type)(value);
        }

        // Built-in codecs
        if (type === 'AutoMock') {
          if (!this.mockBinder) {
            const { MockBinder } = require('./MockBinder');
            this.mockBinder = new MockBinder(this);
          }
          const specs = [];
          if (value.methods && typeof value.methods === 'object') {
            for (const [mName, retVal] of Object.entries(value.methods)) {
              specs.push({ target: `${value.class || 'Mock'}.${mName}`, returns: [{ return: retVal }] });
            }
          }
          return this.mockBinder.createMock(specs, classRegistry, { recursive: true }).mock;
        }
        if (type === 'DecoratedClass') {
          class AutotestDecorated {
            handle() { return 'ok'; }
          }
          const methodName = value.method_name || 'handle';
          if (typeof Reflect !== 'undefined' && typeof Reflect.defineMetadata === 'function') {
            if (value.metadata && typeof value.metadata === 'object') {
              for (const [k, v] of Object.entries(value.metadata)) {
                Reflect.defineMetadata(k, this.hydrate(v, classRegistry), AutotestDecorated);
                if (AutotestDecorated.prototype && typeof AutotestDecorated.prototype[methodName] === 'function') {
                  Reflect.defineMetadata(k, this.hydrate(v, classRegistry), AutotestDecorated.prototype[methodName]);
                  Reflect.defineMetadata(k, this.hydrate(v, classRegistry), AutotestDecorated.prototype, methodName);
                }
              }
            }
            if (value.method_metadata && typeof value.method_metadata === 'object') {
              for (const [k, v] of Object.entries(value.method_metadata)) {
                if (AutotestDecorated.prototype && typeof AutotestDecorated.prototype[methodName] === 'function') {
                  Reflect.defineMetadata(k, this.hydrate(v, classRegistry), AutotestDecorated.prototype[methodName]);
                  Reflect.defineMetadata(k, this.hydrate(v, classRegistry), AutotestDecorated.prototype, methodName);
                }
              }
            }
          }
          const kind = value.kind || 'class';
          if (kind === 'instance') return new AutotestDecorated();
          if (kind === 'prototype') return AutotestDecorated.prototype;
          if (kind === 'method') return AutotestDecorated.prototype[methodName];
          return AutotestDecorated;
        }
        if (type === 'Callback') {
          const returnSpec = value.returns;
          return (..._args) => this.hydrate(returnSpec, classRegistry);
        }
        if (type === 'Undefined') {
          return undefined;
        }
        if (type === 'Number') {
          return Number(value.value);
        }
        if (type === 'Buffer') {
          return Buffer.from(Array.isArray(value.data) ? value.data : []);
        }
        if (['Uint8Array', 'Int8Array', 'Uint16Array', 'Int16Array', 'Uint32Array', 'Int32Array', 'Float32Array', 'Float64Array'].includes(type)) {
          const Ctor = globalThis[type];
          return new Ctor(Array.isArray(value.data) ? value.data : []);
        }
        if (type === 'Date' || type === 'DateTimeImmutable') {
          return new Date(value.value || Date.now());
        }
        if (type === 'Map') {
          const entries = Array.isArray(value.entries)
            ? value.entries.map(([k, v]) => [this.hydrate(k, classRegistry), this.hydrate(v, classRegistry)])
            : [];
          return new Map(entries);
        }
        if (type === 'Set') {
          const vals = Array.isArray(value.values)
            ? value.values.map(v => this.hydrate(v, classRegistry))
            : [];
          return new Set(vals);
        }

        const TargetClass = classRegistry[type];

        // Tier 2: Static Factory Method
        if (value['$factory'] && TargetClass && typeof TargetClass[value['$factory']] === 'function') {
          const ctorArgs = Array.isArray(value['$ctor'])
            ? value['$ctor'].map(arg => this.hydrate(arg, classRegistry))
            : [];
          let instance = TargetClass[value['$factory']](...ctorArgs);
          if (Array.isArray(value['$setup_calls'])) {
            instance = this._replaySetupCallsSync(instance, TargetClass, value['$setup_calls'], classRegistry);
          }
          return instance;
        }

        // Tier 3 & 4: Constructor Instantiation or Prototype-Only Property Hydration
        if (TargetClass && typeof TargetClass === 'function') {
          let instance;
          if (Array.isArray(value['$ctor'])) {
            const ctorArgs = value['$ctor'].map(arg => this.hydrate(arg, classRegistry));
            instance = new TargetClass(...ctorArgs);
          } else {
            instance = Object.create(TargetClass.prototype);
          }

          if (value['$props'] && typeof value['$props'] === 'object') {
            for (const [k, v] of Object.entries(value['$props'])) {
              instance[k] = this.hydrate(v, classRegistry);
            }
          }
          if (Array.isArray(value['$setup_calls'])) {
            instance = this._replaySetupCallsSync(instance, TargetClass, value['$setup_calls'], classRegistry);
          }
          return instance;
        }

        // If no class constructor found, return structured object with hydrated properties
        const obj = {};
        if (value['$props']) {
          for (const [k, v] of Object.entries(value['$props'])) {
            obj[k] = this.hydrate(v, classRegistry);
          }
        }
        return obj;
      }

      // Plain object
      const plainObj = {};
      for (const [k, v] of Object.entries(value)) {
        plainObj[k] = this.hydrate(v, classRegistry);
      }
      return plainObj;
    }

    return value;
  }

  _replaySetupCallsSync(instance, TargetClass, setupCalls, classRegistry) {
    for (const call of setupCalls) {
      if (call && typeof instance[call.method] === 'function') {
        try {
          const callArgs = Array.isArray(call.args)
            ? call.args.map(a => this.hydrate(a, classRegistry))
            : [];
          const res = instance[call.method](...callArgs);
          if (res && typeof res.catch === 'function') {
            res.catch(() => {});
          } else if (res instanceof TargetClass) {
            instance = res;
          }
        } catch (_) {
          // Ignore invalid producer setup call arguments so target invocation still runs
        }
      }
    }
    return instance;
  }
}

module.exports = { DtoHydrator };
