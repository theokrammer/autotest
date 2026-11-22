<?php
declare(strict_types=1);

namespace Autotest\Hydrator;

final class MockBinder
{
    private DtoHydrator $dtoHydrator;

    public function __construct(DtoHydrator $dtoHydrator)
    {
        $this->dtoHydrator = $dtoHydrator;
    }

    public function createMockForService(string $interfaceOrClass, array $mockSpecs, object $testCase, int $depth = 0): object
    {
        if ($depth > 3) {
            return new \stdClass();
        }
        $cleanName = ltrim($interfaceOrClass, '\\');
        if ($cleanName === 'BackedEnum' || $cleanName === 'UnitEnum') {
            return $this->synthesizeDefaultForType($cleanName, $depth, $testCase);
        }

        $special = $this->specialTypeInstance($cleanName);
        if ($special !== null) {
            return $special;
        }
        if (class_exists($cleanName) && empty($mockSpecs) && $this->isUnextendable($cleanName)) {
            $d = $this->synthesizeDefaultForType($cleanName, $depth + 1, $testCase);
            if (is_object($d)) {
                return $d;
            }
        }

        if (is_callable([$testCase, 'createMock'])) {
            try {
                $mock = $testCase->createMock($interfaceOrClass);
            } catch (\Throwable) {
                $d = $this->synthesizeDefaultForType($cleanName, $depth + 1, $testCase);
                return is_object($d) ? $d : new \stdClass();
            }
        } else {
            // Standalone reflection fallback
            if (!interface_exists($interfaceOrClass) && !class_exists($interfaceOrClass)) {
                return $this->createDynamicMockInterface($interfaceOrClass, $mockSpecs, $depth);
            }
            $ref = new \ReflectionClass($interfaceOrClass);
            if ($ref->isInterface() || !$this->isUnextendable($interfaceOrClass)) {
                return $this->createDynamicMockInterface($interfaceOrClass, $mockSpecs, $depth);
            }
            return $this->synthesizeDefaultForType($interfaceOrClass, $depth + 1, $testCase) ?? $ref->newInstanceWithoutConstructor();
        }

        $configuredMethods = [];
        foreach ($mockSpecs as $spec) {
            $target = $spec['target'] ?? '';
            if (str_contains($target, '::')) {
                $parts = explode('::', $target);
            } else {
                $parts = explode('.', $target);
            }
            $methodName = end($parts);
            $configuredMethods[$methodName] = true;

            $whenArgs = $spec['when_args'] ?? [];
            $returns = $spec['returns'] ?? [];
            $expectCalls = $spec['expect_calls'] ?? null;
            $mutateArgs = $spec['mutate_args'] ?? [];

            $callIndex = 0;
            $invocation = $mock->expects($expectCalls !== null ? $testCase->exactly($expectCalls) : $testCase->any())
                ->method($methodName);

            $invocation->willReturnCallback(function (...$actualArgs) use ($whenArgs, $returns, $mutateArgs, &$callIndex) {
                // Verify when_args matchers
                foreach ($whenArgs as $idx => $matcherSpec) {
                    if (isset($actualArgs[$idx])) {
                        $this->assertArgumentMatches($matcherSpec, $actualArgs[$idx]);
                    }
                }

                // Apply mutate_args if configured
                foreach ($mutateArgs as $argIdx => $mutation) {
                    $idx = (int)$argIdx;
                    if (isset($actualArgs[$idx]) && is_object($actualArgs[$idx]) && is_array($mutation)) {
                        foreach ($mutation as $k => $v) {
                            if (property_exists($actualArgs[$idx], $k)) {
                                $actualArgs[$idx]->$k = $v;
                            }
                        }
                    }
                }

                if (empty($returns)) {
                    return null;
                }

                $retSpec = $returns[$callIndex] ?? end($returns);
                $callIndex++;

                if (isset($retSpec['exception'])) {
                    $excClass = $retSpec['exception']['type'] ?? 'Exception';
                    $excMsg = $retSpec['exception']['message'] ?? '';
                    throw new $excClass($excMsg);
                }

                $retVal = $retSpec['return'] ?? ($retSpec['value'] ?? null);
                return $this->dtoHydrator->hydrate($retVal);
            });
        }

        // Configure type-aware defaults for unconfigured PHPUnit mock methods (self/static & final classes)
        if ((interface_exists($interfaceOrClass) || class_exists($interfaceOrClass)) && $depth < 3) {
            $ref = new \ReflectionClass($interfaceOrClass);
            foreach ($ref->getMethods(\ReflectionMethod::IS_PUBLIC) as $m) {
                $mName = $m->getName();
                if ($m->isConstructor() || $m->isDestructor() || $m->isStatic() || isset($configuredMethods[$mName])) {
                    continue;
                }
                if ($mName === 'generate') {
                    try {
                        $mock->method($mName)->willReturnCallback(
                            static fn(...$a) => str_repeat('0', isset($a[0]) && is_int($a[0]) && $a[0] > 0 ? $a[0] : 16)
                        );
                    } catch (\Throwable) {
                    }
                    continue;
                }
                $rt = $m->getReturnType();
                if (!($rt instanceof \ReflectionNamedType) || $rt->allowsNull() || $rt->isBuiltin()) {
                    continue;
                }
                $rtName = $rt->getName();
                try {
                    if ($rtName === 'self' || $rtName === 'static' || ltrim($rtName, '\\') === ltrim($interfaceOrClass, '\\')) {
                        $mock->method($mName)->willReturnSelf();
                    } elseif (class_exists($rtName) || interface_exists($rtName)) {
                        $stub = $this->synthesizeDefaultForType($rtName, $depth + 1, $testCase);
                        if ($stub !== null) {
                            $mock->method($mName)->willReturn($stub);
                        }
                    }
                } catch (\Throwable) {
                    // Ignore non-configurable methods on PHPUnit mocks
                }
            }
        }

        return $mock;
    }

