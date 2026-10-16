#!/usr/bin/env node
// Release-build real-model worker benchmark. Fetch time is recorded separately;
// CPU/memory container limits are host settings, never claimed as a mobile device.
import fs from 'node:fs/promises';
import http from 'node:http';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { createRequire } from 'node:module';
const require = createRequire(import.meta.url);
const playwright = require(process.env.PLAYWRIGHT_MODULE ?? 'playwright');
const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const pkg = path.resolve(process.argv[2] ?? path.join(root, 'target/web-package'));
const asset = path.resolve(process.argv[3] ?? path.join(root, 'target/corpus/BoomBox/BoomBox.glb'));
const server = http.createServer(async (request, response) => {
  const url = new URL(request.url, 'http://localhost');
  if (url.pathname === '/') { response.writeHead(200, { 'Content-Type': 'text/html' }); response.end('<!doctype html><title>USD benchmark</title>'); return; }
  const file = url.pathname === '/asset.glb' ? asset : path.resolve(pkg, '.' + decodeURIComponent(url.pathname));
  if (file !== asset && !file.startsWith(pkg + path.sep)) { response.writeHead(403); response.end(); return; }
  try {
    const bytes = await fs.readFile(file);
    response.writeHead(200, { 'Content-Type': file.endsWith('.js') ? 'text/javascript' : file.endsWith('.wasm') ? 'application/wasm' : 'application/octet-stream' });
    response.end(bytes);
  } catch { response.writeHead(404); response.end(); }
});
await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
const browser = await playwright.chromium.launch({ headless: true, ...(process.env.CHROMIUM_EXECUTABLE ? { executablePath: process.env.CHROMIUM_EXECUTABLE } : {}) });
try {
  const page = await browser.newPage();
  await page.goto(`http://127.0.0.1:${server.address().port}`);
  const result = await page.evaluate(async () => {
    const { Toolbox } = await import('/index.js');
    const fetchStart = performance.now();
    const source = new Uint8Array(await (await fetch('/asset.glb')).arrayBuffer());
    const fetch_ms = performance.now() - fetchStart;
    const cold = [];
    for (let i = 0; i < 3; i++) {
      const start = performance.now();
      const client = new Toolbox();
      await client.ready();
      cold.push(performance.now() - start);
      client.dispose();
    }
    const client = new Toolbox();
    await client.ready();
    const cases = [];
    for (const transfer of [false, true]) {
      const runs = [];
      for (let i = 0; i < 5; i++) {
        const input = source.slice(); // caller buffer preparation excluded equally.
        let heartbeat = 0;
        const timer = setInterval(() => heartbeat++, 5);
        const start = performance.now();
        const model = await client.open(input, { name: 'asset.glb', transfer });
        const opened = performance.now();
        const first = await model.query({ fields: ['name'], limit: 1 });
        const first_metadata_ms = performance.now() - start;
        const timings = [];
        for (let j = 0; j < 50; j++) { const now = performance.now(); await model.getNode(first.rows[0].id); timings.push(performance.now() - now); }
        timings.sort((a, b) => a - b);
        const exportStart = performance.now();
        const output = await model.export('glb');
        const export_ms = performance.now() - exportStart;
        const memory = await client.memory();
        await model.dispose();
        clearInterval(timer);
        runs.push({ open_ms: opened - start, first_metadata_ms, query_p50_ms: timings[25], query_p95_ms: timings[47], export_ms,
          import_mib_per_second: source.length / (1024 * 1024) / ((opened - start) / 1000), output_bytes: output.bytes.length,
          caller_detached: input.byteLength === 0, wasm_bytes: memory.wasm_bytes, main_thread_heartbeats: heartbeat });
      }
      cases.push({ transfer, runs });
    }
    const finalMemory = await client.memory();
    client.dispose();
    return { user_agent: navigator.userAgent, input_bytes: source.length, fetch_ms, worker_init_ms: cold, cases, final_memory: finalMemory };
  });
  const build = JSON.parse(await fs.readFile(path.join(pkg, 'build-report.json'), 'utf8'));
  const report = { schema: 'usd-toolbox.web-benchmark.v1', environment: process.env.BENCH_ENVIRONMENT ?? 'unconstrained desktop host',
    hardware_constrained_device: false, package_wasm: build.files['wasm/usd_toolbox_wasm_bg.wasm'], asset: path.basename(asset), ...result };
  if (process.env.WEB_REPORT) await fs.writeFile(process.env.WEB_REPORT, JSON.stringify(report, null, 2) + '\n');
  console.log(JSON.stringify(report));
} finally { await browser.close(); await new Promise((resolve) => server.close(resolve)); }
