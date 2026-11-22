<?php
declare(strict_types=1);

namespace App\Billing {
    interface TaxRepo {
        public function getRate(string $country): float;
    }
    class PromotedService {
        public function __construct(private TaxRepo $taxRepo) {}
        public function getRate(string $country): float {
            return $this->taxRepo->getRate($country);
        }
    }
}

namespace Tests\Autotest {
    require_once __DIR__ . '/../src/Loader.php';
    require_once __DIR__ . '/../src/DtoHydrator.php';
    require_once __DIR__ . '/../src/MockBinder.php';
    require_once __DIR__ . '/../src/TargetInvoker.php';
    require_once __DIR__ . '/../src/OutcomeAsserter.php';
    require_once __DIR__ . '/../src/RecorderHook.php';
    require_once __DIR__ . '/../src/AutotestHydrator.php';

    use Autotest\Hydrator\AutotestHydrator;
    use Autotest\Hydrator\DtoHydrator;
    use Autotest\Hydrator\Loader;
    use Autotest\Hydrator\MockBinder;
    use Autotest\Hydrator\OutcomeAsserter;
    use Autotest\Hydrator\RecorderHook;
    use Autotest\Hydrator\TargetInvoker;

    $errors = [];

    // 1. Test DtoHydrator
    echo "Testing DtoHydrator...\n";
    $dto = new DtoHydrator();
    try {
        assert($dto->hydrate(42) === 42);
        assert($dto->hydrate("foo") === "foo");
        assert($dto->hydrate("42", "int") === 42);
        assert($dto->hydrate("3.14", "float") === 3.14);
        assert($dto->hydrate("1", "bool") === true);

        $date = $dto->hydrate(['$type' => 'DateTimeImmutable', 'value' => '2024-01-01T00:00:00Z']);
        assert($date instanceof \DateTimeImmutable);
        echo "  DtoHydrator basic tests passed\n";
    } catch (\Throwable $e) {
        $errors[] = "DtoHydrator failed: " . $e->getMessage();
        echo "  DtoHydrator FAILED: " . $e->getMessage() . "\n";
    }

    // 2. Test Loader
    echo "Testing Loader...\n";
    $loader = new Loader();
    try {
        $file = __DIR__ . '/../../../../conformance/profiles/php/php_constructor_promotion.autotest.json';
        $data = $loader->loadScenarioFile($file);
        assert(isset($data['targets'][0]['scenarios'][0]['mocks'][0]['target']));
        assert($data['targets'][0]['scenarios'][0]['mocks'][0]['target'] === 'taxRepo.getRate');
        echo "  Loader test passed (ref resolved)\n";
    } catch (\Throwable $e) {
        $errors[] = "Loader failed: " . $e->getMessage();
        echo "  Loader FAILED: " . $e->getMessage() . "\n";
    }

    // 3. Test MockBinder dynamic interface mock
    echo "Testing MockBinder...\n";
    interface TestService {
        public function calculate(int $x): int;
        public function checkRegex(string $str): bool;
        public function checkRange(int $val): bool;
        public function checkSubset(array $arr): bool;
        public function mutateObject(object $obj): void;
    }
    $mockBinder = new MockBinder($dto);
    try {
        $dummyTestCase = new class {};
        $mock = $mockBinder->createMockForService(TestService::class, [
            [
                'target' => 'service.calculate',
                'when_args' => [10],
                'returns' => [['return' => 42]],
            ],
            [
                'target' => 'service.checkRegex',
                'when_args' => [['$matcher' => 'regex', 'pattern' => '^[A-Z]{3}$']],
                'returns' => [['return' => true]],
            ],
            [
                'target' => 'service.checkRange',
                'when_args' => [['$matcher' => 'range', 'min' => 10, 'max' => 50]],
                'returns' => [['return' => true]],
            ],
            [
                'target' => 'service.checkSubset',
                'when_args' => [['$matcher' => 'subset', 'subset' => ['role' => 'admin']]],
                'returns' => [['return' => true]],
            ],
            [
                'target' => 'service.mutateObject',
                'mutate_args' => [0 => ['status' => 'verified']],
                'returns' => [['return' => null]],
            ],
        ], $dummyTestCase);

        $result = $mock->calculate(10);
        assert($result === 42);

        assert($mock->checkRegex('ABC') === true);
        assert($mock->checkRange(25) === true);
        assert($mock->checkSubset(['role' => 'admin', 'name' => 'Alice']) === true);

        $obj = (object)['status' => 'pending'];
        $mock->mutateObject($obj);
        assert($obj->status === 'verified');

        echo "  MockBinder dynamic mock test passed\n";
    } catch (\Throwable $e) {
        $errors[] = "MockBinder failed: " . $e->getMessage();
        echo "  MockBinder FAILED: " . $e->getMessage() . "\n";
    }

