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
        ?array $invokeReturnedArgs = null,
        ?array $factorySpec = null
    ): mixed {
        set_error_handler(
            static fn() => true,
            E_WARNING | E_NOTICE | E_USER_WARNING | E_USER_NOTICE | E_DEPRECATED | E_USER_DEPRECATED
        );
        try {
            return $this->doInvokeTarget(
                $targetId,
                $constructorArgs,
                $inputArgs,
                $setupCalls,
                $invokeReturnedArgs,
                $factorySpec
            );
        } finally {
            restore_error_handler();
        }
    }

    private function doInvokeTarget(
        string $targetId,
        array $constructorArgs,
        array $inputArgs,
        array $setupCalls = [],
        ?array $invokeReturnedArgs = null,
        ?array $factorySpec = null
    ): mixed {
        // Standalone namespaced PHP functions (no class separator in the target id)
        if (!str_contains($targetId, '::') && !str_contains($targetId, '.')) {
            if (!function_exists($targetId)) {
                throw new \RuntimeException("Target function not found: {$targetId}");
            }
            $refFn = new \ReflectionFunction($targetId);
            $fnArgs = $this->dtoHydrator->hydrateMethodArgs($refFn, $inputArgs);
            $fnResult = $refFn->invokeArgs($fnArgs);
            if ($invokeReturnedArgs !== null && is_callable($fnResult)) {
                $retArgs = array_map(fn($a) => $this->dtoHydrator->hydrate($a), $invokeReturnedArgs);
                $fnResult = $fnResult(...$retArgs);
            }
            return $fnResult;
        }

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
        $callArgs = $this->dtoHydrator->hydrateMethodArgs($refMethod, $inputArgs);

        if ($refMethod->isStatic()) {
            $result = $refMethod->invokeArgs(null, $callArgs);
        } else {
            $instance = null;
            if ($factorySpec !== null && !empty($factorySpec['method']) && $refClass->hasMethod($factorySpec['method'])) {
                $factoryMethod = $refClass->getMethod($factorySpec['method']);
                if ($factoryMethod->isStatic()) {
                    $factoryArgs = $this->dtoHydrator->hydrateMethodArgs($factoryMethod, $factorySpec['args'] ?? []);
                    $instance = $factoryMethod->invokeArgs(null, $factoryArgs);
                }
            }
            if ($instance === null) {
                $instance = $this->instantiateClass($refClass, $constructorArgs);
            }
            if (!empty($setupCalls)) {
                $instance = $this->dtoHydrator->replaySetupCalls($instance, $setupCalls);
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
                default => $this->dtoHydrator->hydrate([], $name),
            };
        }
        return null;
    }
}