    public function synthesizeDefaultForType(string $typeName, int $depth = 0, ?object $testCase = null): mixed
    {
        if ($depth > 3) {
            return null;
        }

        $clean = ltrim($typeName, '?\\');
        if ($clean === '' || $clean === 'object') {
            return new \stdClass();
        }
        $specialInst = $this->specialTypeInstance($clean);
        if ($specialInst !== null && $clean !== 'BackedEnum' && $clean !== 'UnitEnum') {
            return $specialInst;
        }
        if ($clean === 'BackedEnum' || $clean === 'UnitEnum') {
            if (!enum_exists('Autotest\\Hydrator\\AutotestStubBackedEnum', false)) {
                eval('namespace Autotest\\Hydrator; enum AutotestStubBackedEnum: string { case TEST = "test"; }');
            }
            return \Autotest\Hydrator\AutotestStubBackedEnum::TEST;
        }

        if (interface_exists($clean)) {
            if ($testCase !== null && is_callable([$testCase, 'createMock'])) {
                return $this->createMockForService($clean, [], $testCase, $depth + 1);
            }
            return $this->createDynamicMockInterface($clean, [], $depth + 1);
        }

        if (!class_exists($clean)) {
            return null;
        }

        $ref = new \ReflectionClass($clean);
        if ($ref->isAbstract()) {
            return null;
        }

        // 1. Prefer named static factories when available (e.g. MatchingResult::fromFailure)
        foreach (['fromFailure', 'fromSuccess', 'create', 'get', 'fail', 'success'] as $factoryName) {
            if ($ref->hasMethod($factoryName)) {
                $fm = $ref->getMethod($factoryName);
                if ($fm->isPublic() && $fm->isStatic()) {
                    $fArgs = [];
                    foreach ($fm->getParameters() as $fp) {
                        $fArgs[] = $this->defaultArgForReflectionParam($fp, $depth + 1, $testCase);
                    }
                    try {
                        return $fm->invokeArgs(null, $fArgs);
                    } catch (\Throwable) {
                    }
                }
            }
        }

        // 2. Public constructor
        $ctor = $ref->getConstructor();
        if ($ctor === null) {
            return $ref->newInstance();
        }

        $cArgs = [];
        foreach ($ctor->getParameters() as $cp) {
            $cArgs[] = $this->defaultArgForReflectionParam($cp, $depth + 1, $testCase);
        }

        if ($ctor->isPublic()) {
            try {
                return $ref->newInstanceArgs($cArgs);
            } catch (\Throwable) {
                // Retry with numeric/zero args for classes like Ramsey\Uuid\Type\Integer('0')
                $altArgs = [];
                foreach ($ctor->getParameters() as $cp) {
                    $t = $cp->getType();
                    $tn = $t instanceof \ReflectionNamedType ? $t->getName() : '';
                    $altArgs[] = match ($tn) {
                        'string' => '0',
                        'int' => 0,
                        'float' => 0.0,
                        default => $this->defaultArgForReflectionParam($cp, $depth + 1, $testCase),
                    };
                }
                try {
                    return $ref->newInstanceArgs($altArgs);
                } catch (\Throwable) {
                }
            }
        }

        if (!$this->isUnextendable($clean)) {
            return $this->createDynamicMockInterface($clean, [], $depth + 1);
        }

        $inst = $ref->newInstanceWithoutConstructor();
        try {
            $ctor->invokeArgs($inst, $cArgs);
        } catch (\Throwable) {
            // Ensure typed properties are initialized so getters don't throw Error: Typed property ... must not be accessed before initialization
            $cur = $ref;
            while ($cur !== false) {
                foreach ($cur->getProperties() as $prop) {
                    if ($prop->isStatic() || $prop->isInitialized($inst)) {
                        continue;
                    }
                    $pt = $prop->getType();
                    $val = null;
                    if ($pt instanceof \ReflectionNamedType && !$pt->allowsNull()) {
                        $val = match ($pt->getName()) {
                            'string' => '0',
                            'int' => 0,
                            'float' => 0.0,
                            'bool', 'false' => false,
                            'true' => true,
                            'array', 'iterable' => [],
                            'object' => new \stdClass(),
                            default => $this->synthesizeDefaultForType($pt->getName(), $depth + 1, $testCase),
                        };
                    }
                    try {
                        $prop->setValue($inst, $val);
                    } catch (\Throwable) {
                    }
                }
                $cur = $cur->getParentClass();
            }
        }
        return $inst;
    }

