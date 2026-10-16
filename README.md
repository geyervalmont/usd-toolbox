# usd-toolbox

[![CI](https://github.com/geyervalmont/usd-toolbox/actions/workflows/ci.yml/badge.svg)](https://github.com/geyervalmont/usd-toolbox/actions/workflows/ci.yml)
[![Packages](https://github.com/geyervalmont/usd-toolbox/actions/workflows/artifacts.yml/badge.svg)](https://github.com/geyervalmont/usd-toolbox/actions/workflows/artifacts.yml)

**Import, query, edit and package static 3D models and materials.** A Rust toolkit
for USD/USDZ, glTF/GLB and MaterialX, with a CLI, browser WASM package and C ABI.

Open a model once, keep its geometry in memory, and query only the names, metadata,
bindings or bounds you need. Prepare reusable assets with atomic edits, indexed
previews and explicit geometry LODs. Format adapters share a neutral Rust model;
portable APIs take caller-supplied bytes and require no native OpenUSD installation.

## Capabilities

| Workflow | Available tools |
|---|---|
| **Models** | Static USD and glTF import; hierarchy, shared meshes, transforms, UVs and material bindings; preflight, validation, extraction and USDZ/GLB export |
| **Metadata and queries** | Typed scene/node/mesh/material metadata; indexed ID/path lookup; filters, counts, projections, stable pagination and cached physical bounds |
| **Editing and geometry** | Rename, reparent, duplicate and merge; metadata updates; pivot/axis conversion, transform baking, normal generation, welding, repair and LOD sets |
| **Materials and textures** | OpenPBR-aligned materials, MaterialX graph preservation, PNG/JPEG texture tiers, inspection, audits, edits, provenance and procedural PBR recipes |
| **Browser and native integration** | Typed ESM worker API, transferable buffers and explicit disposal; reusable C document handles and prepared export results |
| **Additional targets** | Revit appearance image sets and Omniverse USDZ; optional native OpenUSD preprocessing for composed stages and instances |

Support is profile-specific. Capability discovery and structured diagnostics report
unsupported features and conversion losses. See [model tools](docs/model-toolbox.md),
[material workflows](docs/material-workflows.md) and the [format boundaries](#supported-profiles).

## Try the CLI

From a checkout, with Rust 1.96 or newer:

```sh
cargo build --release --locked -p usd-toolbox-cli
mkdir -p target/demo

# Inspect the included textured model and check its GLB compatibility.
target/release/usd-toolbox model inspect --input fixtures/models/panels.usda
target/release/usd-toolbox model preflight \
  --input fixtures/models/panels.usda --target glb --geometry

# Package the model and its supplied texture into USDZ.
target/release/usd-toolbox model pack \
  --input fixtures/models/panels.usda \
  --output target/demo/panels.usdz --report target/demo/package.json

# Export an indexed GLB with preview-sized textures.
target/release/usd-toolbox model export \
  --input target/demo/panels.usdz --target glb --tier preview \
  --output target/demo/panels.glb --report target/demo/preview.json
```

The original [panels fixture](fixtures/models/README.md) includes its texture and
exercises indexed geometry, material subsets, a UV seam and millimetre units.
The CLI resolves referenced dependencies beneath the source directory.

Query a bounded page without including vertex or image buffers:

```sh
cat > target/demo/query.json <<'JSON'
{
  "filter": { "node_type": "mesh" },
  "fields": ["name", "metadata", "bounds", "effective_material"],
  "sort": "name",
  "limit": 50
}
JSON

target/release/usd-toolbox model query \
  --input target/demo/panels.usdz --request target/demo/query.json

target/release/usd-toolbox model capabilities
```

Query bounds are in metres. Metadata preserves exact 64-bit integers as tagged
decimal strings. For many queries against one import, retain a Rust
`SceneDocument`, browser `Model` or C document handle. The CLI opens an input per
invocation. See the [query contract](docs/query-api.md) and
[geometry LOD packaging](docs/geometry-lods.md) for complete examples.

## Use it in the browser

Build the typed worker package with Python 3.11+ and the matching wasm-bindgen CLI:

```sh
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --version 0.2.128 --locked
python3 scripts/build_web_package.py
```

The result is `target/web-package`. Install that directory as a local application
dependency, or follow the [Vite deployment example](examples/vite/main.js). Keep
its worker, JavaScript glue and WASM file together. The default package includes
model tools; `--full` also includes material authoring and additional target APIs.

```ts
import { Toolbox, type QueryRequest } from '@usd-toolbox/web';

const request: QueryRequest = {
  filter: { node_type: 'mesh' },
  fields: ['name', 'metadata', 'bounds'],
  limit: 50,
};

// Accept a self-contained GLB or USDZ File from your application.
async function inspectAndPreview(file: File) {
  const toolbox = new Toolbox();
  try {
    const model = await toolbox.open(await file.arrayBuffer(), { name: file.name });
    const page = await model.query(request);
    const { bytes, diagnostics } = await model.export('glb', { textureTier: 'preview' });
    return { page, bytes, diagnostics }; // bytes is an owned Uint8Array.
  } finally {
    toolbox.dispose(); // Releases the worker and every model it owns.
  }
}
```

Keep a toolbox and its models alive across repeated queries in an interactive
application; `model.dispose()` releases one model. Applications supply external
dependencies through `assets`. The standard build needs neither SharedArrayBuffer
nor cross-origin isolation. Cancelling queued work preserves other handles;
cancelling an active synchronous conversion terminates its worker and invalidates
all models in that toolbox. See [web ownership, cancellation and deployment](packages/web/README.md).

## Rust, C and material workflows

The Rust workspace separates reusable data from host integration:

| Crates | Responsibility |
|---|---|
| `usd-toolbox-core` | Neutral scenes/materials, typed metadata, retained queries, validation and edits |
| `usd-toolbox-usd`, `usd-toolbox-gltf`, `usd-toolbox-materialx` | Format import/export and compatibility profiles |
| `usd-toolbox-materials`, `usd-toolbox-textures`, `usd-toolbox-procedural` | Material-library operations, texture processing and deterministic recipes |
| `usd-toolbox-revit`, `usd-toolbox-omniverse` | Additional delivery targets |
| `usd-toolbox-cli`, `usd-toolbox-wasm`, `usd-toolbox-ffi` | Filesystem, browser and native host boundaries |

Build the C library with `cargo build --release -p usd-toolbox-ffi`. The
[generated header](crates/ffi/include/usd_toolbox.h) and
[executable C example](examples/c/document.c) cover retained documents and owned
results. Buffer sizing does not repeat prepared model exports or edits.

Materials remain independently usable: inspect and audit packages, edit or attach
textures, generate safe downscales, preserve source graphs, bake procedural recipes,
and export to supported targets. Start with [material workflows](docs/material-workflows.md),
[MaterialX](docs/materialx.md), [target adapters](docs/targets.md) or
[host integration](docs/integration.md). The toolkit has no dependency on its
consumer applications, including OPAL.

## Supported profiles

- **Portable USD:** static, precomposed polygon scenes with supported transforms,
  primvars, bindings and PreviewSurface networks. Sublayers, references, selected
  variants and instances require the separate [native OpenUSD adapter](docs/usd-profiles.md).
- **glTF:** static triangles, strips and fans, shared meshes, supported PBR
  materials and documented extensions. Animation, skinning, morph targets,
  cameras/lights and compressed mesh/texture extensions are outside the
  [current profile](crates/gltf/README.md).
- **Images and shading:** PNG/JPEG processing; EXR/UDIM import and arbitrary
  shader baking are unavailable. Retaining source graphs does not imply evaluating
  them. See the [texture profile](crates/gltf/texture-profile.md).
- **Geometry:** subdivision tessellation and CAD/BIM adapters are outside the
  static workflow. Simplification bounds original-vertex displacement, may stop
  above its target, and does not guarantee complete surface or texture error.
  Rendering and shader code generation belong to consumer applications.

## Performance and verification

Queries reuse indexes and bounded caches. Small metadata edits preserve geometry
and image allocations; exports use indexed geometry and explicit resource budgets.
The [native baseline](docs/performance.md) records a 1.15 µs projected-query p95
and a 0.022 ms metadata edit on a synthetic 10,000-placement scene, measured on a
Ryzen AI 9 HX 370 in a release build. These are workload-specific measurements.

[Browser measurements](docs/web-performance.md) cover initialization, transfer,
queries, conversion and WASM memory. Chromium, Firefox and WebKit have run the
worker conformance suite. Linux GNU/musl C consumers have been exercised;
Windows/macOS execution remains pending. A throttled container is not a physical
mobile-device benchmark. See [recorded validation evidence](docs/validation-evidence/README.md)
and [remaining delivery gates](todo.md).

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo check -p usd-toolbox-wasm --target wasm32-unknown-unknown
```

The [CI workflows](.github/workflows) cover correctness, browser execution,
reference validators, bounded fuzzing and performance regression checks.
[Package workflow runs](https://github.com/geyervalmont/usd-toolbox/actions/workflows/artifacts.yml)
provide versioned native/browser artifacts with headers or TypeScript declarations,
examples, checksums and dependency notices when their builds succeed.

## Licence

This repository does not currently declare a project distribution licence.
Third-party licences and fixture attribution remain applicable; they do not grant
a licence for the project itself. The generated npm package is marked `private`
and is not published to the npm registry. See [vendored parser provenance](vendor/openusd/PATCHES.md)
and [corpus attribution](fixtures/corpus/README.md).
