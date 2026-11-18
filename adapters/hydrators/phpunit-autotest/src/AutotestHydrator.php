<?php
declare(strict_types=1);

namespace Autotest\Hydrator;

final class AutotestHydrator
{
    private Loader $loader;
    private DtoHydrator $dtoHydrator;
    private MockBinder $mockBinder;
    private TargetInvoker $targetInvoker;
    private OutcomeAsserter $outcomeAsserter;
    private RecorderHook $recorderHook;

    public function __construct()
    {
        $this->loader = new Loader();
        $this->dtoHydrator = new DtoHydrator();
        $this->mockBinder = new MockBinder($this->dtoHydrator);
        $this->targetInvoker = new TargetInvoker($this->dtoHydrator);
        $this->outcomeAsserter = new OutcomeAsserter($this->dtoHydrator);
        $this->recorderHook = new RecorderHook();
    }

    public function executeScenario(string $scenarioFilePath, array $scenarioSpec, object $testCase): void
    {
        // 1. Resolve references if needed
        $mocks = $scenarioSpec['mocks'] ?? [];
        $inputs = $scenarioSpec['inputs'] ?? [];
        $expected = $scenarioSpec['expected'] ?? [];

        // 2. Identify target signature from document
        $data = $this->loader->loadScenarioFile($scenarioFilePath);
        $targetId = null;
        $constructorArgsSpec = [];

        foreach ($data['targets'] ?? [] as $target) {
            foreach ($target['scenarios'] ?? [] as $sc) {
                if ($sc['name'] === $scenarioSpec['name']) {
                    $targetId = $target['id'];
                    $constructorArgsSpec = $target['signature']['constructor_args'] ?? [];
                    break 2;
                }
            }
        }

        if ($targetId === null) {
            throw new \RuntimeException("Unable to locate target ID for scenario: {$scenarioSpec['name']}");
        }

        // 3. Bind mocks
        $mockInstances = [];
        foreach ($constructorArgsSpec as $ctorParam) {
            $paramType = $ctorParam['type'];
            $paramName = $ctorParam['name'];

            // Match mocks targeting this parameter
            $matchingMocks = array_filter($mocks, function ($m) use ($paramName) {
                $target = $m['target'] ?? '';
                return str_starts_with($target, $paramName . '.');
            });

            if (!empty($matchingMocks) || interface_exists($paramType)) {
                $mockInstances[] = $this->mockBinder->createMockForService($paramType, $matchingMocks, $testCase);
            } else {
                $mockInstances[] = $this->dtoHydrator->hydrate($ctorParam['default_value'] ?? null, $paramType);
            }
        }

        // 4. Target invocation & error capture
        $actualReturn = null;
        $actualException = null;

        try {
            $actualReturn = $this->targetInvoker->invokeTarget($targetId, $mockInstances, $inputs);
        } catch (\Throwable $t) {
            $actualException = $t;
        }

        // 5. Outcome assertion
        $this->outcomeAsserter->assertOutcome($actualReturn, $actualException, $expected, $testCase);
    }
}
