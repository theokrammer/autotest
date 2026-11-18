<?php
declare(strict_types=1);

namespace Tests;

use PHPUnit\Framework\TestCase;
use PHPUnit\Framework\Attributes\DataProvider;
use Autotest\Hydrator\AutotestHydrator;

final class AutotestBridgeTest extends TestCase
{
    #[DataProvider('scenarioProvider')]
    public function testScenario(string $file, array $scenario): void
    {
        $hydrator = new AutotestHydrator();
        $hydrator->executeScenario($file, $scenario, $this);
    }

    public static function scenarioProvider(): iterable
    {
        $dir = __DIR__ . '/../tests/scenarios';
        if (!is_dir($dir)) {
            return [];
        }

        $iterator = new \RecursiveIteratorIterator(
            new \RecursiveDirectoryIterator($dir, \FilesystemIterator::SKIP_DOTS)
        );

        foreach ($iterator as $item) {
            if ($item->isFile() && str_ends_with($item->getFilename(), '.autotest.json')) {
                $data = json_decode(file_get_contents($item->getPathname()), true, 512, JSON_THROW_ON_ERROR);
                foreach ($data['targets'] ?? [] as $target) {
                    foreach ($target['scenarios'] ?? [] as $scenario) {
                        $name = sprintf('%s :: %s', $target['id'], $scenario['name']);
                        yield $name => [$item->getPathname(), $scenario];
                    }
                }
            }
        }
    }
}
