/** Browser-only async adapter. All model work executes in a dedicated worker. */
export class ToolboxError extends Error {
  constructor(message, code = 'toolbox.error', details) {
    super(message);
    this.name = 'ToolboxError';
    this.code = code;
    this.details = details;
  }
}

const abortError = () => new DOMException('Operation aborted; worker models have been released.', 'AbortError');

function transferableBytes(input, transfer) {
  const bytes = input instanceof ArrayBuffer ? new Uint8Array(input) : input;
  if (!(bytes instanceof Uint8Array)) throw new TypeError('Expected ArrayBuffer or Uint8Array');
  // Shared buffers and slices are copied. Never detach an unrelated view's data.
  const owned = transfer && bytes.buffer instanceof ArrayBuffer && bytes.byteOffset === 0
    && bytes.byteLength === bytes.buffer.byteLength ? bytes : bytes.slice();
  return owned;
}

export class Toolbox {
  #worker;
  #pending = new Map();
  #nextId = 1;
  #closed = false;
  #ready;

  constructor(options = {}) {
    this.#worker = options.workerFactory
      ? options.workerFactory()
      : new Worker(options.workerUrl ?? new URL('./worker.js', import.meta.url), { type: 'module', name: 'usd-toolbox' });
    this.#worker.onmessage = ({ data }) => this.#receive(data);
    this.#worker.onerror = (event) => this.#stop(new ToolboxError(event.message || 'Worker failed', 'worker.failed'));
    this.#worker.onmessageerror = () => this.#stop(new ToolboxError('Invalid worker message', 'worker.message'));
    this.#ready = this.#request('initialize', {
      moduleUrl: String(options.wasmModuleUrl ?? new URL('./wasm/usd_toolbox_wasm.js', import.meta.url)),
      wasmUrl: options.wasmUrl ? String(options.wasmUrl) : undefined,
    });
    // Prevent a startup rejection being unhandled before the consumer awaits ready().
    this.#ready.catch(() => {});
  }

  ready() { return this.#ready; }
  get closed() { return this.#closed; }

  async #run(operation, payload, options = {}, transfers = []) {
    await this.#waitReady(options.signal);
    return this.#request(operation, payload, options, transfers);
  }

  async #waitReady(signal) {
    if (signal?.aborted) throw abortError();
    const onAbort = () => this.#stop(abortError());
    signal?.addEventListener('abort', onAbort, { once: true });
    try { await this.#ready; }
    finally { signal?.removeEventListener('abort', onAbort); }
    if (signal?.aborted) throw abortError();
  }

  #request(operation, payload, options = {}, transfers = []) {
    if (this.#closed) return Promise.reject(new ToolboxError('Toolbox is closed; create a new instance.', 'worker.closed'));
    if (options.signal?.aborted) return Promise.reject(abortError());
    const id = this.#nextId++;
    return new Promise((resolve, reject) => {
      const onAbort = () => {
        const phase = this.#pending.get(id)?.phase;
        if (phase === 'pending' || phase === 'queued') {
          try { this.#worker.postMessage({ operation: 'cancel', cancelId: id }); }
          catch { this.#stop(abortError()); return; }
          this.#settle(id, false, new DOMException('Queued operation aborted.', 'AbortError'));
        } else this.#stop(abortError());
      };
      this.#pending.set(id, { resolve, reject, phase: 'pending', onProgress: options.onProgress, signal: options.signal, onAbort });
      options.signal?.addEventListener('abort', onAbort, { once: true });
      try { this.#worker.postMessage({ id, operation, payload }, transfers); }
      catch (error) { this.#settle(id, false, error); }
    });
  }

  #receive(message) {
    const pending = this.#pending.get(message.id);
    if (message.type === 'ready') {
      // The worker cannot enter a Rust operation until this explicit permit.
      // Cancellation while still queued therefore cannot commit a hidden edit.
      if (!pending) { this.#worker.postMessage({ operation: 'cancel', cancelId: message.id }); return; }
      pending.phase = 'running';
      try { this.#worker.postMessage({ operation: 'start', requestId: message.id }); }
      catch (error) { this.#stop(error); }
      return;
    }
    if (!pending) return;
    if (message.type === 'progress') {
      pending.phase = message.progress.phase;
      // A consumer callback must not corrupt request bookkeeping.
      try { pending.onProgress?.(message.progress); } catch { /* consumer-owned callback */ }
      return;
    }
    if (message.type === 'error') {
      this.#settle(message.id, false, new ToolboxError(message.error.message, message.error.code, message.error.details));
    } else if (message.type === 'result') this.#settle(message.id, true, message.value);
  }

  #settle(id, success, value) {
    const pending = this.#pending.get(id);
    if (!pending) return;
    this.#pending.delete(id);
    pending.signal?.removeEventListener('abort', pending.onAbort);
    (success ? pending.resolve : pending.reject)(value);
  }

  #stop(error) {
    if (this.#closed) return;
    this.#closed = true;
    this.#worker.terminate();
    for (const id of this.#pending.keys()) this.#settle(id, false, error);
  }

  async open(input, options = {}) {
    await this.#waitReady(options.signal);
    if (options.signal?.aborted) throw abortError();
    const bytes = transferableBytes(input, options.transfer === true);
    const assets = (options.assets ?? []).map((asset) => ({
      name: asset.name, bytes: transferableBytes(asset.bytes, options.transfer === true),
    }));
    const transfers = [...new Set([bytes.buffer, ...assets.map((asset) => asset.bytes.buffer)])];
    const result = await this.#request('open', { bytes, name: options.name ?? 'model.usd', assets, document: options.document }, options, transfers);
    return new Model(this, result.handle);
  }

  async preflight(input, options = {}) {
    await this.#waitReady(options.signal);
    if (options.signal?.aborted) throw abortError();
    const bytes = transferableBytes(input, options.transfer === true);
    const assets = (options.assets ?? []).map((asset) => ({
      name: asset.name, bytes: transferableBytes(asset.bytes, options.transfer === true),
    }));
    const transfers = [...new Set([bytes.buffer, ...assets.map((asset) => asset.bytes.buffer)])];
    return this.#request('preflight', { bytes, name: options.name ?? 'model.usd', assets, options: options.profile ?? {} }, options, transfers);
  }

  capabilities(options) { return this.#run('capabilities', {}, options); }
  memory(options) { return this.#run('memory', {}, options); }

  /** Internal model dispatcher; handles are valid only within this Toolbox. */
  modelRequest(handle, method, args = [], options) { return this.#run('model', { handle, method, args }, options); }
  dispose() { this.#stop(new ToolboxError('Toolbox disposed.', 'worker.disposed')); }
}

export class Model {
  #toolbox;
  #handle;
  #disposed = false;
  constructor(toolbox, handle) { this.#toolbox = toolbox; this.#handle = handle; }
  #call(method, args, options) {
    if (this.#disposed) return Promise.reject(new ToolboxError('Model is disposed.', 'model.disposed'));
    return this.#toolbox.modelRequest(this.#handle, method, args, options);
  }
  inspect(options) { return this.#call('inspect', [], options); }
  diagnostics(options) { return this.#call('diagnostics', [], options); }
  query(request = {}, options) { return this.#call('query', [request], options); }
  getNode(id, options) { return this.#call('get_node', [id], options); }
  getMetadata(scope = { scope: 'scene' }, options) { return this.#call('get_metadata', [scope], options); }
  getProperty(target, key, namespace = 'normalized', options) { return this.#call('get_property', [target, namespace, key], options); }
  children(id, options) { return this.#call('children', [id ?? null], options); }
  ancestors(id, options) { return this.#call('ancestors', [id], options); }
  descendants(id, options) { return this.#call('descendants', [id], options); }
  idsForPath(path, options) { return this.#call('ids_for_path', [path], options); }
  count(filter = {}, options) { return this.#call('count', [filter], options); }
  summary(options) { return this.#call('summary', [], options); }
  apply(edits, options) { return this.#call('update', [edits], options); }
  export(target = 'glb', options = {}) {
    const { textureTier, includeHidden, optimizeVertexCache, ...request } = options;
    const settings = {};
    if (textureTier !== undefined) settings.texture_tier = textureTier;
    if (includeHidden !== undefined) settings.include_hidden = includeHidden;
    if (optimizeVertexCache !== undefined) settings.optimize_vertex_cache = optimizeVertexCache;
    return this.#call('export', [target, settings], request);
  }
  exportLods(lods, target = 'glb', options = {}) {
    const { textureTier, includeHidden, optimizeVertexCache, ...request } = options;
    const settings = {};
    if (textureTier !== undefined) settings.texture_tier = textureTier;
    if (includeHidden !== undefined) settings.include_hidden = includeHidden;
    if (optimizeVertexCache !== undefined) settings.optimize_vertex_cache = optimizeVertexCache;
    return this.#call('export_lods', [lods, target, settings], request);
  }
  assets(options) { return this.#call('assets', [], options); }
  async extract(id, options) {
    const { handle } = await this.#call('extract', [id], options);
    return new Model(this.#toolbox, handle);
  }
  async dispose() {
    if (this.#disposed) return;
    this.#disposed = true;
    if (!this.#toolbox.closed) await this.#toolbox.modelRequest(this.#handle, 'dispose');
  }
}
