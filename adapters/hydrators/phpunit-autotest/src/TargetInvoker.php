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
        [$className, $methodName] = explode('::', $targetId);

        if (!class_exists($className)) {
            throw new \RuntimeException("Target class not found: {$className}");
        }

        $refClass = new \ReflectionClass($className);
        $instance = $refClass->newInstanceArgs($constructorArgs);

        if (!$refClass->hasMethod($methodName)) {
            throw new \RuntimeException("Target method {$methodName} not found on {$className}");
        }

        $refMethod = $refClass->getMethod($methodName);
        $callArgs = [];

        foreach ($refMethod->getParameters() as $param) {
            $name = $param->getName();
            if (array_key_exists($name, $inputArgs)) {
                $callArgs[] = $this->dtoHydrator->hydrate(
                    $inputArgs[$name],
                    $param->getType()?->getName()
                );
            } elseif ($param->isDefaultValueAvailable()) {
                $callArgs[] = $param->getDefaultValue();
            } else {
                throw new \InvalidArgumentException("Missing required parameter: {$name}");
            }
        }

        return $refMethod->invokeArgs($instance, $callArgs);
    }
}
