const assert = require('assert');
const fs = require('fs');
const path = require('path');
const { AutotestHydrator, Loader, DtoHydrator, MockBinder, OutcomeAsserter, RecorderHook, executeAutotestScenario } = require('../src/index');

// === Test DtoHydrator ===

const dtoHydrator = new DtoHydrator();

// Scalar passthrough
assert.strictEqual(dtoHydrator.hydrate(42), 42);
assert.strictEqual(dtoHydrator.hydrate('hello'), 'hello');
assert.strictEqual(dtoHydrator.hydrate(null), null);

// Date codec
const date = dtoHydrator.hydrate({ $type: 'Date', value: '2024-01-01T00:00:00Z' });
assert(date instanceof Date);
assert.strictEqual(date.toISOString(), '2024-01-01T00:00:00.000Z');

// Constructor + property fallback
class TestUser {
  constructor(name, age) {
    this.name = name;
    this.age = age;
    this.role = 'guest';
  }
}

const user = dtoHydrator.hydrate(
  { $type: 'TestUser', $ctor: ['Alice', 30], $props: { role: 'admin' } },
  { TestUser }
);
assert.strictEqual(user.name, 'Alice');
assert.strictEqual(user.age, 30);
assert.strictEqual(user.role, 'admin');
assert(user instanceof TestUser);

// Static factory
class Money {
  constructor(amount, currency) {
    this.amount = amount;
    this.currency = currency;
  }
  static fromCents(cents) {
    return new Money(cents / 100, 'EUR');
  }
}

const money = dtoHydrator.hydrate(
  { $type: 'Money', $factory: 'fromCents', $ctor: [1500] },
  { Money }
);
assert(money instanceof Money);
assert.strictEqual(money.amount, 15);

console.log('  DtoHydrator tests passed');

// === Test MockBinder ===

const mockBinder = new MockBinder(dtoHydrator);

// Sequential returns
const { mock, verify } = mockBinder.createMock([
  {
    target: 'gateway.charge',
    when_args: [{ $matcher: 'any' }, { $matcher: 'gt', value: 0 }],
    returns: [
      { value: { success: true } },
      { value: { success: false } },
    ],
    expect_calls: 2,
  }
]);

const r1 = mock.charge('order-1', 100);
assert.deepStrictEqual(r1, { success: true });
const r2 = mock.charge('order-2', 50);
assert.deepStrictEqual(r2, { success: false });
verify(); // Should not throw

// Argument matcher: gt violation
assert.throws(() => {
  mockBinder.createMock([
    { target: 'x.y', when_args: [{ $matcher: 'gt', value: 10 }], returns: [{ value: true }] }
  ]).mock.y(5);
}, /Expected > 10/);

// Call count violation
const { mock: m2, verify: v2 } = mockBinder.createMock([
  { target: 'x.call', returns: [], expect_calls: 1 }
]);
assert.throws(() => v2(), /expected 1 call/);

console.log('  MockBinder tests passed');

// === Test OutcomeAsserter ===

const outcomeAsserter = new OutcomeAsserter(dtoHydrator);

// Value assertion
outcomeAsserter.assert({ result: 42, error: null }, { value: 42 }, {}, null);

// Exception assertion
outcomeAsserter.assert(
  { result: null, error: new TypeError('bad') },
  { exception: { type: 'TypeError', message: 'bad' } },
  { TypeError },
  null
);

// Should throw when exception expected but not thrown
assert.throws(() => {
  outcomeAsserter.assert({ result: 'ok', error: null }, { exception: { type: 'Error' } }, {}, null);
}, /Expected exception/);

// Should rethrow when no exception expected but one occurred
assert.throws(() => {
  outcomeAsserter.assert({ result: null, error: new Error('oops') }, { value: 42 }, {}, null);
}, /oops/);

// Properties assertion
outcomeAsserter.assert(
  { result: { a: 1, b: 'x', c: true }, error: null },
  { properties: { a: 1, b: 'x' } },
  {},
  null
);

console.log('  OutcomeAsserter tests passed');

// === Test RecorderHook ===

const recorder = new RecorderHook();
const tmpDir = fs.mkdtempSync(path.join(require('os').tmpdir(), 'autotest-'));
const testScenarioPath = path.join(tmpDir, 'test.autotest.json');
const testStagingPath = path.join(tmpDir, 'test.autotest.staging.json');

// Write a base scenario file
fs.writeFileSync(testScenarioPath, JSON.stringify({
  schema_version: '1.0',
  source: { file: 'test.ts', content_hash: 'sha256:abc', analyzer: 'test', dependencies: {} },
  fixtures: {},
  targets: [
    {
      id: 'Calc.add',
      kind: 'method',
      signature: { constructor_args: [], parameters: [], return_type: 'number' },
      detected_side_effects: [],
      scenarios: [{ name: 'basic_add', inputs: [1, 2], expected: { value: 3 } }]
    }
  ]
}, null, 2));

// Record an outcome
recorder.recordOutcome(testScenarioPath, 'Calc.add', 'recorded_add', { result: 5, error: null });
const flushed = recorder.flush();
assert.strictEqual(flushed.length, 1);
assert(fs.existsSync(testStagingPath));

