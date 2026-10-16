# Portable static glTF scenes

The scene adapter imports GLB and glTF into the neutral scene and exports indexed
GLB/glTF. The adapter has no filesystem, network, async-runtime, browser-global,
or native OpenUSD dependency. It builds for `wasm32-unknown-unknown`.

```rust
use usd_toolbox_gltf::scene::{import_scene, export_scene};
# fn example(bytes: &[u8]) -> Result<(), usd_toolbox_core::scene::SceneError> {
let imported = import_scene(bytes, "chair.glb", &Default::default())?;
let output = export_scene(&imported.scene, &Default::default())?;
# Ok(())
# }
```

`import_scene_bundle(root_name, files, options)` accepts a `BTreeMap<String,
Vec<u8>>` supplied by the host. `discover_dependencies(bytes, source_name, limits)`
returns normalized external buffer/image paths before import. Relative URIs are
resolved beside the root document. Percent escapes are decoded before validation;
absolute paths, schemes, query strings, fragments, backslashes and parent traversal
are rejected. No ambient filesystem or network fallback exists.

`preflight_scene(bytes, source_name, limits)` returns
`usd-toolbox.gltf-preflight.v1`. It accumulates authored feature blockers without
loading external dependencies, decoding images or materializing geometry. It
explicitly defers full buffer, geometry, image and integrity checks. The scan has
work and diagnostic limits, marks incomplete results, and returns metadata key
inventories instead of arbitrary large metadata bodies. A compatible preflight
result is not proof that full import or a particular export target will succeed.

`gltf_scene_capabilities()` returns `usd-toolbox.gltf-capabilities.v1` so wrappers
can discover the supported profile.

## Import and preservation

Supported geometry includes static hierarchies, shared meshes, indexed/unindexed
triangles, strips and fans, interleaved buffer views, sparse accessors, normalized
UV/colour attributes, normals, authored tangents, and PNG/JPEG PBR textures.
Accessor spans, alignment, component types, sparse ordering, indices, authored
bounds, finite values and configured resource budgets are checked before use.
Images are dimension-checked and decoded once on import to validate their payload;
preflight deliberately skips this work.

Animation, skins, morph targets, cameras, lights, compressed mesh/texture
extensions and unsupported shader extensions are rejected with stable error codes.
The adapter chooses the declared default scene, or scene zero, and reports omitted
scene definitions. Multiple primitives are represented as child mesh nodes while
shared geometry remains shared. Repeated-index degenerate triangles are omitted
with a diagnostic. This is an explicit static profile, not a complete glTF schema
validator.

Supported extensions:

- `KHR_materials_ior`
- `KHR_materials_emissive_strength`
- `KHR_materials_transmission`
- `KHR_materials_clearcoat` without an independent coat normal map
- `KHR_texture_transform`

Original glTF extras and material descriptions are retained as typed metadata.
Integer metadata survives Rust/JSON without loss of 64-bit values; JavaScript
consumers use the typed decimal-string integer representation. Toolbox-authored
stable scene, node and mesh identities and typed metadata survive GLB round trips.
Geometry is still triangulated and encoded as float32 on export.

Material settings without direct neutral surface fields live in the material
metadata property `gltf:render_settings`:

```json
{
  "alpha_mode": "MASK",
  "alpha_cutoff": 0.5,
  "textures": {
    "base_color": {
      "tex_coord": 0,
      "sampler": {"wrapS": 10497, "wrapT": 33071},
      "transform": {"offset": [0, 0], "scale": [2, 2], "rotation": 0}
    },
    "normal": {"tex_coord": 0, "sampler": {}, "scale": 0.7}
  }
}
```

Scene GLB export restores these settings using newly assigned texture indices.
Other format exporters must either map them or report unsupported projection;
retaining metadata alone does not evaluate a shader. Named `st`, `st1`, `st2`, etc.
map to `TEXCOORD_0`, `TEXCOORD_1`, `TEXCOORD_2`. Authored tangent handedness is
converted with the UV origin on import/export. Missing tangents are generated per
triangle; this generator is not advertised as MikkTSpace-compatible.

## Indexed export and measured evidence

Render vertices are deduplicated by position, normal, every UV set, colour and
tangent bits. Material primitives remain separate; UV seams, hard normals,
mirrored tangent handedness and vertex colours remain distinct. Hash-map iteration
does not affect output order: vertices are emitted on first encounter. Index width
is selected automatically, excluding the primitive-restart sentinel reserved by
glTF. Shared meshes with matching bindings reuse one encoded mesh. Reusable scratch
vectors avoid allocating a new attribute key for every repeated corner.

`SceneGltfOptions.optimize_vertex_cache` optionally reorders opaque,
non-transmissive primitives and remaps all vertex streams after measured cache
simulation checks. It defaults to false and preserves transparent triangle order.
See [vertex-ordering.md](vertex-ordering.md) for bounded-work guarantees, native/
WASM parity, corpus measurements and visual comparisons. Actual texture tier
minification and its supported decoder profile are documented in
[texture-profile.md](texture-profile.md).

On the local Linux development machine, a release build exported a 256×256 quad
grid (66,049 points, 65,536 faces) in 72.5 ms in one measured run. The indexed GLB was
3,159,340 bytes; the previous unindexed geometry alone required 9,437,184 bytes, a
66.52% reduction. This is a reproducible synthetic size baseline, not a latency
promise or a complete memory-capacity measurement.

```sh
cargo test -p usd-toolbox-gltf
cargo check -p usd-toolbox-gltf --target wasm32-unknown-unknown
cargo test -p usd-toolbox-gltf --release indexed_grid_benchmark -- --ignored --nocapture
cargo test -p usd-toolbox-gltf --release --test corpus -- --ignored --nocapture
```

The pinned, licensed corpus manifest is `fixtures/corpus/manifest.json`; inputs
live outside Git under `target/corpus`. The corpus test verifies hashes and writes
exports and reports under `target/corpus-export`. Box, BoomBox and
NormalTangentMirrorTest passed import/export and Khronos validation with zero
errors/warnings. SheenChair and RecursiveSkeletons exercise explicit unsupported
extension/animation paths.

`tests/render_corpus.cjs` compares actual Chromium/Three renders of original and
round-tripped models at three camera angles. The measured comparisons were pixel
identical for Box and BoomBox; the normal/tangent fixture differed by one channel
value in one view (mean absolute RGB error below 0.000003/255). Run it with
`PLAYWRIGHT_MODULE`, `THREE_ROOT` and optionally `CHROMIUM_EXECUTABLE` pointing to
installed packages/browser. Structural validation alone does not establish visual
fidelity for every model.

Resource checks separately bound encoded sources, decoded accessor work, decoded
image pixels and output content using configured scene limits. Decoder scratch
limits are best effort. Complete JSON/binary buffers and neutral geometry are
currently materialized; these checks are not guarantees on peak RSS or WASM
linear-memory high-water marks.
