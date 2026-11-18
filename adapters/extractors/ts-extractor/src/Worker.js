const crypto = require('crypto');

class Worker {
  constructor() {
    this.ts = null;
    try {
      this.ts = require('typescript');
    } catch (_) {
      // Fallback parser if typescript module is not installed globally/locally
    }
  }

  handleRequest(request) {
    const id = request.id !== undefined ? request.id : null;
    const method = request.method || '';
    const params = request.params || {};

    if (method === 'ping') {
      return {
        jsonrpc: '2.0',
        id,
        result: { status: 'pong', version: '1.0.0', runtime: 'node-' + process.version }
      };
    }

    if (method === 'extract_file') {
      const filePath = params.file_path || '';
      const source = params.source_content || '';

      try {
        const result = this.analyzeSource(filePath, source);
        return {
          jsonrpc: '2.0',
          id,
          result
        };
      } catch (err) {
        return {
          jsonrpc: '2.0',
          id,
          error: {
            code: -32603,
            message: 'Extraction error: ' + (err.message || String(err))
          }
        };
      }
    }

    return {
      jsonrpc: '2.0',
      id,
      error: {
        code: -32601,
        message: 'Method not found: ' + method
      }
    };
  }

  analyzeSource(filePath, source) {
    const implHash = 'sha256:' + crypto.createHash('sha256').update(source).digest('hex');

    // Extract signatures and public contracts
    const dependencies = {};
    const targets = [];
    const diagnostics = [];

    // Simple robust AST/token extraction
    const lines = source.split('\n');
    let currentClass = null;
    let constructorArgs = [];
    let publicProps = [];
    let braceDepth = 0;

    for (let i = 0; i < lines.length; i++) {
      const line = lines[i].trim();

      // Class match
      const classMatch = line.match(/(?:export\s+)?class\s+(\w+)/);
      if (classMatch) {
        currentClass = classMatch[1];
      }

      // Constructor parameter properties match
      const ctorMatch = line.match(/constructor\s*\(([^)]*)\)/);
      if (ctorMatch && currentClass) {
        const rawParams = ctorMatch[1].split(',');
        constructorArgs = [];
        for (const raw of rawParams) {
          const trimmed = raw.trim();
          if (!trimmed) continue;
          const clean = trimmed.replace(/^(public|private|protected|readonly)\s+/, '');
          const parts = clean.split(':');
          const pName = parts[0].trim().replace(/\?$/, '');
          const pType = parts[1] ? parts[1].split('=')[0].trim() : 'any';

          constructorArgs.push({
            name: pName,
            param_type: pType,
            constraints: pType === 'number' ? ['finite'] : (pType === 'string' ? ['non-empty'] : []),
            default_value: null
          });

          if (!['string', 'number', 'boolean', 'any', 'unknown'].includes(pType)) {
            dependencies[pType] = 'contract_sha256:discovered';
          }
        }
      }

      // Method match
      const methodMatch = line.match(/(?:public\s+|static\s+|async\s+)*(\w+)\s*\(([^)]*)\)\s*(?::\s*([^\{]+))?\s*\{/);
      if (methodMatch && currentClass) {
        const methodName = methodMatch[1];
        if (!['if', 'for', 'while', 'switch', 'catch', 'constructor'].includes(methodName) && !methodName.startsWith('_')) {
          const isStatic = line.includes('static ');
          const rawParams = methodMatch[2].split(',');
          const params = [];
          for (const raw of rawParams) {
            const trimmed = raw.trim();
            if (!trimmed) continue;
            const parts = trimmed.split(':');
            const pName = parts[0].trim().replace(/\?$/, '');
            const pType = parts[1] ? parts[1].split('=')[0].trim() : 'any';
            params.push({
              name: pName,
              param_type: pType,
              constraints: pType === 'number' ? ['finite'] : (pType === 'string' ? ['non-empty'] : []),
              default_value: null
            });
          }

          const returnType = methodMatch[3] ? methodMatch[3].trim() : null;

          // Side effect checks
          const sideEffects = [];
          if (source.includes('process.exit')) {
            sideEffects.push({
              effect_type: 'unmockable_side_effect',
              target: 'process.exit',
              mockable: false
            });
          }

          targets.push({
            id: `${currentClass}.${methodName}`,
            kind: isStatic ? 'function' : 'method',
            signature: {
              constructor_args: constructorArgs,
              parameters: params,
              return_type: returnType
            },
            detected_side_effects: sideEffects,
            scenarios: []
          });
        }
      }
    }

    const contractSignature = targets.map(t => `${t.id}(${t.signature.parameters.map(p => p.name + ':' + p.param_type).join(',')})`).join(';');
    const publicContractHash = 'contract_sha256:' + crypto.createHash('sha256').update(contractSignature).digest('hex');

    return {
      file: filePath,
      impl_hash: implHash,
      public_contract_hash: publicContractHash,
      dependencies,
      targets,
      diagnostics
    };
  }
}

module.exports = { Worker };
