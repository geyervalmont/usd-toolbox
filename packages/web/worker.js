let wasm;
let wasmMemory;
let nextHandle = 1;
const models = new Map();
const requests = new Set();
const cancelled = new Set();
const starts = new Map();
const yieldTask = () => new Promise((resolve) => setTimeout(resolve, 0));
const allowed = new Set(['inspect', 'diagnostics', 'query', 'get_node', 'get_metadata', 'get_property', 'children', 'ancestors', 'descendants', 'ids_for_path', 'count', 'summary', 'update', 'export', 'export_lods', 'assets', 'extract', 'dispose']);

function progress(id, phase) { postMessage({ id, type: 'progress', progress: { phase } }); }
function errorPayload(error) {
  return { message: String(error?.message ?? error), code: error?.code ?? 'toolbox.operation', details: error?.details };
}

async function execute({ id, operation, payload }) {
  const checkCancelled = () => { if (cancelled.has(id)) throw new DOMException('Queued operation aborted.', 'AbortError'); };
  checkCancelled();
  const permitted = await new Promise((resolve) => {
    starts.set(id, resolve);
    progress(id, 'queued');
    postMessage({ id, type: 'ready' });
  });
  starts.delete(id);
  if (!permitted) throw new DOMException('Queued operation aborted.', 'AbortError');
  // Yield between phases; synchronous Rust stages are interruptible by terminating
  // this worker, which the host does on AbortSignal. No main-thread model processing.
  await yieldTask();
  checkCancelled();
  progress(id, operation === 'model' ? payload.method : operation);
  await yieldTask();
  checkCancelled();
  if (operation === 'initialize') {
    if (wasm) throw new Error('Worker is already initialized');
    wasm = await import(payload.moduleUrl);
    const initialized = await wasm.default(payload.wasmUrl ? { module_or_path: payload.wasmUrl } : undefined);
    wasmMemory = initialized?.memory;
    return { value: { ready: true } };
  }
  if (!wasm) throw new Error('Worker is not initialized');
  if (operation === 'memory') return { value: { wasm_bytes: wasmMemory?.buffer.byteLength ?? null, models: models.size } };
  if (operation === 'capabilities') return { value: wasm.model_capabilities() };
  if (operation === 'preflight') {
    return { value: wasm.preflight_model_bundle(payload.name, payload.bytes, payload.assets, payload.options) };
  }
  if (operation === 'open') {
    const scene = wasm.open_model_bundle(payload.name, payload.bytes, payload.assets, payload.document);
    const handle = nextHandle++;
    models.set(handle, scene);
    return { value: { handle } };
  }
  if (operation !== 'model' || !allowed.has(payload.method)) throw new Error('Unknown worker operation');
  const scene = models.get(payload.handle);
  if (!scene) throw new Error('Unknown or disposed model handle');
  const args = payload.args ?? [];
  if (payload.method === 'dispose') {
    scene.free();
    models.delete(payload.handle);
    return { value: null };
  }
  if (payload.method === 'export') {
    const output = scene.export_with_options(...args);
    try {
      const bytes = output.take_bytes ? output.take_bytes() : output.bytes;
      return { value: { bytes, diagnostics: output.losses }, transfers: [bytes.buffer] };
    } finally { output.free(); }
  }
  if (payload.method === 'extract') {
    const extracted = scene.extract(...args);
    const handle = nextHandle++;
    models.set(handle, extracted);
    return { value: { handle } };
  }
  if (payload.method === 'export_lods') {
    const output = scene.export_lods(...args);
    return { value: output, transfers: output.files.map((file) => file.bytes.buffer) };
  }
  if (payload.method === 'assets') {
    const assets = scene.assets();
    return { value: assets, transfers: [...new Set([...assets.values()].map((bytes) => bytes.buffer))] };
  }
  if (typeof scene[payload.method] !== 'function') throw new Error(`This WASM package lacks ${payload.method}`);
  return { value: scene[payload.method](...args) };
}

let queue = Promise.resolve();
self.onmessage = ({ data }) => {
  if (data.operation === 'cancel') {
    if (requests.has(data.cancelId)) cancelled.add(data.cancelId);
    starts.get(data.cancelId)?.(false);
    return;
  }
  if (data.operation === 'start') {
    starts.get(data.requestId)?.(true);
    return;
  }
  requests.add(data.id);
  queue = queue.then(async () => {
    try {
      const { value, transfers = [] } = await execute(data);
      progress(data.id, 'complete');
      postMessage({ id: data.id, type: 'result', value }, transfers);
    } catch (error) { postMessage({ id: data.id, type: 'error', error: errorPayload(error) }); }
    finally { requests.delete(data.id); cancelled.delete(data.id); starts.delete(data.id); }
  });
};
