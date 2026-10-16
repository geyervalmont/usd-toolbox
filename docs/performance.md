# Model performance and resource evidence

Updated: 2026-10-16. These are measured **synthetic native workloads**, not capacity
promises for customer models or mobile browsers. The machine-readable baseline is
[performance-baseline.json](performance-baseline.json); it records the CPU,
platform, Rust version, workload sizes, binary SHA-256 digests, individual stages,
allocation counts, peak memory and lifecycle checks.

## Reproduce

```sh
cargo build --locked --release -p usd-toolbox-cli
python3 scripts/benchmark_queries.py \
  --output /tmp/usd-performance \
  --toolbox target/release/usd-toolbox \
  --baseline docs/performance-baseline.json
```

The runner builds release examples unless `--skip-build` is supplied. It needs
Linux, Python 3, a C compiler, and Rust; the Rust examples themselves remain
portable. `--quick` reduces the larger input sizes and samples for a smoke test.
`--cpu N` pins the runner and its child processes to a selected available CPU.
`--enforce-timing` is intended only for a controlled baseline host: it requires
matching CPU, OS, architecture, Rust version, CPU affinity and workload mode.

The instrumented example tracks successful allocation calls, requested allocation
bytes, live requested bytes and high-water requested bytes. Tracking adds atomic
operations, so compare instrumented results with the same executable type. RSS is
measured with `wait4` through a tiny freshly executed C launcher; this prevents the
Python parent's high-water RSS from being attributed to a short-lived CLI process.
RSS includes fixture construction and serialization for each benchmark process.
Allocation counters exclude allocator bookkeeping and fragmentation.

For an uninstrumented, focused query measurement:

```sh
cargo run --release -p usd-toolbox-core --example query_benchmark -- 10000 1000
cargo run --release -p usd-toolbox-core --example geometry_benchmark -- 32
```

## Current native baseline

Host: AMD Ryzen AI 9 HX 370, Linux x86-64, Rust 1.98.0, release optimization. The
runs were not isolated from every desktop/background task. Latencies below use
1,000 warm repetitions of a stable ID/path or one projected exact-ID query; they
do not represent random access, network latency, browser initialization or GPU
upload. Opening a document includes validation, index creation and a streamed
content digest. JSON parsing is measured separately.

| Synthetic workload | JSON parse | Document open | Lookup p95 | Query p95 | Small metadata edit | Peak process RSS |
|---|---:|---:|---:|---:|---:|---:|
| 10,000 placements of one mesh | 15.45 ms | 29.59 ms | 71 ns | 1.15 µs | 0.022 ms | 46.9 MiB |
| 50,000 root nodes, shared mesh | 80.03 ms | 182.28 ms | 100 ns | 1.17 µs | 0.028 ms | 217.6 MiB |
| 10,000 nodes in chains of 128 | 15.79 ms | 35.48 ms | 71 ns | 1.10 µs | 0.024 ms | 48.7 MiB |
| 5,000 small independent meshes | 11.25 ms | 14.87 ms | 61 ns | 1.11 µs | 0.021 ms | 30.3 MiB |
| 128×128 dense grid, 32,768 triangles | 2.05 ms | 2.37 ms | 40 ns | 1.75 µs | 0.043 ms | 6.0 MiB |
| 100 nodes, four encoded 512² PNG payloads | 67.44 ms | 21.17 ms | 41 ns | 1.02 µs | 0.024 ms | 59.7 MiB |

The texture workload retains four independently owned PNG byte arrays with the
same content; it tests payload ownership and serialization pressure, not image
decoding. Core metadata queries never decode those images. The high parse cost is
an example of why bulk binary transfer should not use scene JSON in web flows.
The fixture's deliberately uncompressed PNG encoding is valid and reproducible.

A previous metadata-only edit cloned all node headers and metadata and rebuilt
indexes. On the 10,000-node fixture it took 37.35 ms. The incremental path takes
roughly 0.02–0.04 ms in these runs, allocates about 10.5 KiB, and preserves node,
mesh and material allocation addresses. It validates changed metadata records,
updates aggregate byte/property budgets and only the affected metadata-index
entries. Rename-only edits retain the same storage and update the name ordering.

Memoized hierarchy validation reduced the 10,000-node deep fixture's document
open from 81.73 ms to about 35 ms. Queries use ID/path indexes and cached subtree
intervals; selective ID/path/metadata filters sort their candidates rather than
walking the complete sorted scene. Pagination stops after the requested page and
one additional match. It does not construct every matching row.

Small source-format stages were also measured with the current release CLI:

| Input | Preflight | Import + inspect | USDZ package | USDZ → GLB | USDZ bytes | GLB bytes |
|---|---:|---:|---:|---:|---:|---:|
| Two textured panels | 2.78 ms | 2.97 ms | 3.90 ms | 3.69 ms | 22,880 | 15,824 |
| Seven-part desk | 2.55 ms | 3.59 ms | 4.25 ms | 3.22 ms | 25,377 | 22,620 |

These whole-process times include process launch and local file operations. They
are separate from the core stage measurements above. Small CLI process peak RSS
was approximately 7–10 MiB. They are too small to establish large-model import or
export throughput. `scripts/benchmark_model.py` remains available for a larger
USD grid and records input/package/preview sizes; it should be run alongside a
representative customer corpus before setting service capacity limits.

