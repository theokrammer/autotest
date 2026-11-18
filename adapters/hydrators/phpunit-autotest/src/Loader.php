<?php
declare(strict_types=1);

namespace Autotest\Hydrator;

final class Loader
{
    public function loadScenarioFile(string $filePath): array
    {
        if (!file_exists($filePath)) {
            throw new \RuntimeException("Scenario file not found: {$filePath}");
        }

        $content = file_get_contents($filePath);
        $data = json_decode($content, true, 512, JSON_THROW_ON_ERROR);

        return $this->resolveReferences($data);
    }

    public function resolveReferences(array $data): array
    {
        $fixtures = $data['fixtures'] ?? [];

        if (isset($data['targets'])) {
            foreach ($data['targets'] as &$target) {
                if (isset($target['scenarios'])) {
                    foreach ($target['scenarios'] as &$scenario) {
                        if (isset($scenario['mocks'])) {
                            foreach ($scenario['mocks'] as &$mock) {
                                if (isset($mock['$ref'])) {
                                    $ref = $mock['$ref'];
                                    $key = str_replace('#/fixtures/', '', $ref);
                                    if (isset($fixtures[$key])) {
                                        $mock = $fixtures[$key];
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        return $data;
    }
}
