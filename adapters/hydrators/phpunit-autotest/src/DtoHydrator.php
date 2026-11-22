<?php
declare(strict_types=1);

namespace Autotest\Hydrator;

final class DtoHydrator
{
    private static ?array $customFactories = null;
    private ?MockBinder $mockBinder = null;
    private ?object $testCase = null;
    private string $targetNamespace = '';

    public function setMockContext(MockBinder $binder, object $testCase): void
    {
        $this->mockBinder = $binder;
        $this->testCase = $testCase;
    }

    public function setTargetNamespace(string $ns): void
    {
        $this->targetNamespace = trim($ns, '\\');
    }

    public function resolveTypeName(string $name): string
    {
        $clean = ltrim(trim($name), '?\\');
        if ($clean === '' || class_exists($clean) || interface_exists($clean) || enum_exists($clean)) {
            return $clean;
        }
        if ($clean === 'Autotest\\Hydrator\\AutotestStubBackedEnum') {
            if (!enum_exists($clean, false)) {
                eval('namespace Autotest\\Hydrator; enum AutotestStubBackedEnum: string { case TEST = "test"; }');
            }
            return $clean;
        }
        if (!str_contains($clean, '\\')) {
            if ($this->targetNamespace !== '') {
                $candidate = $this->targetNamespace . '\\' . $clean;
                if (class_exists($candidate) || interface_exists($candidate) || enum_exists($candidate)) {
                    return $candidate;
                }
                // Check parent namespace (e.g. Brick\Money\Context -> Brick\Money\CurrencyType)
                $parentSlash = strrpos($this->targetNamespace, '\\');
                if ($parentSlash !== false) {
                    $parentCandidate = substr($this->targetNamespace, 0, $parentSlash) . '\\' . $clean;
                    if (class_exists($parentCandidate) || interface_exists($parentCandidate) || enum_exists($parentCandidate)) {
                        return $parentCandidate;
                    }
                }
            }
            $suffix = '\\' . $clean;
            foreach (get_declared_classes() as $decl) {
                if (str_ends_with($decl, $suffix)) {
                    return $decl;
                }
            }
            foreach (get_declared_interfaces() as $decl) {
                if (str_ends_with($decl, $suffix)) {
                    return $decl;
                }
            }
        }
        return $clean;
    }

