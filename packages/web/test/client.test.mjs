import { test } from 'node:test';
import assert from 'node:assert/strict';
import { Toolbox } from '../index.js';

class FakeWorker {
  stopped = false;
  messages = [];
  postMessage(message, transfers = []) {
    const received = structuredClone(message, { transfer: transfers });
    this.messages.push(received);
    if (message.operation === 'initialize') this.reply(message.id, { ready: true });
    if (message.operation === 'open') this.reply(message.id, { handle: 1 });
    if (message.operation === 'model' && message.payload.method === 'dispose') this.reply(message.id, null);
  }
  reply(id, value) { queueMicrotask(() => this.onmessage?.({ data: { id, type: 'result', value } })); }
  terminate() { this.stopped = true; }
}
function fixture() {
  const worker = new FakeWorker();
  return { worker, toolbox: new Toolbox({ workerFactory: () => worker }) };
}

test('default input ownership is preserved; explicit transfer detaches whole buffers', async () => {
  const { toolbox, worker } = fixture();
  const first = new Uint8Array([1, 2, 3]);
  await toolbox.open(first);
  assert.deepEqual(first, new Uint8Array([1, 2, 3]));
  assert.deepEqual(worker.messages.at(-1).payload.bytes, first);
  const second = new Uint8Array([4, 5, 6]);
  await toolbox.open(second, { transfer: true });
  assert.equal(second.byteLength, 0);
  assert.deepEqual(worker.messages.at(-1).payload.bytes, new Uint8Array([4, 5, 6]));
  const parent = new Uint8Array([9, 8, 7, 6]);
  await toolbox.open(parent.subarray(1, 3), { transfer: true });
  assert.equal(parent.byteLength, 4, 'sliced buffer does not detach unrelated data');
  toolbox.dispose();
});

test('abort terminates synchronous work and rejects every invalidated handle request', async () => {
  const { toolbox, worker } = fixture();
  const model = await toolbox.open(new Uint8Array([1]));
  const controller = new AbortController();
  const first = model.query({}, { signal: controller.signal });
  const second = model.inspect();
  const rejected = Promise.all([assert.rejects(first, { name: 'AbortError' }), assert.rejects(second, { name: 'AbortError' })]);
  await new Promise((resolve) => setTimeout(resolve, 0));
  const active = worker.messages.find((message) => message.operation === 'model' && message.payload.method === 'query');
  worker.onmessage({ data: { id: active.id, type: 'progress', progress: { phase: 'query' } } });
  controller.abort();
  await rejected;
  assert.equal(worker.stopped, true);
  await assert.rejects(model.inspect(), { code: 'worker.closed' });
  await model.dispose();
});

test('already-aborted requests preserve buffers and do not terminate other models', async () => {
  const { toolbox, worker } = fixture();
  await toolbox.ready();
  const controller = new AbortController();
  controller.abort();
  const bytes = new Uint8Array([1]);
  await assert.rejects(toolbox.open(bytes, { transfer: true, signal: controller.signal }), { name: 'AbortError' });
  assert.equal(bytes.byteLength, 1);
  assert.equal(worker.stopped, false);
  toolbox.dispose();
});

test('disposal is idempotent and disposed model rejects later calls', async () => {
  const { toolbox, worker } = fixture();
  const model = await toolbox.open(new Uint8Array([1]));
  await model.dispose();
  await model.dispose();
  assert.equal(worker.messages.filter((m) => m.payload.method === 'dispose').length, 1);
  await assert.rejects(model.inspect(), { code: 'model.disposed' });
  toolbox.dispose();
  toolbox.dispose();
});

test('worker errors reject outstanding operations', async () => {
  const { toolbox, worker } = fixture();
  const model = await toolbox.open(new Uint8Array([1]));
  const result = model.query();
  const rejected = assert.rejects(result, { code: 'worker.failed' });
  await new Promise((resolve) => setTimeout(resolve, 0));
  worker.onerror({ message: 'unreachable trap' });
  await rejected;
  assert.equal(toolbox.closed, true);
});

test('abort during initialization settles immediately without transferring input', async () => {
  const worker = new FakeWorker();
  worker.reply = () => {};
  const toolbox = new Toolbox({ workerFactory: () => worker });
  const controller = new AbortController();
  const bytes = new Uint8Array([1]);
  const operation = toolbox.open(bytes, { signal: controller.signal, transfer: true });
  const rejected = assert.rejects(operation, { name: 'AbortError' });
  controller.abort();
  await rejected;
  assert.equal(bytes.byteLength, 1);
  assert.equal(worker.stopped, true);
});

test('queued cancellation cooperates without invalidating other model handles', async () => {
  const { toolbox, worker } = fixture();
  const model = await toolbox.open(new Uint8Array([1]));
  const controller = new AbortController();
  const operation = model.query({}, { signal: controller.signal });
  const rejected = assert.rejects(operation, { name: 'AbortError' });
  await new Promise((resolve) => setTimeout(resolve, 0));
  controller.abort();
  await rejected;
  assert.equal(worker.stopped, false);
  assert.equal(worker.messages.at(-1).operation, 'cancel');
  await model.dispose();
  toolbox.dispose();
});