    // 4. Test TargetInvoker
    echo "Testing TargetInvoker...\n";
    class CalcTest {
        public function add(int $a, int $b): int {
            return $a + $b;
        }
    }
    $invoker = new TargetInvoker($dto);
    try {
        $res = $invoker->invokeTarget(CalcTest::class . '::add', [], ['a' => 3, 'b' => 4]);
        assert($res === 7);
        echo "  TargetInvoker basic test passed\n";
    } catch (\Throwable $e) {
        $errors[] = "TargetInvoker failed: " . $e->getMessage();
        echo "  TargetInvoker FAILED: " . $e->getMessage() . "\n";
    }

    // 5. Test OutcomeAsserter
    echo "Testing OutcomeAsserter...\n";
    $asserter = new OutcomeAsserter($dto);
    $testCase = new class {
        public function assertEquals($expected, $actual) {
            if ($expected !== $actual) {
                throw new \AssertionError("Expected {$expected}, got {$actual}");
            }
        }
    };
    try {
        $asserter->assertOutcome(42, null, ['outcome' => 'return', 'value' => 42], $testCase);
        $asserter->assertOutcome(null, new \InvalidArgumentException("bad arg"), [
            'outcome' => 'exception',
            'exception_type' => 'InvalidArgumentException',
            'message_contains' => 'bad'
        ], $testCase);
        echo "  OutcomeAsserter tests passed\n";
    } catch (\Throwable $e) {
        $errors[] = "OutcomeAsserter failed: " . $e->getMessage();
        echo "  OutcomeAsserter FAILED: " . $e->getMessage() . "\n";
    }

    // 6. Test AutotestHydrator end-to-end execution
    echo "Testing AutotestHydrator end-to-end...\n";
    $hydrator = new AutotestHydrator();
    $file = __DIR__ . '/../../../../conformance/profiles/php/php_constructor_promotion.autotest.json';
    $scenario = [
        'name' => 'promoted_readonly_repo_call',
        'mocks' => [
            ['$ref' => '#/fixtures/mock_tax_repo']
        ],
        'inputs' => ['country' => 'DE'],
        'expected' => [
            'outcome' => 'return',
            'type' => 'float',
            'value' => 0.19
        ]
    ];
    try {
        $hydrator->executeScenario($file, $scenario, $testCase);
        echo "  AutotestHydrator end-to-end test passed\n";

        $tempOutput = tempnam(sys_get_temp_dir(), 'autotest_rec_');
        $hydrator->executeScenario($file, $scenario, $testCase, ['record' => true, 'record_output' => $tempOutput]);
        assert(file_exists($tempOutput));
        $recordedData = json_decode(file_get_contents($tempOutput), true);
        assert($recordedData['outcome'] === 'return');
        assert($recordedData['value'] === 0.19);
        unlink($tempOutput);
        echo "  AutotestHydrator recording test passed\n";

        // 7. Test RecorderHook + DtoHydrator round-trip for immutable private-property objects (like Yiisoft\Router\Route)
        $recorder = new RecorderHook();
        $privateObj = new class('/users', ['HEAD']) {
            public function __construct(private string $pattern, private array $methods) {}
            public function getPattern(): string { return $this->pattern; }
            public function getMethods(): array { return $this->methods; }
        };
        $recordedObj = $recorder->recordOutcome($privateObj, null);
        $jsonRoundTrip = json_decode(json_encode($recordedObj), true);
        $hydratedObj = $dto->hydrate($jsonRoundTrip['value']);
        assert(is_object($hydratedObj));
        assert($hydratedObj->getPattern() === '/users');
        assert($hydratedObj->getMethods() === ['HEAD']);
        echo "  Private-property object record/hydrate round-trip passed\n";
    } catch (\Throwable $e) {
        $errors[] = "AutotestHydrator end-to-end failed: " . $e->getMessage();
        echo "  AutotestHydrator end-to-end FAILED: " . get_class($e) . ": " . $e->getMessage() . "\n";
        echo "  Trace: " . $e->getTraceAsString() . "\n";
    }

    if (!empty($errors)) {
        echo "\n=== ERRORS SUMMARY ===\n";
        foreach ($errors as $err) {
            echo "- " . $err . "\n";
        }
        exit(1);
    } else {
        echo "\nALL PHP RUNTIME TESTS PASSED!\n";
        exit(0);
    }
}