    /**
     * Types PHP forbids user classes from implementing/extending (enums, DateTimeInterface, Throwable, DateInterval)
     * are replaced by real instances instead of dynamic mocks.
     */
    private function specialTypeInstance(string $name): ?object
    {
        $clean = ltrim($name, '\\');
        if ($clean === '' || $clean === 'BackedEnum' || $clean === 'UnitEnum') {
            return null;
        }
        if (enum_exists($clean)) {
            $cases = $clean::cases();
            return $cases[0] ?? null;
        }
        if (strcasecmp($clean, 'DateTimeInterface') === 0 || strcasecmp($clean, 'DateTimeImmutable') === 0) {
            return new \DateTimeImmutable('2024-01-01T00:00:00+00:00');
        }
        if (strcasecmp($clean, 'DateTime') === 0) {
            return new \DateTime('2024-01-01T00:00:00+00:00');
        }
        if (strcasecmp($clean, 'DateInterval') === 0) {
            return new \DateInterval('P0D');
        }
        if (strcasecmp($clean, 'DateTimeZone') === 0) {
            return new \DateTimeZone('UTC');
        }
        if (strcasecmp($clean, 'Throwable') === 0) {
            return new \RuntimeException('autotest');
        }
        return null;
    }

    /** Final / readonly classes and classes with final constructors cannot be subclassed by mocks. */
    private function isUnextendable(string $class): bool
    {
        if (!class_exists($class)) {
            return false;
        }
        $r = new \ReflectionClass($class);
        $ctor = $r->getConstructor();
        return $r->isFinal() || $r->isReadOnly() || ($ctor !== null && $ctor->isFinal());
    }

    private function defaultArgForReflectionParam(\ReflectionParameter $param, int $depth, ?object $testCase): mixed
    {
        if ($param->isDefaultValueAvailable()) {
            return $param->getDefaultValue();
        }
        if ($param->allowsNull()) {
            return null;
        }
        $type = $param->getType();
        if ($type instanceof \ReflectionNamedType) {
            $name = $type->getName();
            return match ($name) {
                'string' => '/test',
                'int' => 1,
                'float' => 1.0,
                'bool', 'false' => false,
                'true' => true,
                'array', 'iterable' => ['GET'],
                'object' => new \stdClass(),
                default => $this->synthesizeDefaultForType($name, $depth + 1, $testCase),
            };
        }
        return null;
    }