    public function hydrate(mixed $value, ?string $expectedType = null): mixed
    {
        if (!is_array($value)) {
            if ($expectedType !== null && $this->isClassOrInterfaceType($expectedType)) {
                $targetClass = $this->resolveSingleClassOrInterface($expectedType);
                if ($targetClass !== null && (!is_object($value) || !($value instanceof $targetClass))) {
                    if ($this->mockBinder !== null) {
                        return $this->mockBinder->synthesizeDefaultForType($targetClass, 0, $this->testCase);
                    }
                }
            }
            return $this->coerceScalar($value, $expectedType);
        }

        // Built-in codecs
        if (isset($value['$type'])) {
            $rawType = (string)$value['$type'];
            $type = $this->resolveTypeName($rawType);

            if ($rawType === 'ClosureMarker') {
                return fn() => null;
            }

            // AutoMock synthesis
            if ($rawType === 'AutoMock') {
                $mockClass = $this->resolveTypeName((string)($value['class'] ?? 'stdClass'));
                if (($mockClass === 'BackedEnum' || $mockClass === 'UnitEnum' || enum_exists($mockClass)) && $this->mockBinder !== null) {
                    return $this->mockBinder->synthesizeDefaultForType($mockClass, 0, $this->testCase);
                }
                $specs = [];
                if (isset($value['methods']) && is_array($value['methods'])) {
                    foreach ($value['methods'] as $mName => $retVal) {
                        $specs[] = [
                            'target' => $mockClass . '::' . $mName,
                            'returns' => [['return' => $retVal]],
                        ];
                    }
                }
                if ($this->mockBinder !== null && $this->testCase !== null) {
                    return $this->mockBinder->createMockForService($mockClass, $specs, $this->testCase);
                }
                return new \stdClass();
            }

            // Tier 1: Project Custom Factory Registry
            $customFactory = $this->lookupCustomFactory($type);
            if ($customFactory !== null) {
                return $customFactory($value);
            }

            // Built-in date/time, enum, and callback codecs
            if ($rawType === 'Callback') {
                $returnSpec = $value['returns'] ?? null;
                return fn(...$args) => $this->hydrate($returnSpec);
            }
            if ($rawType === 'DateTimeImmutable' || $rawType === 'DateTimeInterface') {
                return new \DateTimeImmutable($value['value'] ?? 'now');
            }
            if (enum_exists($type)) {
                $caseName = $value['$props']['name'] ?? ($value['name'] ?? null);
                if (is_string($caseName) && $caseName !== '') {
                    $refEnum = new \ReflectionEnum($type);
                    if ($refEnum->hasCase($caseName)) {
                        return $refEnum->getCase($caseName)->getValue();
                    }
                }
                $cases = $type::cases();
                return $cases[0] ?? null;
            }

            $instance = null;

            // Tier 2: Static Factory Method
            if (!empty($value['$factory'])) {
                $factoryMethod = $value['$factory'];
                $rawCtor = $value['$ctor'] ?? [];
                if (method_exists($type, $factoryMethod)) {
                    $refMethod = new \ReflectionMethod($type, $factoryMethod);
                    $ctorArgs = $this->hydrateMethodArgs($refMethod, $rawCtor);
                    $instance = $type::$factoryMethod(...$ctorArgs);
                }
            }

            // Tier 3 & 4: Constructor-First with Property-Fallback
            if ($instance === null && class_exists($type)) {
                $refClass = new \ReflectionClass($type);
                if (isset($value['$ctor'])) {
                    $ctor = $refClass->getConstructor();
                    $ctorArgs = $ctor !== null
                        ? $this->hydrateMethodArgs($ctor, $value['$ctor'])
                        : $this->hydrateArgs($value['$ctor']);
                    if ($ctor !== null && !$ctor->isPublic()) {
                        $instance = $refClass->newInstanceWithoutConstructor();
                        $ctor->invokeArgs($instance, $ctorArgs);
                    } else {
                        $instance = $refClass->newInstanceArgs($ctorArgs);
                    }
                } else {
                    $instance = $refClass->newInstanceWithoutConstructor();
                }

                if ($instance !== null && isset($value['$props']) && is_array($value['$props'])) {
                    foreach ($value['$props'] as $propName => $propVal) {
                        $prop = null;
                        $currentRef = $refClass;
                        while ($currentRef !== false) {
                            if ($currentRef->hasProperty($propName)) {
                                $prop = $currentRef->getProperty($propName);
                                break;
                            }
                            $currentRef = $currentRef->getParentClass();
                        }
                        if ($prop !== null && !$prop->isStatic()) {
                            $prop->setValue($instance, $this->hydrate($propVal));
                        }
                    }
                }
            }

            if ($instance !== null) {
                if (isset($value['$setup_calls']) && is_array($value['$setup_calls'])) {
                    $instance = $this->replaySetupCalls($instance, $value['$setup_calls']);
                }
                return $instance;
            }
        }

        // Empty or plain array passed where a class/interface is expected
        if ($expectedType !== null && $this->isClassOrInterfaceType($expectedType)) {
            $targetClass = $this->resolveSingleClassOrInterface($expectedType);
            if ($targetClass !== null && $this->mockBinder !== null) {
                return $this->mockBinder->synthesizeDefaultForType($targetClass, 0, $this->testCase);
            }
        }

        // Recursively hydrate array
        $hydrated = [];
        foreach ($value as $k => $v) {
            $hydrated[$k] = $this->hydrate($v);
        }
        return $hydrated;
    }

