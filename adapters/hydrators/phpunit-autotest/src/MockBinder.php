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
        if (method_exists($testCase, 'createMock')) {
            $mock = $testCase->createMock($interfaceOrClass);
        } else {
            // Standalone reflection fallback
            $ref = new \ReflectionClass($interfaceOrClass);
            if ($ref->isInterface()) {
                $mock = $this->createDynamicMockInterface($interfaceOrClass, $mockSpecs);
                return $mock;
            }
            throw new \RuntimeException("Cannot create mock for {$interfaceOrClass} without TestCase");
        }

        foreach ($mockSpecs as $spec) {
            $target = $spec['target'] ?? '';
            $parts = explode('.', $target);
            $methodName = end($parts);

            $whenArgs = $spec['when_args'] ?? [];
            $returns = $spec['returns'] ?? [];
            $expectCalls = $spec['expect_calls'] ?? null;

            $callIndex = 0;
            $invocation = $mock->expects($expectCalls !== null ? $testCase->exactly($expectCalls) : $testCase->any())
                ->method($methodName);

            $invocation->willReturnCallback(function (...$actualArgs) use ($whenArgs, $returns, &$callIndex) {
                // Verify when_args matchers
                foreach ($whenArgs as $idx => $matcherSpec) {
                    if (isset($actualArgs[$idx])) {
                        $this->assertArgumentMatches($matcherSpec, $actualArgs[$idx]);
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

                return $this->dtoHydrator->hydrate($retSpec['return'] ?? null);
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
            case 'regex':
                $pattern = $matcher['pattern'] ?? '//';
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

    private function createDynamicMockInterface(string $interface, array $mockSpecs): object
    {
        return new class($mockSpecs, $this->dtoHydrator, $this) {
            private array $specs;
            private DtoHydrator $hydrator;
            private MockBinder $binder;

            public function __construct(array $specs, DtoHydrator $hydrator, MockBinder $binder) {
                $this->specs = $specs;
                $this->hydrator = $hydrator;
                $this->binder = $binder;
            }

            public function __call(string $name, array $args) {
                foreach ($this->specs as $spec) {
                    $target = $spec['target'] ?? '';
                    if (str_ends_with($target, '.' . $name)) {
                        $returns = $spec['returns'] ?? [];
                        if (!empty($returns)) {
                            $first = $returns[0];
                            if (isset($first['exception'])) {
                                $exc = $first['exception']['type'];
                                throw new $exc($first['exception']['message'] ?? '');
                            }
                            return $this->hydrator->hydrate($first['return'] ?? null);
                        }
                    }
                }
                return null;
            }
        };
    }
}
