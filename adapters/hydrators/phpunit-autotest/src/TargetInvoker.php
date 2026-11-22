<?php
declare(strict_types=1);

namespace Autotest\Hydrator;

final class TargetInvoker
{
    private DtoHydrator $dtoHydrator;

    public function __construct(DtoHydrator $dtoHydrator)
    {
        $this->dtoHydrator = $dtoHydrator;
    }

    public function invokeTarget(
        string $targetId,
        array $constructorArgs,
        array $inputArgs
    ): mixed {
        if (str_contains($targetId, '::')) {
            [$className, $methodName] = explode('::', $targetId, 2);
        } else {
            [$className, $methodName] = explode('.', $targetId, 2);
        }

        if (!class_exists($className)) {
            throw new \RuntimeException("Target class not found: {$className}");
        }

        $refClass = new \ReflectionClass($className);
        if (!$refClass->hasMethod($methodName)) {
            throw new \RuntimeException("Target method {$methodName} not found on {$className}");
        }

        $refMethod = $refClass->getMethod($methodName);
        $callArgs = [];
        $idx = 0;

        foreach ($refMethod->getParameters() as $param) {
            $name = $param->getName();
            $typeStr = null;
            if ($param->hasType()) {
                $type = $param->getType();
                if ($type instanceof \ReflectionNamedType) {
                    $typeStr = $type->getName();
                } else {
                    $typeStr = (string)$type;
                }
            }

            if (array_key_exists($name, $inputArgs)) {
                $callArgs[] = $this->dtoHydrator->hydrate($inputArgs[$name], $typeStr);
            } elseif (array_key_exists($idx, $inputArgs)) {
                $callArgs[] = $this->dtoHydrator->hydrate($inputArgs[$idx], $typeStr);
            } elseif ($param->isDefaultValueAvailable()) {
                $callArgs[] = $param->getDefaultValue();
            } else {
                throw new \InvalidArgumentException("Missing required parameter: {$name}");
            }
            $idx++;
        }

        if ($refMethod->isStatic()) {
            return $refMethod->invokeArgs(null, $callArgs);
        }

        $instance = $refClass->newInstanceArgs($constructorArgs);
        return $refMethod->invokeArgs($instance, $callArgs);
    }
}
