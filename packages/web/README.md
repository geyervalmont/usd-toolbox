# USD toolbox for the web

The ESM wrapper owns a dedicated module worker. Import, queries, edits and export
run inside that worker. Geometry and images remain in WASM between queries;
responses contain only requested fields. No server, native USD installation,
SharedArrayBuffer or cross-origin isolation is required for the standard build.

Build the WASM module and package with `scripts/build_web_package.py`. The generated
package lives under `target/web-package`. Serve JavaScript as JavaScript and WASM
as `application/wasm`; worker and WASM URLs must satisfy your application's origin
and CSP rules. Applications provide all source and dependency bytes.

```js
import { Toolbox } from '@usd-toolbox/web';

const toolbox = new Toolbox();
const model = await toolbox.open(await file.arrayBuffer(), { name: file.name });
const page = await model.query({
  filter: { node_type: 'mesh' },
  fields: ['name', 'metadata', 'bounds', 'effective_material'],
  limit: 50,
});
const sceneMetadata = await model.getMetadata();
const object = await model.getNode(page.rows[0].id);
const exported = await model.export('glb');
// exported.bytes is a Uint8Array owned by this thread.
await model.dispose();
toolbox.dispose();
```

For a smaller preview use `model.export('glb', {textureTier: 'preview'})`.
`preview`, `1k`, `2k`, `4k` and `8k` cap the longest image edge at
512/1024/2048/4096/8192 pixels; smaller images are never enlarged. Sources remain
intact. Colour textures filter in linear light, data maps stay linear and normal
maps are renormalized. Geometry LOD generation is a separate explicit operation.
`optimizeVertexCache: true` enables measured opaque-primitive cache reordering;
transparent/opacity/transmission geometry retains its triangle order.

`model.exportLods({levels: [{name: 'half', face_ratio: 0.5,
max_vertex_displacement: 0.01}]})` returns a manifest and transferred binary files.
Each level derives from the unchanged source. The displacement is in scene units;
the manifest states the error policy and whether the target was reached. Browser
LOD generation caps eight levels and aggregate output/geometry work; it may reject
a small derivative whose source processing exceeds those limits.

Use the same query and `next_cursor` for the next page. Cursors are opaque and
bound to content revision, generation and query. Editing invalidates previous
cursors. Metadata integers use typed decimal strings to retain all 64 bits.
Missing keys, explicit null and unresolved values remain distinct. Bounds queries
return each node's own mesh bounds in metres; inspection describes the full scene.

`preflight(bytes, {name, assets, profile})` scans compatibility before opening a
model. Authored-layer metadata can be inspected even when neutral import fails.
Set `profile.check_geometry` to import and validate geometry using the supplied
assets; glTF inputs additionally check GLB export when `profile.target` is `glb`
or `browser_preview`. Other target encoding remains deferred. `profile.max_issues`
caps diagnostics, including geometry/target findings, and truncation or failed
requested checks makes the report incomplete. `max_specs` and
`max_metadata_bytes` apply only to USD scans; glTF reports identify these as
`usd_only_options`. Browser resource limits stay fixed. Read the returned
validation/performed/deferred checks and completeness; a lightweight scan does
not imply geometry validation, successful composition or render equivalence.

Inputs are copied by default. `transfer: true` transfers a whole ArrayBuffer to
the worker and detaches it from the caller. Slices are copied to protect unrelated
views. `assets` is an array of `{name, bytes}` using confined relative paths. This
wrapper does not fetch textures, follow arbitrary asset URLs or resolve USD layers.

Every operation accepts an optional `AbortSignal` and phase progress callback.
The worker cooperatively checks queued cancellation at scheduling/yield points;
cancelling a queued request preserves other handles. A synchronous Rust conversion cannot process
JavaScript cancellation events while it runs, so abort terminates the worker.
**Aborting any active request invalidates every model in that Toolbox and rejects
all pending requests.** Create another Toolbox and reopen needed models. For
independent cancellation domains, use separate Toolbox instances. Already-aborted
requests do not transfer input buffers or terminate other work.
Cancellation during initialization also settles immediately. Once input buffers
have transferred, queued cancellation cannot restore their caller ownership.

`Model.apply()` edits the retained document atomically. Metadata and hierarchy
edits reuse existing geometry and image allocations. `extract()` returns a new
model handle. Release individual models with `dispose()`; terminate the Toolbox
when the session ends. WASM may reuse freed allocations without shrinking its
linear memory. Worker termination releases the entire runtime. `memory()` reports
the linear-memory allocation and live model count, not total browser process RSS.

Exports transfer owned bytes to the caller and release the Rust export result.
No borrowed WASM memory view is exposed, so memory growth cannot invalidate a
previous response. Standalone USDA/USDC exports need the sidecars returned by
`model.assets()`; USDZ and GLB embed their dependencies.

For Vite, the tested deployment copies the generated package into
`public/toolbox`. The [complete example](../../examples/vite/main.js) resolves that
directory with `import.meta.env.BASE_URL`, so nested deployments work. Run
`scripts/test_vite_package.mjs` to build and exercise the production bundle under
`/models/`. Keep the package's worker, glue module and WASM file together.

Applications that manage their own asset layout can pass explicit URLs:

```js
const toolbox = new Toolbox({
  workerUrl: new URL('/vendor/usd-toolbox/worker.js', location.href),
  wasmModuleUrl: new URL('/vendor/usd-toolbox/wasm/usd_toolbox_wasm.js', location.href),
});
```

The raw wasm-bindgen module also remains available for advanced integration.
Its synchronous functions belong in a worker when handling significant assets.
Call `free()` on raw scene/export handles. `take_bytes()` moves an export's payload
out once to avoid an extra Rust clone; `bytes` preserves the older copy getter.

The package is prepared locally by the build script and can be installed from
`npm pack ./target/web-package`. It is marked private because this repository has
no declared project distribution licence. Third-party notices and file checksums
are included; they do not grant a licence for the project itself. The build does
not upload or publish anything.
