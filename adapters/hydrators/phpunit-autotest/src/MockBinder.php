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

    public function createMockForService(string $interfaceOrClass, array $mockSpecs, object $testCase): object
    {
        if (is_callable([$testCase, 'createMock'])) {
            $mock = $testCase->createMock($interfaceOrClass);
        } else {
            // Standalone reflection fallback
            if (!interface_exists($interfaceOrClass) && !class_exists($interfaceOrClass)) {
                return $this->createDynamicMockInterface($interfaceOrClass, $mockSpecs);
            }
            $ref = new \ReflectionClass($interfaceOrClass);
            if ($ref->isInterface() || !$ref->isFinal()) {
                return $this->createDynamicMockInterface($interfaceOrClass, $mockSpecs);
            }
            if (empty($mockSpecs)) {
                return $ref->newInstanceWithoutConstructor();
            }
            throw new \RuntimeException("Cannot mock final class {$interfaceOrClass} with custom specs without TestCase");
        }

        foreach ($mockSpecs as $spec) {
            $target = $spec['target'] ?? '';
            if (str_contains($target, '::')) {
                $parts = explode('::', $target);
            } else {
                $parts = explode('.', $target);
            }
            $methodName = end($parts);

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

        return $mock;
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

    private function createDynamicMockInterface(string $interfaceOrClass, array $mockSpecs): object
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

        $clean = str_replace('\\', '_', trim($interfaceOrClass, '\\'));
        $className = 'AutotestMock_' . $clean . '_' . substr(md5($interfaceOrClass), 0, 8);

        if (!class_exists($className, false)) {
            $ref = new \ReflectionClass($interfaceOrClass);
            $methodsCode = [];
            foreach ($ref->getMethods(\ReflectionMethod::IS_PUBLIC) as $m) {
                if ($m->isConstructor() || $m->isDestructor() || $m->isStatic()) {
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
                if ($isVoid) {
                    $methodsCode[] = "public function {$mName}({$paramList}){$retType} { \$this->__dispatch('{$mName}', func_get_args()); return; }";
                } else {
                    $fallbackExpr = $this->defaultReturnExpr($m->getReturnType());
                    $methodsCode[] = "public function {$mName}({$paramList}){$retType} { \$r = \$this->__dispatch('{$mName}', func_get_args()); return \$r !== null ? \$r : {$fallbackExpr}; }";
                }
            }
            $methodsStr = implode("\n", $methodsCode);
            $implementsOrExtends = $ref->isInterface() ? "implements \\{$interfaceOrClass}" : "extends \\{$interfaceOrClass}";
            $code = "class {$className} {$implementsOrExtends} {
                private array \$__specs;
                private \\Autotest\\Hydrator\\MockBinder \$__binder;
                public function __construct(array \$specs, \\Autotest\\Hydrator\\MockBinder \$binder) {
                    \$this->__specs = \$specs;
                    \$this->__binder = \$binder;
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

        return new $className($mockSpecs, $this);
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

    private function defaultReturnExpr(?\ReflectionType $type): string
    {
        if ($type instanceof \ReflectionNamedType && !$type->allowsNull()) {
            return match ($type->getName()) {
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
        return 'null';
    }
}
