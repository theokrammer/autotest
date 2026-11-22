<?php
declare(strict_types=1);

namespace Autotest\Hydrator;

final class RecorderHook
{
    public function recordOutcome(mixed $actualReturn, ?\Throwable $actualException): array
    {
        if ($actualException !== null) {
            return [
                'outcome' => 'exception',
                'exception_type' => get_class($actualException),
                'message_contains' => $actualException->getMessage(),
            ];
        }

        if (is_object($actualReturn)) {
            $mockIface = $this->detectMockInterface($actualReturn);
            if ($mockIface !== null) {
                return [
                    'outcome' => 'return',
                    'type' => $mockIface,
                    'expected_events' => [],
                ];
            }
        }

        return [
            'outcome' => 'return',
            'type' => is_object($actualReturn) ? get_class($actualReturn) : gettype($actualReturn),
            'value' => $this->serializeValue($actualReturn),
            'expected_events' => [],
        ];
    }

    private function detectMockInterface(object $obj): ?string
    {
        $cls = get_class($obj);
        $isDynamicMock = str_starts_with($cls, 'AutotestMock_')
            || str_starts_with($cls, 'MockObject_');
        if (!$isDynamicMock) {
            return null;
        }
        $interfaces = class_implements($obj) ?: [];
        foreach ($interfaces as $iface) {
            if (!str_starts_with($iface, 'PHPUnit\\')) {
                return $iface;
            }
        }
        $parent = get_parent_class($obj);
        if ($parent !== false) {
            return $parent;
        }
        return 'object';
    }

    public function serializeValue(mixed $val, int $depth = 0): mixed
    {
        if (is_scalar($val) || $val === null) {
            return $val;
        }

        if ($depth > 8) {
            return null;
        }

        if (is_array($val)) {
            $out = [];
            foreach ($val as $k => $v) {
                $out[$k] = $this->serializeValue($v, $depth + 1);
            }
            return $out;
        }

        if ($val instanceof \DateTimeInterface) {
            return [
                '$type' => 'DateTimeImmutable',
                'value' => $val->format(\DateTimeInterface::ATOM)
            ];
        }

        if ($val instanceof \Closure) {
            return ['$type' => 'ClosureMarker'];
        }

        if (is_object($val)) {
            $class = get_class($val);
            $props = [];
            foreach (get_object_vars($val) as $k => $v) {
                $props[$k] = $this->serializeValue($v, $depth + 1);
            }
            $ref = new \ReflectionClass($val);
            while ($ref !== false) {
                foreach ($ref->getProperties() as $prop) {
                    if ($prop->isStatic()) {
                        continue;
                    }
                    $name = $prop->getName();
                    if (array_key_exists($name, $props)) {
                        continue;
                    }
                    if (!$prop->isInitialized($val)) {
                        continue;
                    }
                    $props[$name] = $this->serializeValue($prop->getValue($val), $depth + 1);
                }
                $ref = $ref->getParentClass();
            }
            return [
                '$type' => $class,
                '$props' => (object)$props
            ];
        }

        return (string)$val;
    }
}
