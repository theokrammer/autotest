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

    public function executeScenario(string $scenarioFilePath, array $scenarioSpec, object $testCase, array $options = []): void
    {
        // 1. Resolve references if needed
        $mocks = $scenarioSpec['mocks'] ?? [];
        $inputs = $scenarioSpec['inputs'] ?? [];
        $expected = $scenarioSpec['expected'] ?? [];

        // 2. Identify target signature from document
        $data = $this->loader->loadScenarioFile($scenarioFilePath);
        $targetId = null;
        $constructorArgsSpec = [];

        $requestedTargetId = $options['target_id'] ?? ($scenarioSpec['$target_id'] ?? null);
        foreach ($data['targets'] ?? [] as $target) {
            if ($requestedTargetId !== null && ($target['id'] ?? null) !== $requestedTargetId) {
                continue;
            }
            foreach ($target['scenarios'] ?? [] as $sc) {
                if ($sc['name'] === ($scenarioSpec['name'] ?? '')) {
                    $targetId = $target['id'];
                    $constructorArgsSpec = $target['signature']['constructor_args'] ?? [];
                    if (empty($mocks) && isset($sc['mocks'])) {
                        $mocks = $sc['mocks'];
                    }
                    if (empty($inputs) && isset($sc['inputs'])) {
                        $inputs = $sc['inputs'];
                    }
                    if (empty($expected) && isset($sc['expected'])) {
                        $expected = $sc['expected'];
                    }
                    break 2;
                }
            }
        }

        if ($targetId === null) {
            throw new \RuntimeException("Unable to locate target ID for scenario: {$scenarioSpec['name']}");
        }

        $fixtures = $data['fixtures'] ?? [];
        foreach ($mocks as &$m) {
            if (isset($m['$ref'])) {
                $refKey = str_replace('#/fixtures/', '', $m['$ref']);
                if (isset($fixtures[$refKey])) {
                    $m = $fixtures[$refKey];
                }
            }
        }
        unset($m);

        // 3. Bind mocks
        $mockInstances = [];
        foreach ($constructorArgsSpec as $ctorParam) {
            $paramType = $ctorParam['type'];
            $paramName = $ctorParam['name'];

            // Match mocks targeting this parameter
            $matchingMocks = array_filter($mocks, function ($m) use ($paramName, $paramType) {
                $target = $m['target'] ?? '';
                $shortType = basename(str_replace('\\', '/', $paramType));
                return str_starts_with($target, $paramName . '.')
                    || str_starts_with($target, $paramName . '::')
                    || str_starts_with($target, $paramType . '::')
                    || str_starts_with($target, $paramType . '.')
                    || str_starts_with($target, $shortType . '::')
                    || str_starts_with($target, $shortType . '.');
            });

            if (!empty($matchingMocks) || interface_exists($paramType) || class_exists($paramType)) {
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

        if (getenv('AUTOTEST_RECORD') !== false || !empty($options['record'])) {
            $recorded = $this->recorderHook->recordOutcome($actualReturn, $actualException);
            if (!empty($options['record_output'])) {
                file_put_contents($options['record_output'], json_encode($recorded, JSON_PRETTY_PRINT));
            } else {
                $stagingPath = str_replace('.autotest.json', '.autotest.staging.json', $scenarioFilePath);
                if ($stagingPath === $scenarioFilePath) {
                    $stagingPath = $scenarioFilePath . '.staging.json';
                }
                $entries = [];
                if (file_exists($stagingPath)) {
                    $decoded = json_decode((string)file_get_contents($stagingPath), true);
                    if (is_array($decoded)) {
                        $entries = $decoded;
                    }
                }
                $scenarioName = $scenarioSpec['name'] ?? 'unnamed';
                $entry = [
                    'target' => $targetId,
                    'scenario' => $scenarioName,
                    'recorded_at' => gmdate('Y-m-d\TH:i:s\Z'),
                    'mode' => 'recorded_sandbox',
                    'expected' => $recorded,
                ];
                $replaced = false;
                foreach ($entries as $idx => $existing) {
                    if (($existing['target'] ?? null) === $targetId && ($existing['scenario'] ?? null) === $scenarioName) {
                        $entries[$idx] = $entry;
                        $replaced = true;
                        break;
                    }
                }
                if (!$replaced) {
                    $entries[] = $entry;
                }
                file_put_contents($stagingPath, json_encode($entries, JSON_PRETTY_PRINT | JSON_UNESCAPED_SLASHES));
            }
            return;
        }

        // 5. Outcome assertion
        $this->outcomeAsserter->assertOutcome($actualReturn, $actualException, $expected, $testCase);
    }
}
