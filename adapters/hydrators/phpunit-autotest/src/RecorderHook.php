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

        return [
            'outcome' => 'return',
            'type' => is_object($actualReturn) ? get_class($actualReturn) : gettype($actualReturn),
            'value' => $this->serializeValue($actualReturn),
            'expected_events' => [],
        ];
    }

    private function serializeValue(mixed $val): mixed
    {
        if (is_scalar($val) || $val === null) {
            return $val;
        }

        if (is_array($val)) {
            return array_map([$this, 'serializeValue'], $val);
        }

        if ($val instanceof \DateTimeInterface) {
            return [
                '$type' => 'DateTimeImmutable',
                'value' => $val->format(\DateTimeInterface::ATOM)
            ];
        }

        if (is_object($val)) {
            $class = get_class($val);
            $props = [];
            $ref = new \ReflectionClass($val);
            foreach ($ref->getProperties(\ReflectionProperty::IS_PUBLIC) as $prop) {
                $props[$prop->getName()] = $this->serializeValue($prop->getValue($val));
            }
            return [
                '$type' => $class,
                '$props' => $props
            ];
        }

        return (string)$val;
    }
}
