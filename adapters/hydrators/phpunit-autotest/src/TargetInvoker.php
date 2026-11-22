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
        array $inputArgs,
        array $setupCalls = [],
        ?array $invokeReturnedArgs = null
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
            $result = $refMethod->invokeArgs(null, $callArgs);
        } else {
            $instance = $this->instantiateClass($refClass, $constructorArgs);
            foreach ($setupCalls as $call) {
                $setupMethod = $call['method'] ?? null;
                if ($setupMethod && $refClass->hasMethod($setupMethod)) {
                    $setupRef = $refClass->getMethod($setupMethod);
                    $setupArgs = [];
                    $rawSetupArgs = $call['args'] ?? [];
                    foreach ($setupRef->getParameters() as $sIdx => $sParam) {
                        if (array_key_exists($sIdx, $rawSetupArgs)) {
                            $sType = $sParam->getType();
                            $sTypeStr = $sType instanceof \ReflectionNamedType ? $sType->getName() : null;
                            $setupArgs[] = $this->dtoHydrator->hydrate($rawSetupArgs[$sIdx], $sTypeStr);
                        } elseif ($sParam->isDefaultValueAvailable()) {
                            $setupArgs[] = $sParam->getDefaultValue();
                        }
                    }
                    $setupRes = $setupRef->invokeArgs($instance, $setupArgs);
                    if (is_object($setupRes) && $setupRes instanceof $className) {
                        $instance = $setupRes;
                    }
                }
            }
            $result = $refMethod->invokeArgs($instance, $callArgs);
        }

        if ($invokeReturnedArgs !== null && is_callable($result)) {
            $retArgs = array_map(fn($a) => $this->dtoHydrator->hydrate($a), $invokeReturnedArgs);
            $result = $result(...$retArgs);
        }

        return $result;
    }

    private function instantiateClass(\ReflectionClass $refClass, array $constructorArgs): object
    {
        $ctor = $refClass->getConstructor();
        if ($ctor === null) {
            return $refClass->newInstance();
        }

        $resolvedArgs = [];
        $idx = 0;
        foreach ($ctor->getParameters() as $param) {
            if (array_key_exists($idx, $constructorArgs)) {
                $resolvedArgs[] = $constructorArgs[$idx];
            } elseif ($param->isDefaultValueAvailable()) {
                $resolvedArgs[] = $param->getDefaultValue();
            } elseif ($param->allowsNull()) {
                $resolvedArgs[] = null;
            } else {
                $resolvedArgs[] = $this->defaultValueForParam($param);
            }
            $idx++;
        }

        if ($ctor->isPublic()) {
            return $refClass->newInstanceArgs($resolvedArgs);
        }

        // Private/protected constructor (immutable value objects like Route, Group, MatchingResult)
        $instance = $refClass->newInstanceWithoutConstructor();
        $ctor->invokeArgs($instance, $resolvedArgs);
        return $instance;
    }

    private function defaultValueForParam(\ReflectionParameter $param): mixed
    {
        $type = $param->getType();
        if ($type instanceof \ReflectionNamedType) {
            $name = $type->getName();
            return match ($name) {
                'string' => '/test',
                'int' => 1,
                'float' => 1.0,
                'bool' => false,
                'array' => ['GET'],
                'iterable' => [],
                'object' => new \stdClass(),
                default => class_exists($name) ? (new \ReflectionClass($name))->newInstanceWithoutConstructor() : null,
            };
        }
        return null;
    }
}
