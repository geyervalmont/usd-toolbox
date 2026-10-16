# Static model toolbox

The toolbox handles static model scenes and reusable materials through Rust, CLI,
WASM and C. OPAL consumes these APIs but is not a dependency. The original 0.3.0
scene work now includes metadata queries, glTF import, geometry tools, browser
workers and explicit compatibility profiles. The checklist is in [todo.md](../todo.md).

## Model workflow

```text
USD / USDZ + supplied files     GLB / glTF + supplied files
              \                 /
               neutral Scene v1
         hierarchy + shared meshes + materials + metadata
                           |
              retained indexed SceneDocument
         preflight / query / inspect / edit / extract / diff
                           |
             canonical USDZ / indexed GLB previews
```

Portable import accepts static precomposed USD and static triangle glTF. The
optional [native adapter](usd-profiles.md) creates portable snapshots from real
OpenUSD composition, selected variants/payloads and instances. It is a separate
host preprocessing step; the WASM package does not load native USD binaries.

## Core scene and metadata

`core::scene` defines nodes, meshes, attributes, bindings, units, typed metadata,
limits and operations. Nodes can share one immutable mesh payload. Matrices are
row-major, act on column vectors and compose as `parent * local`; reset stacks
ignore ancestor transforms. Units are metres per unit; up axis is Y or Z.

Inspection bounds cover all placed geometry in scene units. Query bounds cover
one node's own mesh in world metres and include their derivation/generation.
Missing direct materials inherit the nearest ancestor's binding; face subsets
have explicit nonoverlapping assignments. Effective visibility and purpose are
separate from authored values. Normals, tangents, named UV sets and colours retain
supported interpolation/indexing semantics.

Metadata exists at scene, node, mesh and material scope. Source properties and
normalized catalogue fields are separate. Source locations/types and authored,
inherited, defaulted or unresolved state remain visible. Exact 64-bit integers
cross JavaScript as tagged decimal strings. Supported USD customData, assetInfo,
kind and custom attributes are preserved and re-authored; graph archives retain
source evidence without claiming arbitrary shader evaluation.

See [query-api.md](query-api.md) for schemas, migrations, filters, field selection,
stable sorting, cursor revisions, selective indexes and all runtime interfaces.
The default query excludes mesh buffers and encoded images.

## Editing and geometry

`SceneDocument::apply` is atomic. Rename and metadata updates change only relevant
records and indexes, preserve payload allocations and advance generation.
Structural/geometry operations invalidate derived data as needed. The standalone
`edit_scene` API remains a copy-returning convenience.

Supported operations include rename, add/remove/reparent, subtree duplication,
model merge, metadata, transforms, visibility, unit/axis conversion, pivot changes,
material assignment/rebinding and subtree extraction. Merge/duplication require
explicit prefixes and reject identity collisions. Reparenting exposes world and
inheritance preservation policies. Stable IDs/source paths survive rename.

Geometry tools include deterministic triangulation, crease/area-weighted smooth
normals, transform baking (including shear/mirror/normal handling), compatible
vertex welding, degenerate removal and conservative simplification. They report
counts, measured vertex displacement, limitations and provenance. Invalid-index
repair is explicit; malformed topology is not silently reinterpreted. [Geometry
LOD sets](geometry-lods.md) and texture tiers are separate derivatives. LOD export
is available in Rust, the CLI, C prepared outputs and the browser worker package.

The simplifier locks open/material/attribute boundaries, checks local manifold and
winding conditions, and can stop above the requested face target. Its error bound
is original-vertex displacement, not a global surface-distance or intersection
proof. Large coordinates/nonplanar polygons are checked by triangulation; lossy
float32 target projection is reported. These tools are not a CAD remesher.

## Import, preflight and export profiles

[USD profiles](usd-profiles.md) describe the portable shader, binding, inherited
primvar, metadata, inactive-prim and native snapshot semantics. Preflight inventories
unsupported prim/schema types, composition, dependencies, animation and material
networks even when neutral import fails. Reports have bounded findings, stable
codes, source paths, severity, remedies and explicit completeness/deferred checks.
Metadata scans skip images and binary mesh arrays where the parser permits it;
USDA still materializes authored text values.

