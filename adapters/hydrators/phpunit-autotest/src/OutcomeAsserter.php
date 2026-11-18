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

                $expectedValue = $this->dtoHydrator->hydrate($expectedSpec['value'] ?? null);
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
                if (!($actualException instanceof $expectedClass)) {
                    throw new \AssertionError(sprintf(
                        "Expected exception instance of %s, got %s",
                        $expectedClass,
                        get_class($actualException)
                    ));
                }

                if (!empty($expectedSpec['message_contains'])) {
                    $substr = $expectedSpec['message_contains'];
                    if (!str_contains($actualException->getMessage(), $substr)) {
                        throw new \AssertionError(sprintf(
                            "Expected exception message to contain '%s', but got '%s'",
                            $substr,
                            $actualException->getMessage()
                        ));
                    }
                }
                break;

            case 'no_crash':
                if ($actualException !== null) {
                    throw new \AssertionError("Expected no_crash, but caught exception: " . $actualException->getMessage());
                }
                break;
        }
    }
}
