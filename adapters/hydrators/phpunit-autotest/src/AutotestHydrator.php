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
        $this->dtoHydrator->setMockContext($this->mockBinder, $testCase);

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

        // 3. Bind mocks & constructor overrides
        $targetClass = '';
        $targetNamespace = '';
        if (str_contains($targetId, '::')) {
            [$targetClass] = explode('::', $targetId, 2);
            $lastSlash = strrpos($targetClass, '\\');
            if ($lastSlash !== false) {
                $targetNamespace = substr($targetClass, 0, $lastSlash);
            }
        }

        // Prefer live PHP reflection types for constructor params so FQCNs match
        $refCtorParams = [];
        if ($targetClass !== '' && class_exists($targetClass)) {
            $refCls = new \ReflectionClass($targetClass);
            $refCtor = $refCls->getConstructor();
            if ($refCtor !== null) {
                foreach ($refCtor->getParameters() as $rp) {
                    $rt = $rp->getType();
                    $rtName = $rt instanceof \ReflectionNamedType ? $rt->getName() : null;
                    $refCtorParams[$rp->getName()] = $rtName;
                }
            }
        }

        $constructorOverrides = $scenarioSpec['constructor_overrides'] ?? [];
        $mockInstances = [];
        foreach ($constructorArgsSpec as $ctorParam) {
            $paramType = $ctorParam['type'];
            $paramName = $ctorParam['name'];

            if (array_key_exists($paramName, $constructorOverrides)) {
                $mockInstances[] = $this->dtoHydrator->hydrate($constructorOverrides[$paramName]);
                continue;
            }

            // Resolve short class/interface names relative to target class namespace or reflection
            $resolvedType = ltrim($refCtorParams[$paramName] ?? $paramType, '?');
            if (
                !interface_exists($resolvedType)
                && !class_exists($resolvedType)
                && $targetNamespace !== ''
                && !str_contains($resolvedType, '\\')
            ) {
                $candidate = $targetNamespace . '\\' . $resolvedType;
                if (interface_exists($candidate) || class_exists($candidate)) {
                    $resolvedType = $candidate;
                }
            }

            // Match mocks targeting this parameter
            $matchingMocks = array_filter($mocks, function ($m) use ($paramName, $resolvedType) {
                $target = $m['target'] ?? '';
                $shortType = basename(str_replace('\\', '/', $resolvedType));
                return str_starts_with($target, $paramName . '.')
                    || str_starts_with($target, $paramName . '::')
                    || str_starts_with($target, $resolvedType . '::')
                    || str_starts_with($target, $resolvedType . '.')
                    || str_starts_with($target, $shortType . '::')
                    || str_starts_with($target, $shortType . '.');
            });

            if (!empty($matchingMocks) || interface_exists($resolvedType) || class_exists($resolvedType)) {
                $mockInstances[] = $this->mockBinder->createMockForService($resolvedType, $matchingMocks, $testCase);
            } elseif (array_key_exists('default_value', $ctorParam) && $ctorParam['default_value'] !== null) {
                $mockInstances[] = $this->dtoHydrator->hydrate($ctorParam['default_value'], $paramType);
            } else {
                $cleanType = ltrim($paramType, '?');
                if (str_starts_with($paramType, '?') || str_contains($paramType, 'null')) {
                    $mockInstances[] = null;
                } else {
                    $mockInstances[] = match ($cleanType) {
                        'string' => '/test',
                        'int' => 1,
                        'float' => 1.0,
                        'bool' => false,
                        'array', 'iterable' => ['GET'],
                        default => null,
                    };
                }
            }
        }

        // 4. Target invocation & error capture
        $actualReturn = null;
        $actualException = null;

        $setupCalls = $scenarioSpec['setup_calls'] ?? [];
        $invokeReturnedArgs = array_key_exists('invoke_returned_args', $scenarioSpec)
            ? $scenarioSpec['invoke_returned_args']
            : null;
        $factorySpec = $scenarioSpec['factory'] ?? null;

        try {
            $actualReturn = $this->targetInvoker->invokeTarget(
                $targetId,
                $mockInstances,
                $inputs,
                $setupCalls,
                $invokeReturnedArgs,
                $factorySpec
            );
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