    public function assertArgumentMatches(mixed $matcher, mixed $actual): void
    {
        if (!is_array($matcher) || !isset($matcher['$matcher'])) {
            if ($matcher !== $actual) {
                throw new \AssertionError(sprintf("Argument mismatch: expected %s, got %s", json_encode($matcher), json_encode($actual)));
            }
            return;
        }

        $kind = $matcher['$matcher'];
        switch ($kind) {
            case 'any':
                break;
            case 'exact':
                $expected = $matcher['value'] ?? null;
                if ($expected !== $actual) {
                    throw new \AssertionError(sprintf("Argument mismatch: expected %s, got %s", json_encode($expected), json_encode($actual)));
                }
                break;
            case 'regex':
                $pattern = $matcher['pattern'] ?? '//';
                if ($pattern !== '' && !in_array($pattern[0], ['/', '#', '~', '%', '@'], true)) {
                    $pattern = '/' . str_replace('/', '\/', $pattern) . '/';
                }
                if (!preg_match($pattern, (string)$actual)) {
                    throw new \AssertionError("Regex mismatch: {$actual} does not match {$pattern}");
                }
                break;
            case 'range':
                $min = $matcher['min'] ?? -INF;
                $max = $matcher['max'] ?? INF;
                if ($actual < $min || $actual > $max) {
                    throw new \AssertionError("Range mismatch: {$actual} not in [{$min}, {$max}]");
                }
                break;
            case 'subset':
                $expectedSubset = $matcher['subset'] ?? [];
                if (!is_array($actual)) {
                    throw new \AssertionError("Subset mismatch: actual is not an array");
                }
                foreach ($expectedSubset as $k => $v) {
                    if (!array_key_exists($k, $actual) || $actual[$k] !== $v) {
                        throw new \AssertionError("Subset mismatch on key {$k}");
                    }
                }
                break;
        }
    }

    public function dispatchDynamicCall(array $specs, string $name, array $args): mixed
    {
        foreach ($specs as $spec) {
            $target = $spec['target'] ?? '';
            if (str_contains($target, '::')) {
                $parts = explode('::', $target);
            } else {
                $parts = explode('.', $target);
            }
            $targetMethod = end($parts);
            if ($targetMethod === $name) {
                $whenArgs = $spec['when_args'] ?? [];
                $mutateArgs = $spec['mutate_args'] ?? [];
                $returns = $spec['returns'] ?? [];

                // Check when_args
                $matched = true;
                foreach ($whenArgs as $idx => $matcherSpec) {
                    if (isset($args[$idx])) {
                        try {
                            $this->assertArgumentMatches($matcherSpec, $args[$idx]);
                        } catch (\AssertionError $e) {
                            $matched = false;
                            break;
                        }
                    }
                }
                if (!$matched) {
                    continue;
                }

                // Apply mutate_args
                foreach ($mutateArgs as $argIdx => $mutation) {
                    $idx = (int)$argIdx;
                    if (isset($args[$idx]) && is_object($args[$idx]) && is_array($mutation)) {
                        foreach ($mutation as $k => $v) {
                            if (property_exists($args[$idx], $k)) {
                                $args[$idx]->$k = $this->dtoHydrator->hydrate($v);
                            }
                        }
                    }
                }

                if (empty($returns)) {
                    return null;
                }

                $retSpec = $returns[0];
                if (isset($retSpec['exception'])) {
                    $excClass = $retSpec['exception']['type'] ?? 'Exception';
                    $excMsg = $retSpec['exception']['message'] ?? '';
                    throw new $excClass($excMsg);
                }

                $retVal = $retSpec['return'] ?? ($retSpec['value'] ?? null);
                return $this->dtoHydrator->hydrate($retVal);
            }
        }
        return null;
    }

