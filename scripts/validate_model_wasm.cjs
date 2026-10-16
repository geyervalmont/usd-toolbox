// node scripts/validate_model_wasm.cjs wasm/pkg.js model.usdz native.glb
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const os = require('node:os');
const { execFileSync } = require('node:child_process');
const wasm = require(path.resolve(process.argv[2]));
const scene = wasm.import_model(fs.readFileSync(process.argv[3]));
const glb = scene.export('glb');
assert.deepEqual(Buffer.from(glb.bytes), fs.readFileSync(process.argv[4]), 'native/WASM GLB bytes');
assert.equal(scene.inspect().placed_faces, 2);
assert.deepEqual(scene.inspect().bounds, {min:[750,450,-250],max:[1750,450,250]});
const edited = scene.apply([{operation:'convert_units',meters_per_unit:1}]);
assert.deepEqual(edited.inspect().bounds.min,[.75,.45,-.25]);
assert.ok(scene.diff(edited).length);
const extracted = scene.extract('/Panels/Surface');
assert.equal(extracted.inspect().nodes,1);
assert.equal(scene.inspect().meters_per_unit,.001,'edits leave source unchanged');
assert.ok(scene.assets().size > 0,'standalone USD assets available as typed buffers');
assert.throws(()=>wasm.model_from_json('{}'));
const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'usd-model-parity-'));
try {
  const cli = process.env.USD_TOOLBOX_BIN || path.resolve('target/debug/usd-toolbox');
  const source = path.resolve(process.argv[3]);
  const request = { fields: ['name', 'source_path', 'metadata', 'bounds', 'world_transform', 'effective_material', 'source_hash'], limit: 100 };
  const queryPath = path.join(directory, 'query.json');
  fs.writeFileSync(queryPath, JSON.stringify(request));
  const native = (args) => execFileSync(cli, ['model', ...args], { encoding: 'utf8' });
  const page = JSON.parse(native(['query', '--input', source, '--request', queryPath]));
  assert.deepEqual(scene.query(request).rows, page.rows, 'projected native/WASM query equivalence');
  assert.deepEqual(scene.get_metadata({ scope: 'scene' }), JSON.parse(native(['metadata', '--input', source])), 'scene metadata equivalence');
  const first = scene.query({ fields: ['name'], limit: 1 });
  const next = scene.query({ fields: ['name'], limit: 1, cursor: first.next_cursor });
  assert.notEqual(first.rows[0].id, next.rows[0].id);
  const edits = [{ operation: 'rename', node: '/Panels/Surface', name: '木材 panel' },
    { operation: 'set_metadata', target: { scope: 'node', id: '/Panels/Surface' }, key: 'serial', value: { value: { type: 'u64', value: '18446744073709551615' } } }];
  const editPath = path.join(directory, 'edits.json');
  const outputPath = path.join(directory, 'edited.usdz');
  fs.writeFileSync(editPath, JSON.stringify(edits));
  native(['apply', '--input', source, '--edits', editPath, '--output', outputPath]);
  scene.update(edits);
  assert.throws(() => scene.query({ fields: ['name'], limit: 1, cursor: first.next_cursor }), /stale_cursor/);
  const after = JSON.parse(native(['query', '--input', outputPath, '--request', queryPath]));
  // Generation is document-local; file reopen starts at zero. Bounds explicitly
  // report that generation, while the numerical result and derivation must agree.
  const projected = scene.query(request).rows;
  for (const row of projected) if (row.fields.bounds) row.fields.bounds.generation = 0;
  assert.deepEqual(projected, after.rows, 'metadata and structural edits agree across runtimes');
  const malformed = new TextEncoder().encode('#usda 1.0\ndef Mesh {');
  const preflight = wasm.preflight_model_bundle('broken.usda', malformed, [], {});
  const broken = path.join(directory, 'broken.usda');
  fs.writeFileSync(broken, malformed);
  let nativeFailure;
  try { native(['preflight', '--input', broken]); } catch (error) { nativeFailure = JSON.parse(error.stdout); }
  assert.deepEqual(preflight.issues, nativeFailure.issues, 'native/WASM malformed-input diagnostics');
} finally { fs.rmSync(directory, { recursive: true, force: true }); }
for (const value of [glb,extracted,edited,scene]) value.free();
console.log('Native/WASM binary parity, queries, metadata, pagination, diagnostics, edits, extraction and disposal passed');
