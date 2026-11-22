<?php
declare(strict_types=1);

namespace Autotest\Hydrator;

final class OutcomeAsserter
{
    private DtoHydrator $dtoHydrator;

    public function __construct(DtoHydrator $dtoHydrator)
    {
        $this->dtoHydrator = $dtoHydrator;
    }

    public function assertOutcome(
        mixed $actualReturn,
        ?\Throwable $actualException,
        array $expectedSpec,
        object $testCase
    ): void {
        $outcomeKind = $expectedSpec['outcome'] ?? 'return';

        switch ($outcomeKind) {
            case 'return':
                if ($actualException !== null) {
                    throw new \AssertionError(
                        "Expected successful return, but caught exception: " . get_class($actualException) . ": " . $actualException->getMessage()
                    );
                }

                if (!array_key_exists('value', $expectedSpec) && !empty($expectedSpec['type'])) {
                    $expectedType = $expectedSpec['type'];
                    if ($expectedType === 'object' || str_contains($expectedType, '@anonymous')) {
                        if (method_exists($testCase, 'assertIsObject')) {
                            $testCase->assertIsObject($actualReturn);
                        } elseif (!is_object($actualReturn)) {
                            throw new \AssertionError("Expected object");
                        }
                    } elseif (interface_exists($expectedType) || class_exists($expectedType) || enum_exists($expectedType)) {
                        if (method_exists($testCase, 'assertInstanceOf')) {
                            $testCase->assertInstanceOf($expectedType, $actualReturn);
                        } elseif (!($actualReturn instanceof $expectedType)) {
                            throw new \AssertionError("Expected instance of {$expectedType}");
                        }
                    } elseif (in_array($expectedType, ['string', 'integer', 'double', 'boolean', 'array', 'NULL'], true)) {
                        if (method_exists($testCase, 'assertSame')) {
                            $testCase->assertSame($expectedType, gettype($actualReturn));
                        } elseif (gettype($actualReturn) !== $expectedType) {
                            throw new \AssertionError("Expected type {$expectedType}, got " . gettype($actualReturn));
                        }
                    }
                    break;
                }

                $rawExpected = $expectedSpec['value'] ?? null;
                $hasClosureMarker = $actualReturn instanceof \Closure
                    || str_contains((string)json_encode($rawExpected), '"$type":"ClosureMarker"');
                if ($hasClosureMarker) {
                    $recorder = new RecorderHook();
                    $normalizedActual = json_decode((string)json_encode($recorder->serializeValue($actualReturn)), true);
                    if (method_exists($testCase, 'assertEquals')) {
                        $testCase->assertEquals($rawExpected, $normalizedActual);
                    } elseif ($rawExpected != $normalizedActual) {
                        throw new \AssertionError(sprintf(
                            "Expected %s, got %s",
                            json_encode($rawExpected),
                            json_encode($normalizedActual)
                        ));
                    }
                    break;
                }

                $expectedValue = $this->dtoHydrator->hydrate($rawExpected);
                if (method_exists($testCase, 'assertEquals')) {
                    $testCase->assertEquals($expectedValue, $actualReturn);
                } else {
                    if ($expectedValue != $actualReturn) {
                        throw new \AssertionError(sprintf(
                            "Expected %s, got %s",
                            json_encode($expectedValue),
                            json_encode($actualReturn)
                        ));
                    }
                }
                break;

            case 'exception':
                if ($actualException === null) {
                    throw new \AssertionError("Expected exception of type {$expectedSpec['exception_type']}, but no exception was thrown.");
                }

                $expectedClass = $expectedSpec['exception_type'] ?? 'Throwable';
                if (method_exists($testCase, 'assertInstanceOf')) {
                    $testCase->assertInstanceOf($expectedClass, $actualException);
                } elseif (!($actualException instanceof $expectedClass)) {
                    throw new \AssertionError(sprintf(
                        "Expected exception instance of %s, got %s",
                        $expectedClass,
                        get_class($actualException)
                    ));
                }

                if (!empty($expectedSpec['message_contains'])) {
                    $substr = $expectedSpec['message_contains'];
                    if (method_exists($testCase, 'assertStringContainsString')) {
                        $testCase->assertStringContainsString($substr, $actualException->getMessage());
                    } elseif (!str_contains($actualException->getMessage(), $substr)) {
                        throw new \AssertionError(sprintf(
                            "Expected exception message to contain '%s', but got '%s'",
                            $substr,
                            $actualException->getMessage()
                        ));
                    }
                }
                break;

            case 'no_crash':
                if (method_exists($testCase, 'assertNull')) {
                    $testCase->assertNull($actualException);
                } elseif ($actualException !== null) {
                    throw new \AssertionError("Expected no_crash, but caught exception: " . $actualException->getMessage());
                }
                break;
        }
    }
}