    private function createDynamicMockInterface(string $interfaceOrClass, array $mockSpecs, int $depth = 0): object
    {
        if (!interface_exists($interfaceOrClass) && !class_exists($interfaceOrClass)) {
            return new class($mockSpecs, $this) {
                private array $specs;
                private MockBinder $binder;
                public function __construct(array $specs, MockBinder $binder) {
                    $this->specs = $specs;
                    $this->binder = $binder;
                }
                public function __call(string $name, array $args): mixed {
                    return $this->binder->dispatchDynamicCall($this->specs, $name, $args);
                }
            };
        }

        $special = $this->specialTypeInstance($interfaceOrClass);
        if ($special !== null) {
            return $special;
        }
        if (class_exists($interfaceOrClass) && $this->isUnextendable($interfaceOrClass)) {
            $d = $this->synthesizeDefaultForType($interfaceOrClass, $depth + 1, null);
            if (is_object($d)) {
                return $d;
            }
        }

        $clean = str_replace('\\', '_', trim($interfaceOrClass, '\\'));
        $className = 'AutotestMock_' . $clean . '_' . substr(md5($interfaceOrClass), 0, 8);

        if (!class_exists($className, false)) {
            $ref = new \ReflectionClass($interfaceOrClass);
            $methodsCode = [];
            foreach ($ref->getMethods(\ReflectionMethod::IS_PUBLIC | \ReflectionMethod::IS_PROTECTED) as $m) {
                if ($m->isConstructor() || $m->isDestructor() || $m->isFinal()) {
                    continue;
                }
                // Only emit protected or static methods if they are abstract (e.g. AbstractSessionHandler::doRead or RandomLib\Mixer::getStrength)
                if (($m->isProtected() || $m->isStatic()) && !$m->isAbstract()) {
                    continue;
                }
                $mName = $m->getName();
                $params = [];
                foreach ($m->getParameters() as $p) {
                    $pStr = '';
                    if ($p->hasType()) {
                        $typeStr = $this->formatReflectionType($p->getType(), $interfaceOrClass);
                        if ($typeStr !== '') {
                            $pStr .= $typeStr . ' ';
                        }
                    }
                    if ($p->isPassedByReference()) {
                        $pStr .= '&';
                    }
                    if ($p->isVariadic()) {
                        $pStr .= '...';
                    }
                    $pStr .= '$' . $p->getName();
                    if ($p->isDefaultValueAvailable()) {
                        $pStr .= ' = ' . var_export($p->getDefaultValue(), true);
                    }
                    $params[] = $pStr;
                }
                $paramList = implode(', ', $params);
                $retType = '';
                $isVoid = false;
                if ($m->hasReturnType()) {
                    $rt = $m->getReturnType();
                    $rtStr = $this->formatReflectionType($rt, $interfaceOrClass);
                    if ($rtStr === 'void' || $rtStr === 'never') {
                        $isVoid = true;
                    }
                    if ($rtStr !== '') {
                        $retType = ': ' . $rtStr;
                    }
                }
                $vis = $m->isProtected() ? 'protected' : 'public';
                if ($m->isStatic()) {
                    if ($isVoid) {
                        $methodsCode[] = "{$vis} static function {$mName}({$paramList}){$retType} { return; }";
                    } else {
                        $staticFallback = $this->staticDefaultReturnExpr($m->getReturnType(), $interfaceOrClass);
                        $methodsCode[] = "{$vis} static function {$mName}({$paramList}){$retType} { return {$staticFallback}; }";
                    }
                } elseif ($isVoid) {
                    $methodsCode[] = "{$vis} function {$mName}({$paramList}){$retType} { \$this->__dispatch('{$mName}', func_get_args()); return; }";
                } elseif ($mName === 'generate') {
                    $methodsCode[] = "{$vis} function {$mName}({$paramList}){$retType} { \$a = func_get_args(); \$r = \$this->__dispatch('{$mName}', \$a); return \$r !== null ? \$r : str_repeat('0', isset(\$a[0]) && is_int(\$a[0]) && \$a[0] > 0 ? \$a[0] : 16); }";
                } else {
                    $fallbackExpr = $this->defaultReturnExpr($m->getReturnType(), $interfaceOrClass, $depth);
                    $methodsCode[] = "{$vis} function {$mName}({$paramList}){$retType} { \$r = \$this->__dispatch('{$mName}', func_get_args()); return \$r !== null ? \$r : {$fallbackExpr}; }";
                }
            }
            $methodsStr = implode("\n", $methodsCode);
            $implementsOrExtends = $ref->isInterface() ? "implements \\{$interfaceOrClass}" : "extends \\{$interfaceOrClass}";
            $code = "class {$className} {$implementsOrExtends} {
                private array \$__specs;
                private \\Autotest\\Hydrator\\MockBinder \$__binder;
                private int \$__depth;
                public function __construct(array \$specs, \\Autotest\\Hydrator\\MockBinder \$binder, int \$depth = 0) {
                    \$this->__specs = \$specs;
                    \$this->__binder = \$binder;
                    \$this->__depth = \$depth;
                }
                public function __dispatch(string \$name, array \$args): mixed {
                    return \$this->__binder->dispatchDynamicCall(\$this->__specs, \$name, \$args);
                }
                public function __call(string \$name, array \$args): mixed {
                    return \$this->__dispatch(\$name, \$args);
                }
                {$methodsStr}
            }";
            eval($code);
        }

