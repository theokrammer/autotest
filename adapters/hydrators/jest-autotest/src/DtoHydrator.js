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

        // Built-in date codec
        if (type === 'Date' || type === 'DateTimeImmutable') {
          return new Date(value.value || Date.now());
        }

        const TargetClass = classRegistry[type];

        // Tier 2: Static Factory Method
        if (value['$factory'] && TargetClass && typeof TargetClass[value['$factory']] === 'function') {
          const ctorArgs = Array.isArray(value['$ctor'])
            ? value['$ctor'].map(arg => this.hydrate(arg, classRegistry))
            : [];
          return TargetClass[value['$factory']](...ctorArgs);
        }

        // Tier 3: Constructor Instantiation
        if (TargetClass && typeof TargetClass === 'function') {
          const ctorArgs = Array.isArray(value['$ctor'])
            ? value['$ctor'].map(arg => this.hydrate(arg, classRegistry))
            : [];
          const instance = new TargetClass(...ctorArgs);

          // Tier 4: Property Fallback
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