    public function replaySetupCalls(object $instance, array $setupCalls): object
    {
        $className = get_class($instance);
        $refClass = new \ReflectionClass($className);
        foreach ($setupCalls as $call) {
            $method = $call['method'] ?? null;
            if (!$method || !$refClass->hasMethod($method)) {
                continue;
            }
            $refMethod = $refClass->getMethod($method);
            // Skip static mutators (like Request::setFactory) when replaying instance setup_calls on a DTO argument
            if ($refMethod->isStatic()) {
                continue;
            }
            $rawArgs = $call['args'] ?? [];
            $callArgs = $this->hydrateMethodArgs($refMethod, $rawArgs);
            $res = $refMethod->invokeArgs($instance, $callArgs);
            if (is_object($res) && $res instanceof $className) {
                $instance = $res;
            }
        }
        return $instance;
    }

    public function hydrateMethodArgs(\ReflectionFunctionAbstract $refMethod, array $rawArgs): array
    {
        $callArgs = [];
        $idx = 0;
        foreach ($refMethod->getParameters() as $param) {
            $name = $param->getName();
            $typeStr = $this->reflectionTypeToString($param->getType());
            $hasNamed = array_key_exists($name, $rawArgs);
            $hasIdx = array_key_exists($idx, $rawArgs);

            if ($param->isVariadic()) {
                if ($hasNamed || $hasIdx) {
                    $rawVal = $hasNamed ? $rawArgs[$name] : $rawArgs[$idx];
                    if (is_array($rawVal) && array_is_list($rawVal) && !isset($rawVal['$type']) && !$this->typeAcceptsArray($param->getType())) {
                        foreach ($rawVal as $item) {
                            $callArgs[] = $this->hydrate($item, $typeStr);
                        }
                    } else {
                        $callArgs[] = $this->hydrate($rawVal, $typeStr);
                    }
                }
                break;
            }

            if ($hasNamed) {
                $callArgs[] = $this->hydrate($rawArgs[$name], $typeStr);
            } elseif ($hasIdx) {
                $callArgs[] = $this->hydrate($rawArgs[$idx], $typeStr);
            } elseif ($param->isDefaultValueAvailable()) {
                $callArgs[] = $param->getDefaultValue();
            } elseif ($param->allowsNull()) {
                $callArgs[] = null;
            } else {
                $callArgs[] = $this->hydrate([], $typeStr);
            }
            $idx++;
        }
        return $callArgs;
    }

    private function reflectionTypeToString(?\ReflectionType $type): ?string
    {
        if ($type instanceof \ReflectionNamedType) {
            return $type->getName();
        }
        if ($type instanceof \ReflectionUnionType) {
            $names = [];
            foreach ($type->getTypes() as $t) {
                if ($t instanceof \ReflectionNamedType && $t->getName() !== 'null') {
                    $names[] = $t->getName();
                }
            }
            return empty($names) ? null : implode('|', $names);
        }
        return null;
    }

    private function typeAcceptsArray(?\ReflectionType $type): bool
    {
        if ($type instanceof \ReflectionNamedType) {
            return in_array($type->getName(), ['array', 'iterable', 'mixed'], true);
        }
        if ($type instanceof \ReflectionUnionType) {
            foreach ($type->getTypes() as $t) {
                if ($t instanceof \ReflectionNamedType && in_array($t->getName(), ['array', 'iterable', 'mixed'], true)) {
                    return true;
                }
            }
        }
        return false;
    }

    private function isClassOrInterfaceType(string $type): bool
    {
        foreach (explode('|', $type) as $part) {
            $clean = ltrim(trim($part), '?\\');
            if ($clean === '' || in_array(strtolower($clean), ['int', 'float', 'string', 'bool', 'array', 'iterable', 'callable', 'mixed', 'object', 'null', 'void', 'self', 'static'], true)) {
                return false;
            }
        }
        return $this->resolveSingleClassOrInterface($type) !== null;
    }

    private function resolveSingleClassOrInterface(string $type): ?string
    {
        foreach (explode('|', $type) as $part) {
            $clean = $this->resolveTypeName($part);
            if ($clean !== '' && (class_exists($clean) || interface_exists($clean) || enum_exists($clean))) {
                return $clean;
            }
        }
        return null;
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