        return new $className($mockSpecs, $this, $depth);
    }

    private function formatReflectionType(?\ReflectionType $type, string $ownerClass): string
    {
        if ($type === null) {
            return '';
        }

        if ($type instanceof \ReflectionNamedType) {
            $name = $type->getName();
            if ($name === 'self') {
                $name = '\\' . ltrim($ownerClass, '\\');
            } elseif ($name === 'static' || $type->isBuiltin()) {
                // keep as-is (int, string, mixed, void, never, null, true, false, static, etc.)
            } else {
                $name = '\\' . ltrim($name, '\\');
            }

            if ($type->allowsNull() && !in_array($name, ['mixed', 'null', 'void', 'never'], true)) {
                return '?' . $name;
            }
            return $name;
        }

        if ($type instanceof \ReflectionUnionType) {
            $parts = array_map(fn($t) => $this->formatReflectionType($t, $ownerClass), $type->getTypes());
            return implode('|', array_filter($parts, fn($p) => $p !== ''));
        }

        if ($type instanceof \ReflectionIntersectionType) {
            $parts = array_map(fn($t) => $this->formatReflectionType($t, $ownerClass), $type->getTypes());
            return implode('&', array_filter($parts, fn($p) => $p !== ''));
        }

        return '';
    }

    private function defaultReturnExpr(?\ReflectionType $type, string $ownerClass = '', int $depth = 0): string
    {
        if ($type instanceof \ReflectionNamedType && !$type->allowsNull()) {
            $name = $type->getName();
            if ($name === 'self' || $name === 'static' || ($ownerClass !== '' && ltrim($name, '\\') === ltrim($ownerClass, '\\'))) {
                return '$this';
            }
            if ($type->isBuiltin()) {
                return match ($name) {
                    'array', 'iterable' => '[]',
                    'string' => "''",
                    'int' => '0',
                    'float' => '0.0',
                    'bool', 'false' => 'false',
                    'true' => 'true',
                    'object' => 'new \stdClass()',
                    default => 'null',
                };
            }
            if ($depth < 3 && (class_exists($name) || interface_exists($name) || enum_exists($name))) {
                $escaped = var_export($name, true);
                return "\$this->__binder->synthesizeDefaultForType({$escaped}, \$this->__depth + 1)";
            }
        }
        return 'null';
    }

    private function staticDefaultReturnExpr(?\ReflectionType $type, string $ownerClass = ''): string
    {
        if ($type instanceof \ReflectionNamedType && !$type->allowsNull()) {
            $name = $type->getName();
            if ($type->isBuiltin()) {
                return match ($name) {
                    'array', 'iterable' => '[]',
                    'string' => "''",
                    'int' => '0',
                    'float' => '0.0',
                    'bool', 'false' => 'false',
                    'true' => 'true',
                    'object' => 'new \stdClass()',
                    default => 'null',
                };
            }
            if (enum_exists($name)) {
                return '\\' . ltrim($name, '\\') . '::cases()[0]';
            }
        }
        return 'null';
    }
}
