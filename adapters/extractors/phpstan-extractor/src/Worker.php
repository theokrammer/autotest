<?php
declare(strict_types=1);

namespace Autotest\PhpStanExtractor;

final class Worker
{
    public function handleRequest(array $request): array
    {
        $id = $request['id'] ?? null;
        $method = $request['method'] ?? '';
        $params = $request['params'] ?? [];

        if ($method === 'ping') {
            return [
                'jsonrpc' => '2.0',
                'id' => $id,
                'result' => ['status' => 'pong', 'version' => '1.0.0']
            ];
        }

        if ($method === 'extract_file') {
            $filePath = $params['file_path'] ?? '';
            $source = $params['source_content'] ?? '';

            $result = $this->analyzeSource($filePath, $source);
            return [
                'jsonrpc' => '2.0',
                'id' => $id,
                'result' => $result
            ];
        }

        return [
            'jsonrpc' => '2.0',
            'id' => $id,
            'error' => [
                'code' => -32601,
                'message' => 'Method not found: ' . $method
            ]
        ];
    }

    private function analyzeSource(string $filePath, string $source): array
    {
        $implHash = 'sha256:' . hash('sha256', $source);
        $contractHash = 'contract_sha256:' . hash('sha256', preg_replace('/\{.*\}/s', '', $source));

        return [
            'file' => $filePath,
            'impl_hash' => $implHash,
            'public_contract_hash' => $contractHash,
            'dependencies' => [],
            'targets' => [],
            'diagnostics' => []
        ];
    }
}
