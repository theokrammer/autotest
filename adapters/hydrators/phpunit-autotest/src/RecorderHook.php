<?php
declare(strict_types=1);

namespace Autotest\Hydrator;

final class RecorderHook
{
    public function recordOutcome(mixed $actualReturn, ?\Throwable $actualException, bool $nonDeterministic = false): array
    {
        if ($actualException !== null) {
            // Strip runner call-site file paths (", called in /tmp/autotest-php-runner-...") from TypeError/ArgumentCountError
            $msg = (string)preg_replace('/, called in .+$/s', '', $actualException->getMessage());
            return [
                'outcome' => 'exception',
                'exception_type' => get_class($actualException),
                'message_contains' => $msg,
            ];
        }

        if ($nonDeterministic || $this->isNonDeterministicOrOpaqueReturn($actualReturn)) {
            $type = is_object($actualReturn)
                ? (str_contains(get_class($actualReturn), '@anonymous') ? 'object' : get_class($actualReturn))
                : gettype($actualReturn);
            return [
                'outcome' => 'return',
                'type' => $type,
                'expected_events' => [],
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

    private function isNonDeterministicOrOpaqueReturn(mixed $val): bool
    {
        if ($val instanceof \Generator) {
            return true;
        }
        if (is_object($val) && str_contains(get_class($val), '@anonymous')) {
            $parent = get_parent_class($val);
            if ($parent !== false && is_a($val, \Traversable::class)) {
                return true;
            }
        }
        if (is_string($val)) {
            // Non-UTF-8 binary strings (e.g. raw 16-byte UUID bytes)
            if (!preg_match('//u', $val)) {
                return true;
            }
            // UUID v1..v8, 32-char hex session IDs (create_sid), or wall-clock HTTP Date / Max-Age cookie strings
            if (
                preg_match('/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i', $val)
                || preg_match('/^[0-9a-f]{32}$/i', $val)
            ) {
                return true;
            }
            if (
                str_contains($val, 'Max-Age=')
                || str_contains($val, gmdate('D, d M Y'))
                || str_contains($val, 'REQUEST_TIME')
                || str_contains($val, '/tmp/.tmp')
                || str_contains($val, 'Proxy: http://127.0.0.1:')
                || str_contains($val, 'autotest-php-runner-')
            ) {
                return true;
            }
        }
        if (is_object($val) || is_array($val)) {
            $json = @json_encode($this->serializeValue($val), JSON_UNESCAPED_SLASHES);
            // If json_encode fails (e.g. JSON_ERROR_UTF8 on binary Fields::$bytes inside Uuid), treat as opaque type-only
            if (!is_string($json) || $json === '') {
                return true;
            }
            $todayDay = gmdate('D, d M Y');
            $todayIso = gmdate('Y-m-d\TH:i:');
            if (
                str_contains($json, $todayDay)
                || str_contains($json, $todayIso)
                || str_contains($json, 'REQUEST_TIME')
                || str_contains($json, '/tmp/.tmp')
                || str_contains($json, 'autotest-php-runner-')
                || str_contains($json, 'Proxy: http://127.0.0.1:')
                || str_contains($json, 'session.save_handler')
                || str_contains($json, 'SessionHandlerProxy')
            ) {
                return true;
            }
            // Check for current epoch second integers (within +-600s of time(), e.g. SystemTimeProvider::getTime() or ServerBag)
            $now = time();
            if (preg_match_all('/\b(1[7-9]\d{8})\b/', $json, $matches)) {
                foreach ($matches[1] as $numStr) {
                    if (abs($now - (int)$numStr) < 600) {
                        return true;
                    }
                }
            }
        }
        return false;
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

        if ($val instanceof \Closure || $val instanceof \Generator) {
            return ['$type' => 'ClosureMarker'];
        }

        if ($val instanceof \UnitEnum) {
            return [
                '$type' => get_class($val),
                '$props' => (object)['name' => $val->name],
            ];
        }

        if (is_object($val)) {
            $class = get_class($val);
            if (
                $this->detectMockInterface($val) !== null
                || str_starts_with($class, 'Autotest\\Hydrator\\')
                || str_starts_with($class, 'PHPUnit\\')
            ) {
                return ['$type' => 'ClosureMarker'];
            }
            $skipProps = ['__specs' => true, '__binder' => true, '__depth' => true];
            if ($val instanceof \Throwable) {
                $skipProps['trace'] = true;
                $skipProps['previous'] = true;
                $skipProps['file'] = true;
                $skipProps['line'] = true;
            }
            $props = [];
            foreach (get_object_vars($val) as $k => $v) {
                if (isset($skipProps[$k]) || str_starts_with((string)$k, '__phpunit')) {
                    continue;
                }
                $props[$k] = $this->serializeValue($v, $depth + 1);
            }
            $ref = new \ReflectionClass($val);
            while ($ref !== false) {
                foreach ($ref->getProperties() as $prop) {
                    if ($prop->isStatic()) {
                        continue;
                    }
                    $name = $prop->getName();
                    if (array_key_exists($name, $props) || isset($skipProps[$name]) || str_starts_with($name, '__phpunit')) {
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
