#!/usr/bin/env node
import assert from 'node:assert/strict';
import fs from 'node:fs/promises';
import http from 'node:http';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { createRequire } from 'node:module';

const require = createRequire(import.meta.url);
const playwright = require(process.env.PLAYWRIGHT_MODULE ?? 'playwright');
const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const pkg = path.resolve(process.argv[2] ?? path.join(root, 'target/web-package'));
const server = http.createServer(async (request, response) => {
  const url = new URL(request.url, 'http://localhost');
  if (url.pathname === '/') { response.writeHead(200, { 'Content-Type': 'text/html' }); response.end('<!doctype html><title>USD toolbox browser conformance</title>'); return; }
  const base = url.pathname.startsWith('/fixtures/') ? root : pkg;
  const local = path.resolve(base, '.' + decodeURIComponent(url.pathname));
  if (!local.startsWith(base + path.sep)) { response.writeHead(403); response.end(); return; }
  try {
    const data = await fs.readFile(local);
    const type = local.endsWith('.wasm') ? 'application/wasm' : local.endsWith('.js') ? 'text/javascript' : 'application/octet-stream';
    response.writeHead(200, { 'Content-Type': type }); response.end(data);
  } catch { response.writeHead(404); response.end(); }
});
await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
const url = `http://127.0.0.1:${server.address().port}`;
const reports = [];
try {
  for (const name of (process.env.BROWSERS ?? 'chromium').split(',')) {
    const browser = await playwright[name].launch({ headless: true, ...(name === 'chromium' && process.env.CHROMIUM_EXECUTABLE ? { executablePath: process.env.CHROMIUM_EXECUTABLE } : {}) });
    try {
      const page = await browser.newPage();
      const errors = [];
      page.on('pageerror', (error) => errors.push(error.message));
      await page.goto(url);
      const result = await page.evaluate(async () => {
        const check = (test, message) => { if (!test) throw new Error(message); };
        const { Toolbox } = await import('/index.js');
        const phases = [];
        const start = performance.now();
        const toolbox = new Toolbox();
        await toolbox.ready();
        const init_ms = performance.now() - start;
        const source = new Uint8Array(await (await fetch('/fixtures/models/panels.usda')).arrayBuffer());
        const checker = new Uint8Array(await (await fetch('/fixtures/models/checker.png')).arrayBuffer());
        const options = { name: 'panels.usda', assets: [{ name: 'checker.png', bytes: checker }], document: { metadata_indexes: [{ key: 'product_id' }] }, onProgress: ({ phase }) => phases.push(phase) };
        const preflight = await toolbox.preflight(source, options);
        check(preflight.schema === 'usd-toolbox.preflight.v1', 'versioned authored preflight');
        const model = await toolbox.open(source, options);
        check(source.byteLength > 0 && checker.byteLength > 0, 'default input ownership');
        const summary = await model.summary();
        check(summary.meshes === 1, 'model mesh summary');
        const first = await model.query({ fields: ['name'], limit: 1 });
        check(first.rows.length === 1 && first.next_cursor, 'first page cursor');
        const next = await model.query({ fields: ['name'], limit: 1, cursor: first.next_cursor });
        check(first.rows[0].id !== next.rows[0].id, 'pagination advances');
        const meshes = await model.query({ filter: { node_type: 'mesh' }, fields: ['name', 'bounds', 'metadata'] });
        const mesh = meshes.rows[0];
        check(mesh.fields.bounds.value.min[0] === .75 && mesh.fields.bounds.unit === 'm', 'world bounds expressed in metres');
        check(!/"positions"\s*:/.test(JSON.stringify(meshes)), 'query omits bulk geometry');
        const report = await model.apply([{ operation: 'set_metadata', target: { scope: 'node', id: mesh.id }, key: 'product_id', value: { value: { type: 'u64', value: '18446744073709551615' }, state: 'authored' } }]);
        check(report.generation === 1, 'document generation advances');
        const metadata = await model.getMetadata({ scope: 'node', id: mesh.id });
        check(metadata.normalized.product_id.value.value === '18446744073709551615', 'typed integer preserved');
        check((await model.getProperty({ scope: 'node', id: mesh.id }, 'product_id')).value.value === '18446744073709551615', 'direct property lookup');
        check(await model.count({ node_type: 'mesh' }) === 1, 'matching count');
        check((await model.idsForPath(mesh.id)).includes(mesh.id), 'indexed source path');
        check((await model.ancestors(mesh.id)).includes('/Panels'), 'ancestor lookup');
        check((await model.descendants('/Panels')).includes(mesh.id), 'descendant lookup');
        let stale = false;
        try { await model.query({ fields: ['name'], limit: 1, cursor: first.next_cursor }); } catch (error) { stale = error.code === 'query.stale_cursor' && error.details.path === '/cursor'; }
        check(stale, 'stale cursor rejected');
        const lods = await model.exportLods({ levels: [{ name: 'coarse', face_ratio: 0.5, max_vertex_displacement: 0.1 }] });
        check(lods.manifest.schema === 'usd-toolbox.geometry-lods.v1' && lods.files.length === 1, 'LOD derivative bundle');
        check(lods.manifest.files[0].bytes === lods.files[0].bytes.length && lods.manifest.files[0].sha256.length === 64, 'LOD artifact manifest');
        check((await model.summary()).generation === 1, 'LOD export leaves original document unchanged');
        const queuedCancel = new AbortController();
        let cancelledBeforeCommit = false;
        try { await model.apply([{ operation: 'rename', node: '/Panels', name: 'Must not commit' }], {
          signal: queuedCancel.signal, onProgress: ({ phase }) => { if (phase === 'queued') queuedCancel.abort(); },
        }); } catch (error) { cancelledBeforeCommit = error.name === 'AbortError'; }
        check(cancelledBeforeCommit && !toolbox.closed && (await model.summary()).generation === 1, 'queued edit cancellation preserves state and handles');
        const output = await model.export('glb');
        check(new TextDecoder().decode(output.bytes.slice(0, 4)) === 'glTF', 'real GLB bytes');
        const glbPreflight = await toolbox.preflight(output.bytes, { name: 'roundtrip.glb' });
        check(glbPreflight.schema === 'usd-toolbox.gltf-preflight.v1', 'glTF preflight');
        check(glbPreflight.validation.geometry === 'deferred', 'lightweight glTF geometry remains deferred');
        const checkedGlb = await toolbox.preflight(output.bytes, { name: 'roundtrip.glb', profile: { check_geometry: true, target: 'browser_preview' } });
        check(checkedGlb.complete && checkedGlb.target === 'browser_preview', 'glTF requested profile applied');
        check(checkedGlb.validation.geometry === 'performed' && checkedGlb.validation.buffers === 'performed' && checkedGlb.validation.target === 'performed', 'glTF geometry and target validation performed');
        const invalidGltf = new TextEncoder().encode(JSON.stringify({ asset: { version: '2.0' }, nodes: [{ mesh: 99 }], scenes: [{ nodes: [0] }], scene: 0 }));
        const failedGltf = await toolbox.preflight(invalidGltf, { name: 'invalid.gltf', profile: { check_geometry: true } });
        check(!failedGltf.complete && failedGltf.validation.geometry === 'failed', 'failed requested geometry is incomplete');
        const manyGltf = new TextEncoder().encode(JSON.stringify({ asset: { version: '2.0' }, cameras: [{}], skins: [{}], animations: [{}] }));
        const cappedGltf = await toolbox.preflight(manyGltf, { name: 'many.gltf', profile: { max_issues: 1 } });
        check(cappedGltf.diagnostics.length === 1 && cappedGltf.diagnostics_truncated && !cappedGltf.complete, 'glTF diagnostic cap honored');
        const roundtrip = await toolbox.open(output.bytes, { name: 'roundtrip.glb', transfer: true });
        check(output.bytes.byteLength === 0, 'explicit buffer transfer');
        check((await roundtrip.inspect()).placed_faces === 4, 'GLB reimport retains four preview triangles');
        await roundtrip.dispose();
        const samples = [];
        for (let i = 0; i < 30; i++) {
          const before = performance.now();
          await model.getNode(mesh.id);
          samples.push(performance.now() - before);
        }
        samples.sort((a, b) => a - b);
        const memory = [await toolbox.memory()];
        await model.dispose();
        for (let i = 0; i < 8; i++) { const item = await toolbox.open(source, options); await item.query(); await item.dispose(); memory.push(await toolbox.memory()); }
        check(memory.slice(1).every((entry) => entry.models === 0), 'handle lifecycle');
        check(memory.at(-1).wasm_bytes <= Math.max(...memory.slice(0, 4).map((entry) => entry.wasm_bytes)) * 2, 'bounded retained WASM allocation');
        toolbox.dispose();

        const cancellable = new Toolbox();
        await cancellable.ready();
        const controller = new AbortController();
        const operation = cancellable.open(source, { ...options, signal: controller.signal, onProgress: ({ phase }) => { if (phase === 'open') controller.abort(); } });
        let aborted = false;
        try { await operation; } catch (error) { aborted = error.name === 'AbortError'; }
        check(aborted && cancellable.closed, 'abort terminates worker without blocking UI');
        for (const phase of ['preflight', 'query', 'update', 'export']) {
          const client = new Toolbox();
          const model = await client.open(source, options);
          const controller = new AbortController();
          const request = { signal: controller.signal, onProgress: (progress) => { if (progress.phase === phase) controller.abort(); } };
          let interrupted = false;
          try {
            if (phase === 'preflight') await client.preflight(source, { ...options, ...request });
            else if (phase === 'query') await model.query({}, request);
            else if (phase === 'update') await model.apply([{ operation: 'rename', node: '/Panels', name: 'Cancel' }], request);
            else await model.export('glb', request);
          } catch (error) { interrupted = error.name === 'AbortError'; }
          check(interrupted && client.closed, `${phase} cancellation settles and releases worker`);
        }
        return { init_ms, query_p50_ms: samples[15], query_p95_ms: samples[28], memory, phases: [...new Set(phases)] };
      });
      assert.deepEqual(errors, [], `${name} browser errors`);
      reports.push({ browser: name, ...result });
    } finally { await browser.close(); }
  }
  const report = { schema: 'usd-toolbox.browser-evidence.v1', package: pkg, reports };
  if (process.env.WEB_REPORT) await fs.writeFile(process.env.WEB_REPORT, JSON.stringify(report, null, 2) + '\n');
  console.log(JSON.stringify(report));
} finally { await new Promise((resolve) => server.close(resolve)); }
