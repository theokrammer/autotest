<?php
declare(strict_types=1);

namespace Autotest\Hydrator;

final class DtoHydrator
{
    private static ?array $customFactories = null;

    public function hydrate(mixed $value, ?string $expectedType = null): mixed
    {
        if (!is_array($value)) {
            return $this->coerceScalar($value, $expectedType);
        }

        // Built-in codecs
        if (isset($value['$type'])) {
            $type = $value['$type'];

            // Tier 1: Project Custom Factory Registry
            $customFactory = $this->lookupCustomFactory($type);
            if ($customFactory !== null) {
                return $customFactory($value);
            }

            // Built-in date/time codecs
            if ($type === 'DateTimeImmutable' || $type === 'DateTimeInterface') {
                return new \DateTimeImmutable($value['value'] ?? 'now');
            }

            // Tier 2: Static Factory Method
            if (!empty($value['$factory'])) {
                $factoryMethod = $value['$factory'];
                $ctorArgs = $this->hydrateArgs($value['$ctor'] ?? []);
                if (method_exists($type, $factoryMethod)) {
                    return $type::$factoryMethod(...$ctorArgs);
                }
            }

            // Tier 3: Constructor-First
            if (isset($value['$ctor']) && class_exists($type)) {
                $ctorArgs = $this->hydrateArgs($value['$ctor']);
                $refClass = new \ReflectionClass($type);
                $instance = $refClass->newInstanceArgs($ctorArgs);

                // Tier 4: Property-Fallback
                if (!empty($value['$props']) && is_array($value['$props'])) {
                    foreach ($value['$props'] as $propName => $propVal) {
                        if ($refClass->hasProperty($propName)) {
                            $prop = $refClass->getProperty($propName);
                            if ($prop->isPublic() && !$prop->isReadOnly()) {
                                $prop->setValue($instance, $this->hydrate($propVal));
                            }
                        }
                    }
                }

                return $instance;
            }
        }

        // Recursively hydrate array
        $hydrated = [];
        foreach ($value as $k => $v) {
            $hydrated[$k] = $this->hydrate($v);
        }
        return $hydrated;
    }

    private function hydrateArgs(array $args): array
    {
        return array_map(fn($arg) => $this->hydrate($arg), $args);
    }

    private function coerceScalar(mixed $value, ?string $type): mixed
    {
        if ($type === null || $value === null) {
            return $value;
        }

        return match ($type) {
            'int' => (int)$value,
            'float' => (float)$value,
            'string' => (string)$value,
            'bool' => (bool)$value,
            default => $value,
        };
    }

    private function lookupCustomFactory(string $type): ?callable
    {
        if (self::$customFactories === null) {
            self::$customFactories = [];
            $factoryFile = getcwd() . '/tests/autotest.factories.php';
            if (file_exists($factoryFile)) {
                $loaded = require $factoryFile;
                if (is_array($loaded)) {
                    self::$customFactories = $loaded;
                }
            }
        }

        return self::$customFactories[$type] ?? null;
    }
}
