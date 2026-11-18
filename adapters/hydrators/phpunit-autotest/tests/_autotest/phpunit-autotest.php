<?php
declare(strict_types=1);

/**
 * autotest/phpunit - Self-contained single-file offline vendor fallback
 * Dual-licensed under MIT OR Apache-2.0
 */

namespace Autotest\Hydrator;

if (!class_exists('Autotest\Hydrator\AutotestHydrator')) {
    require_once __DIR__ . '/../../src/Loader.php';
    require_once __DIR__ . '/../../src/DtoHydrator.php';
    require_once __DIR__ . '/../../src/MockBinder.php';
    require_once __DIR__ . '/../../src/TargetInvoker.php';
    require_once __DIR__ . '/../../src/OutcomeAsserter.php';
    require_once __DIR__ . '/../../src/RecorderHook.php';
    require_once __DIR__ . '/../../src/AutotestHydrator.php';
}