## Geometry policy and measurements

The release geometry example starts with a 32×32 planar grid: 1,089 vertices and
2,048 triangles. One initial run measured normal generation at 0.47 ms,
conservative LOD generation at 2.10 ms, and welding at 1.54 ms. Those timings
include atomic `edit_scene` copying and validation. The durable baseline contains
another measured run rather than assuming these numbers are invariant.

The LOD result contains 593 vertices and 1,056 faces, with maximum measured
original-vertex displacement 1.414 scene units. The requested target was 1,024
faces; the report explicitly states that the conservative policy could not reach
it. The displacement limit was two scene units.

The simplifier locks open boundaries, material boundaries, and vertices at
corner-attribute seams. It checks the manifold edge-link condition, local winding
and duplicate triangles. It uses bounded deterministic edge collapse and reports
its work limit and whether the target was reached. Its error bound is **original
vertex displacement**, not surface Hausdorff distance, texture distortion or a
guarantee against intersections with distant geometry. It clears tangents for
regeneration. It is a conservative static preview LOD tool, not a certified
remesher or CAD reduction engine.

Normal generation takes an explicit crease angle and area-weighting choice and
keeps material boundaries. Transform baking handles affine shear, inverse-
transpose normals, mirrors, tangent handedness and shared-mesh occurrences.
Welding requires exact compatibility for vertex/varying attributes and retains
face-corner seams. Degenerate-face removal remaps corner attributes and material
subsets; malformed count streams are rejected. Standalone invalid-index removal
requires an explicit option. Every geometry operation returns before/after counts,
conversion notes and provenance through the edit report.

## Regression checks

The [performance workflow](../.github/workflows/performance.yml) runs relevant
pull requests and can be dispatched manually. It uploads the complete report,
stdout/stderr, resource samples and exact native executables. These deterministic
checks fail independently of CPU speed:

- Repeated stable ID/path lookup and warm bounds lookup allocate nothing.
- A short metadata edit allocates at most **128 KiB**. The measured approximately
  10.5 KiB leaves headroom while catching accidental whole-scene copying.
- Metadata edits preserve node, geometry and material allocation addresses.
- Repeated queries retain **zero additional live requested bytes**.
- Ten open/query/bounds/close cycles retain **zero additional live requested
  bytes** per cycle, including textured models. Lifecycle fixtures are capped at
  1,000 nodes or a 32×32 grid to keep this regression test bounded.
- Explicitly over-limit node input is rejected.
- The deterministic 32×32 LOD stays at or below **1,100 faces** and within the
  requested **two-unit original-vertex displacement** bound.

Timing and RSS comparisons warn at more than **3×** the matching workload's
baseline, with a one-millisecond floor for millisecond timings. Shared hosted CI
does not fail from wall-clock noise. A controlled matching-host run can enforce
those thresholds; sustained regressions should be repeated and profiled before
changing the baseline. These engineering thresholds are not end-user SLAs.

## Memory, caching and portability limits

`SceneDocument` owns its scene privately. No mutation can bypass its generation
and cache policy. Node/path/hierarchy/material indexes are built once. Metadata
indexes are selective; callers choose which properties to index. Bounds and world
transforms are computed on demand and retained within the configured cache
budget. Small metadata, rename, visibility and material-only edits retain those
geometry caches; structural/geometry edits clear them. Page cursors bind the
exact scene-content revision, generation and complete query. Reopening unchanged
content supports CLI pagination; edits and different content invalidate cursors.

Metadata has independent byte/property/depth limits. Queries have result-count,
response-byte, index and cache budgets. Default retained operation history is
limited to 10,000 entries; the next edit fails rather than growing history
indefinitely. These are logical content/work limits, not hard operating-system
memory guarantees. Geometry transactions may retain old and new payloads during
validation. General copy-returning `edit_scene`, merge, extraction and many
geometry changes still copy buffers. Large metadata values and broad indexes can
also be expensive even when they are within their explicit budgets.

Full encoded inputs, decoded geometry, image decoder scratch, indexes, cached
bounds and complete exports are distinct allocations. Format adapters can still
parse an input more than once across separate preflight/import/inspection calls;
metadata-only parser support is format-specific. The reusable document removes
repeated import work for queries, but does not make all format conversions
streaming. Use binary buffers and a retained worker/document for browser work,
not repeated scene-JSON serialization.

Freeing a document releases its owned allocations. Allocator capacity can remain
in a native process, and WASM linear memory does not automatically shrink; zero
live-byte growth is not a claim that RSS or a browser's linear-memory size returns
to its initial value. Worker teardown remains the reliable browser fallback for
reclaiming an entire model runtime.

This report contains no physical constrained-device, Firefox/WebKit performance,
WASM download-size or GPU-render throughput claim. Browser correctness tests and
emulated mobile tests are useful separate evidence; a physical constrained-device
baseline and a representative licensed customer-model corpus are still required
before making those performance claims. SIMD, parallelism, quantization and
compression should be judged against those workloads and decoding/startup costs,
not added on the assumption that smaller files always make a faster application.
