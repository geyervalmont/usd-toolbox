# Optional vertex-cache ordering

`SceneGltfOptions.optimize_vertex_cache` defaults to `false`. Enabling it attempts
cache-local triangle ordering and first-use vertex remapping inside individual
opaque, non-transmissive material primitives. MASK, BLEND, opacity-mapped and
transmissive primitives keep their original triangle and vertex order. Material
boundaries and shared-mesh reuse remain intact. The default export is unchanged.

The deterministic traversal builds compressed adjacency lists, drains each
visited vertex's remaining triangles and prefers cached vertices with small live
valence. Every adjacency/stack entry is processed at most once; choosing the next
cached vertex examines at most sixteen entries. Work is O(indices + 16×vertices).
The scratch estimate is checked against remaining configured byte budget before
allocation. If it would not fit, export retains original ordering and reports
`gltf.vertex_cache_skipped` instead of rejecting otherwise valid geometry.

Each candidate is compared against the original using independent FIFO-16 and
FIFO-32 post-transform cache simulations. A candidate is adopted only when
neither miss count increases and at least one decreases. The adopted candidate
then remaps *every* attribute stream using the same first-use permutation. It
never welds extra vertices, changes attribute values, rotates triangle corners or
changes winding. UV seams, hard normals, colours and tangent handedness survive
bit-for-bit. An accepted change reports `gltf.vertex_cache_optimized` with both
before/after miss counts. No new shipping dependency was added.

Cache simulation is a proxy. It does not establish the cache architecture or GPU
frame time of a particular browser/device. Ordering also does not reduce vertex
count or uncompressed file size; the measurements below report unchanged bytes.
Rasterization of coplanar/depth-tied triangles can remain order-sensitive even
with opaque materials. The option is deliberately explicit and off by default.

## Measured evidence, 2026-10-16

Local Linux release build; one warmup followed by five exports, reported as p50.
Raw timings are retained in the reports. CPU load and frequency vary: these are
observations, not a claim that adding optimizer work makes export faster.

| Fixture | FIFO-16 misses before → after | FIFO-32 misses before → after | Export p50 ms before → after | GLB bytes, unchanged |
|---|---:|---:|---:|---:|
| 256² quad grid, row order | 131,584 → 130,818 | 131,584 → 130,808 | 56.27 → 40.73 | 3,159,824 |
| 256² quad grid, shuffled faces | 262,144 → 130,818 | 262,144 → 130,808 | 36.63 → 41.98 | 3,159,824 |
| Khronos BoomBox | 5,343 → 4,718 | 4,958 → 4,164 | 17.85 → 18.34 | 10,617,444 |
| Khronos Box | 24 → 24, unchanged | 24 → 24, unchanged | 0.096 → 0.053 | 3,520 |
| Khronos NormalTangentMirrorTest | 5,330 → 5,330, unchanged | 3,240 → 3,240, unchanged | 4.40 → 4.65 | 1,571,372 |

BoomBox improves simulated misses by 11.7% at sixteen cache entries and 16.0% at
thirty-two entries. The shuffled grid improves by roughly 50.1%; its average
cache misses per triangle fall from 2.0 to 0.998. Candidates for Box and the
normal-map fixture are discarded because they do not satisfy the benefit gate.

Five regular regression tests cover all attribute streams and winding,
first-use remapping, deterministic bytes, mixed material boundaries, MASK/BLEND/
opacity/transmission exclusion, insufficient scratch budget and no-benefit
fallback. The seam fixture has two UV sets, split normals, colours and mirrored
authored tangents. Complete deindexed triangle records are compared bit-for-bit
as multisets, preserving each triangle's corner order. Its deliberate shuffled
order must improve both cache simulations by more than 50%.

The three real corpus inputs are checked against their pinned manifest hashes.
Every deindexed corner/attribute tuple remains identical after optimization, and
all resulting GLBs pass Khronos validation with zero errors and warnings. At
three Chromium/Three camera angles, Box and NormalTangentMirrorTest rendered
pixel-identically to their source. BoomBox was identical in two views; in the
third, total absolute RGB error was three across 442,368 channel samples
(mean 0.000006782/255), with no channel differing by more than three. This is a
bounded visual fixture, not a universal statement about all viewing conditions.

The native and bare `wasm32-unknown-unknown` runtime probes both returned the
complete-export checksum `15636585764341450986` for a shuffled grid with UV,
colour and authored tangent streams. The probe verifies that the option actually
changes ordering and keeps the output byte count unchanged. Its WASM requires no
imports, browser APIs or WASI.

```sh
cargo test -p usd-toolbox-gltf --test ordering
cargo test -p usd-toolbox-gltf --release --test ordering -- --include-ignored --nocapture --test-threads=1
CORPUS_EXPORT_DIR=target/corpus-optimized node crates/gltf/tests/render_corpus.cjs
cargo run -p usd-toolbox-gltf --release --example ordering_portability
cargo build -p usd-toolbox-gltf --release --target wasm32-unknown-unknown --example ordering_portability
node -e 'WebAssembly.instantiate(require("fs").readFileSync("target/wasm32-unknown-unknown/release/examples/ordering_portability.wasm"),{}).then(({instance})=>console.log(String(BigInt.asUintN(64,instance.exports.ordering_checksum()))))'
```

Renderer setup uses the existing `PLAYWRIGHT_MODULE`, `THREE_ROOT` and
`CHROMIUM_EXECUTABLE` environment variables. Synthetic metrics are written to
`target/gltf-ordering-grid.json`; corpus exports, timings, browser comparison and
validator reports are under `target/corpus-optimized`.
