class DtoHydrator {
  constructor() {
    this.customFactories = new Map();
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
          return TargetClass[value['$factory']](...ctorArgs);
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
}

module.exports = { DtoHydrator };