const staged = JSON.parse(fs.readFileSync(testStagingPath, 'utf8'));
assert.strictEqual(staged.length, 1);
assert.strictEqual(staged[0].scenario, 'recorded_add');
assert.strictEqual(staged[0].expected.value, 5);
assert.strictEqual(staged[0].mode, 'recorded');

// Merge back
const merged = recorder.mergeVerifiedStaging(testStagingPath);
assert.strictEqual(merged, true);
assert(!fs.existsSync(testStagingPath));

const finalDoc = JSON.parse(fs.readFileSync(testScenarioPath, 'utf8'));
assert.strictEqual(finalDoc.targets[0].scenarios.length, 2);
const recordedScenario = finalDoc.targets[0].scenarios.find(s => s.name === 'recorded_add');
assert.strictEqual(recordedScenario.mode, 'recorded');
assert.strictEqual(recordedScenario.expected.value, 5);

// Cleanup
fs.rmSync(tmpDir, { recursive: true, force: true });

console.log('  RecorderHook tests passed');

// === Test Loader with $ref resolution ===

const loader = new Loader();
const tmpDir2 = fs.mkdtempSync(path.join(require('os').tmpdir(), 'autotest-'));
const refPath = path.join(tmpDir2, 'ref.autotest.json');
fs.writeFileSync(refPath, JSON.stringify({
  schema_version: '1.0',
  source: { file: 'test.ts', content_hash: 'sha256:abc', analyzer: 'test', dependencies: {} },
  fixtures: { default_user: { name: 'Alice', age: 30 } },
  targets: [
    {
      id: 'UserService.get',
      kind: 'method',
      signature: { constructor_args: [], parameters: [], return_type: null },
      detected_side_effects: [],
      scenarios: [
        { name: 'get_default', inputs: [], expected: { value: { $ref: '#/fixtures/default_user' } } }
      ]
    }
  ]
}, null, 2));

const resolved = loader.loadScenarioFile(refPath);
const expectedVal = resolved.targets[0].scenarios[0].expected.value;
assert.deepStrictEqual(expectedVal, { name: 'Alice', age: 30 });

fs.rmSync(tmpDir2, { recursive: true, force: true });

console.log('  Loader $ref resolution tests passed');

// === Test executeAutotestScenario ===
(async () => {
  const tmpDir3 = fs.mkdtempSync(path.join(require('os').tmpdir(), 'autotest-exec-'));
  const scPath = path.join(tmpDir3, 'calc.autotest.json');
  fs.writeFileSync(scPath, JSON.stringify({
    schema_version: '1.0',
    source: { file: 'calc.ts', content_hash: 'sha256:123', analyzer: 'test', dependencies: {} },
    fixtures: {},
    targets: [
      {
        id: 'MathService.multiply',
        kind: 'method',
        signature: { constructor_args: [], parameters: [{ name: 'a', type: 'number' }, { name: 'b', type: 'number' }], return_type: 'number' },
        detected_side_effects: [],
        scenarios: [
          { name: 'multiply_positive', inputs: [6, 7], expected: { value: 42 } }
        ]
      }
    ]
  }, null, 2));

  class MathService {
    multiply(a, b) {
      return a * b;
    }
  }

  const scenarioSpec = { name: 'multiply_positive', inputs: [6, 7], expected: { value: 42 } };
  await executeAutotestScenario(scPath, scenarioSpec, {
    classRegistry: { MathService }
  });

  // Test recording with executeAutotestScenario
  await executeAutotestScenario(scPath, { name: 'multiply_positive', inputs: [6, 7], expected: { value: 42 } }, {
    classRegistry: { MathService },
    record: true
  });

  const stagedPath = path.join(tmpDir3, 'calc.autotest.staging.json');
  assert(fs.existsSync(stagedPath), 'Staged file must be created in record mode');
  const stagedContent = JSON.parse(fs.readFileSync(stagedPath, 'utf8'));
  assert.strictEqual(stagedContent.length, 1);
  assert.strictEqual(stagedContent[0].expected.value, 42);

  fs.rmSync(tmpDir3, { recursive: true, force: true });
  console.log('  executeAutotestScenario bridge tests passed');

  // === Test recursive Proxy mock (await safety) & DecoratedClass ===
  const { mock: recMock } = mockBinder.createMock([], {}, { recursive: true });
  assert.strictEqual(recMock.isValid(), false);
  assert.deepStrictEqual(recMock.getAllItems(), []);
  assert(recMock.getModules() instanceof Map);
  assert.strictEqual(recMock.setPrefix('/api'), recMock);
  const awaitedProxy = await recMock;
  assert.strictEqual(awaitedProxy, recMock);

  const decorated = dtoHydrator.hydrate({
    $type: 'DecoratedClass',
    kind: 'class',
    metadata: { path: '/cats' },
  });
  assert.strictEqual(typeof decorated, 'function');
  assert.strictEqual(new decorated().handle(), 'ok');
  console.log('  Recursive Proxy + DecoratedClass tests passed');

  console.log('\nAll @autotest/jest hydrator tests passed!');
})();