[glTF support](../crates/gltf/README.md) includes static GLB and JSON glTF,
interleaved/sparse/indexed accessors, transforms, material/texture semantics and
caller-supplied external dependencies. Unsupported animation, skinning, morphs and
extensions fail explicitly. Import retains source extras and exact metadata.

GLB export triangulates planar polygons and deduplicates compatible vertex/corner
attributes, preserving seams, hard edges and material boundaries. It selects
16/32-bit indices, shares compatible placements and emits normal-map tangents.
UV conventions, units, axis, mirrored and nonuniform transforms are mapped
explicitly. Shear requires deliberate baking. Hidden/proxy/guide geometry is
omitted with diagnostics unless requested.

[Preview textures](../crates/gltf/texture-profile.md) actually downsample imported
images to the requested tier. Filtering distinguishes colour, data, normals and
alpha; sources stay unchanged. Shipping decoders remain PNG/JPEG. EXR has an
isolated native/WASM assessment; EXR/UDIM, HDR policy and arbitrary shader baking
are not advertised capabilities.

USDZ and GLB are self-contained. Standalone USDA/USDC need `export_scene_assets` /
`model.assets()` sidecars. The CLI confines sidecars to the output directory and
refuses to replace different content. Input resolution also confines canonical
paths and rejects missing, duplicate or escaping dependencies. Libraries perform
no filesystem/network resolution.

## CLI and runtime examples

```sh
usd-toolbox model preflight --input model.usda --target glb --geometry
usd-toolbox model query --input model.usdz --request query.json
usd-toolbox model metadata --input model.usdz --node /Asset/Part --key product_id
usd-toolbox model pack --input model.glb --output model.usdz --report build.json
usd-toolbox model export --input model.usdz --target glb --tier preview --output preview.glb
usd-toolbox model apply --input model.usdz --edits edits.json --output changed.usdz
usd-toolbox model extract --input model.usdz --node /Desk/Top --output top.usdz
usd-toolbox model diff --before model.usdz --after changed.usdz --output diff.json
```

Write commands protect source files. Material bindings can pin an independent
library USDZ by source material, package path and SHA-256. Build reports include
canonical inspection, diagnostics, output hashes/size and builder version.

The [typed web package](../packages/web/README.md) retains model handles in a
single-threaded module worker. It supports copied/transferable binary inputs,
phase progress, abort, metadata/query/edit/export and explicit disposal. Synchronous
Rust stages use worker termination for immediate cancellation; aborting active
work invalidates that worker's handles. No borrowed memory view escapes WASM.
Freed linear-memory allocations are reusable; worker teardown releases the runtime.

The C API offers retained document and prepared-result handles in addition to
legacy JSON/buffer operations. Prepared results prevent conversion being repeated
for two-pass sizing. Borrowed bundle inputs avoid JSON-encoded binary arrays.
The [C example](../examples/c/document.c) compiles and executes against the real ABI.

## Limits and evidence

Native defaults remain 100,000 nodes, depth 256, five million unique points/faces,
15 million corners, 1,024 corners per polygon, 10,000 dependencies and 512 MiB.
Browser defaults remain 20,000 nodes, one million points/faces, three million
corners and 128 MiB. Metadata, query responses, indexes, caches, decoded resources
and operation history have additional budgets. These logical limits are not
process-RSS promises. No advertised model-size limit was raised from synthetic tests.

The vendored `openusd 0.7.0` parser has checked allocation/count/offset/work/depth
limits and deterministic regressions for a USDC allocation issue discovered by
fuzzing. Eight sanitizer targets cover formats, images, metadata and queries.
Project-authored USD fixtures exercise hierarchy, metadata widths, transforms,
material interfaces and geometry semantics; a pinned licensed public glTF corpus
adds validator and browser image comparisons. Customer/exporter-version coverage
still needs permissioned real assets.

[Native benchmarks](performance.md) record release timing, allocation counts,
peak RSS, copy/cache/lifecycle checks and regression gates. Browser evidence is
measured separately. Actual Chromium, Firefox and WebKit worker execution and a
Vite production deployment are tested. Physical mobile capacity and non-Linux
platform execution require their own evidence; configured CI jobs alone are not
successful platform tests. Native/WASM comparisons check projected queries,
metadata, edits, diagnostics and deterministic exports.

Rendering, shader compilation, CAD/BIM adapters, rigging and animation remain
separate consumer/project boundaries. See [future scope](future-model-scope.md).
