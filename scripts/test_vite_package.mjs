#!/usr/bin/env node
import assert from 'node:assert/strict';
import fs from 'node:fs/promises';
import http from 'node:http';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { createRequire } from 'node:module';
import { spawnSync } from 'node:child_process';

const require = createRequire(import.meta.url);
const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const vite = process.env.VITE_CLI ?? require.resolve('vite/bin/vite.js');
const playwright = require(process.env.PLAYWRIGHT_MODULE ?? 'playwright');
const pkg = path.resolve(process.argv[2] ?? path.join(root, 'target/web-package'));
const work = path.join(root, 'target/vite-example');
await fs.mkdir(work, { recursive: true });
await fs.cp(path.join(root, 'examples/vite'), work, { recursive: true });
await fs.mkdir(path.join(work, 'public'), { recursive: true });
await fs.cp(pkg, path.join(work, 'public/toolbox'), { recursive: true });
const build = spawnSync(process.execPath, [vite, 'build', '--base=/models/'], { cwd: work, encoding: 'utf8' });
assert.equal(build.status, 0, build.stdout + build.stderr);
const base = path.join(work, 'dist');
const server = http.createServer(async (request, response) => {
  const url = new URL(request.url, 'http://localhost');
  const file = path.resolve(base, '.' + decodeURIComponent(url.pathname).replace(/^\/models/, '') + (url.pathname.endsWith('/') ? 'index.html' : ''));
  if (!file.startsWith(base + path.sep)) { response.writeHead(403); response.end(); return; }
  try {
    const bytes = await fs.readFile(file);
    const type = file.endsWith('.js') ? 'text/javascript' : file.endsWith('.wasm') ? 'application/wasm' : file.endsWith('.html') ? 'text/html' : 'application/octet-stream';
    response.writeHead(200, { 'Content-Type': type }); response.end(bytes);
  } catch { response.writeHead(404); response.end(); }
});
await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
let browser;
try {
  browser = await playwright.chromium.launch({ headless: true, ...(process.env.CHROMIUM_EXECUTABLE ? { executablePath: process.env.CHROMIUM_EXECUTABLE } : {}) });
  const page = await browser.newPage();
  const errors = [];
  page.on('pageerror', (error) => errors.push(error.message));
  await page.goto(`http://127.0.0.1:${server.address().port}/models/`);
  await page.waitForFunction(() => window.toolboxExample);
  const result = await page.evaluate(async () => {
    const model = await window.toolboxExample.open(new TextEncoder().encode('#usda 1.0\ndef Xform "Vite" {\ndef Mesh "Triangle" {\nuniform token subdivisionScheme = "none"\npoint3f[] points = [(0,0,0),(1,0,0),(0,1,0)]\nint[] faceVertexCounts = [3]\nint[] faceVertexIndices = [0,1,2]\n}\n}\n'));
    try { return await model.query({ fields: ['name'] }); }
    finally { await model.dispose(); window.toolboxExample.dispose(); }
  });
  assert.deepEqual(errors, []);
  assert.equal(result.rows[0].fields.name, 'Vite');
  console.log('Vite production build under /models/: ESM, worker, WASM, query and disposal passed');
} finally { await browser?.close(); await new Promise((resolve) => server.close(resolve)); }
