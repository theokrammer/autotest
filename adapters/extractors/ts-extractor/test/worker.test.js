const assert = require('assert');
const { Worker } = require('../src/Worker');

const worker = new Worker();

// Test 1: Ping
const pingRes = worker.handleRequest({ jsonrpc: '2.0', id: 1, method: 'ping' });
assert.strictEqual(pingRes.result.status, 'pong');
assert.strictEqual(pingRes.id, 1);

// Test 2: Extraction
const sampleTs = `
export class OrderService {
  constructor(public readonly gateway: PaymentGateway, private retries: number) {}

  public async charge(orderId: string, amount: number): Promise<boolean> {
    if (amount <= 0) {
      process.exit(1);
    }
    return true;
  }
}
`;

const extractRes = worker.handleRequest({
  jsonrpc: '2.0',
  id: 2,
  method: 'extract_file',
  params: {
    file_path: 'src/order.ts',
    source_content: sampleTs
  }
});

assert.strictEqual(extractRes.id, 2);
const r = extractRes.result;
assert.strictEqual(r.file, 'src/order.ts');
assert(r.impl_hash.startsWith('sha256:'));
assert(r.public_contract_hash.startsWith('contract_sha256:'));
assert.strictEqual(r.targets.length, 1);
assert.strictEqual(r.targets[0].id, 'OrderService.charge');
assert.strictEqual(r.targets[0].signature.constructor_args.length, 2);
assert.strictEqual(r.targets[0].signature.parameters.length, 2);
assert.strictEqual(r.targets[0].detected_side_effects[0].effect_type, 'unmockable_side_effect');
assert(r.dependencies['PaymentGateway'] !== undefined);

console.log('All ts-extractor worker tests passed!');
