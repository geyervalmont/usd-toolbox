# USD toolbox TODO

Updated: 2026-10-16. This tracks the model-toolbox goal started after the 0.3.0 static-model
implementation. Checked items have implementation or an explicit profile decision;
unchecked items still require work or external validation. Limits below remain explicit.
See [current capabilities and limits](docs/model-toolbox.md) for what works today.

The priorities are broad model compatibility, fast processing, easy metadata
queries, and reliable use in the web through WASM. Rust, CLI, WASM and C should
expose the same portable operations with explicit capability differences.

## Goal evidence and remaining gates

Implementation: [model toolbox](docs/model-toolbox.md), [query/metadata contracts](docs/query-api.md),
[USD profiles and optional native adapter](docs/usd-profiles.md), [browser package](packages/web/README.md).
Evidence: [native performance](docs/performance.md), [texture/EXR assessment](crates/gltf/texture-profile.md),
[compression assessment](crates/gltf/compression-assessment.md), [recorded validation](docs/validation-evidence), [full native shader registry](docs/reference-validation.md),
[LOD packages](docs/geometry-lods.md) and [vertex ordering](crates/gltf/vertex-ordering.md).

The licensed public corpus and synthetic conformance fixtures are present. They
are not yet a permissioned customer/exporter-version corpus. Actual Chromium,
Firefox and WebKit plus Vite production deployment pass. A 1-CPU/1-GiB container
benchmark is recorded; it is not a physical constrained/mobile-device result.
Linux GNU/musl, Windows MSVC and macOS Intel/ARM builds and executable C consumers
pass; [CI evidence](docs/validation-evidence/github-ci.json) records the tested
commit, jobs and package artifacts. CI artifacts follow the repository's public
visibility. The project has no declared distribution licence, and the npm package
remains marked private.

Profile decisions do not mean every format feature is implemented. Composition
and instancing use an optional native snapshot adapter. Subdivision is rejected
without an approved evaluator. EXR/UDIM and mesh/texture codecs were assessed and
remain outside shipping support until their fidelity, resource and corpus gates
are met. The future CAD/BIM/animation items below are scoping decisions, documented
in [future model scope](docs/future-model-scope.md), not new import capabilities.

## Delivery order

| Priority | Work | Result |
|---|---|---|
| P0 | Representative corpus, performance baselines, preflight | Know what real inputs need and how much processing costs |
| P0 | Metadata contract, query API and indexes | Find objects and retrieve useful metadata without exporting an entire scene |
| P0 | Browser worker API, cancellation and lifecycle | Keep the web application responsive with predictable memory ownership |
| P1 | GLB/glTF import and broader USD/material compatibility | Accept more externally authored models |
| P1 | Indexed geometry, fewer copies and cached derived data | Reduce processing time, memory and preview size |
| P1 | Model editing and metadata-preserving round trips | Prepare reusable assets without losing their identity or descriptive data |
| P2 | Bounded USD composition, instancing, LODs and compression | Extend the asset types and scale supported by measured demand |

P0 work should establish a small vertical slice: open a model in a browser worker,
report compatibility, query its hierarchy and metadata, retrieve one page of
results, cancel work, and release memory. Verify the same queries natively.

## 1. Representative inputs and compatibility preflight

- [ ] Build a licensed or permissioned corpus of models exported by the tools our
  users actually use. Include textured products, furniture, assemblies, repeated
  components, large hierarchies and difficult geometry; retain exporter versions.
- [x] Keep minimal synthetic fixtures for individual semantics and regression
  cases. Record expected acceptance, rejection and loss reports for each input.
- [x] Add a preflight scanner independent of successful neutral-scene import.
  Today `model inspect` requires import and profile rejection generally reports
  the first blocker. Accumulate recoverable compatibility findings in one pass.
- [x] Report prim/schema types, references, payloads, variants, instances,
  animation, material networks, dependencies and required features. Distinguish
  malformed data from valid features the toolbox cannot yet interpret.
- [x] Bound diagnostic count and scan work. Report when scanning was incomplete;
  do not claim to have found every issue after a fatal parse failure or limit.
- [x] Add target-specific preflight for USDZ, GLB and browser preview, including
  triangulation, shear, missing UVs, shading losses and resource estimates.
  Generic scene validity alone does not establish exportability.
- [x] Return stable issue codes, source locations, severity and suggested remedies.
  Make repair/conversion plans reviewable; apply changes only when requested.
- [x] Expose a versioned scene capability matrix in Rust, CLI, WASM and C so
  applications can discover supported import, query, edit and export features.

## 2. Metadata contract and preservation

- [x] Add typed metadata containers at scene, node, mesh and material scope.
  Cover labels, descriptions, tags, classifications, product/manufacturer IDs,
  source identifiers, units and provenance without tying the core to OPAL or BIM.
- [x] Preserve supported USD `assetInfo`, `customData`, `kind` and custom
  attributes with namespace, value type and source location. Inventory layer
  metadata and define which entries can be preserved, projected or only reported.
- [x] Define missing, explicitly empty/null, authored, defaulted, inherited and
  unresolved values. Never return an unresolved value as a resolved answer.
- [x] Separate original metadata from normalized catalogue fields and computed
  values such as bounds. Include the derivation/source of computed results.
- [x] Add explicit per-property loss reports for metadata that cannot round-trip.
  Original source retention alone does not make a generated USDZ lossless.
- [x] Preserve stable object identity across rename, reparent, extraction and
  export. Define collision handling when combining models and preserve source IDs.
- [x] Version metadata schemas and migration rules. Test nested dictionaries,
  Unicode, large integers and typed arrays across Rust, JSON, JavaScript and C.
- [x] Bound metadata depth, size and returned payloads independently of geometry.
  Keep large opaque payloads out of routine metadata responses.

## 3. Query API: easy access without full scene serialization

- [x] Introduce a reusable scene/document handle that supports many queries after
  one import. Keep geometry and indexes in Rust/WASM memory between calls.
- [x] Support direct lookup by stable ID and source prim path; retrieve one object,
  its metadata, parent, children, ancestors or descendants.
- [x] Support structured filters for name/path, node type, metadata existence or
  value, material binding, visibility, purpose and subtree. Specify case handling,
  numeric comparison and missing-value semantics; avoid arbitrary code execution.
- [x] Provide field selection, stable sorting, bounded limits and pagination.
  A request for names and bounds must not serialize positions or texture bytes.
  Bind cursors to the scene revision and query so edits cannot silently mix pages.
- [x] Return effective material bindings, local/world transforms, physical bounds,
  dependencies and source hashes on request. Make expensive computed fields opt-in.
- [x] Expose summary/count queries without requiring clients to fetch every row.
  Index frequently used metadata fields selectively rather than indexing all values.
- [x] Build ID/path and parent/child indexes once; cache transforms and bounds.
  Invalidate the affected entries after edits using a defined scene generation.
  Ensure mutation cannot bypass cache invalidation.
- [x] Add a metadata-focused open/scan mode that avoids image decoding and avoids
  geometry materialization where the underlying format/parser permits it. Report
  which validation and integrity checks were performed, deferred or unavailable.
- [x] Expose the same query semantics through Rust, `model query` JSON output,
  WASM/TypeScript and C. Use structured requests first; a textual query language
  can be a later convenience layer.
- [x] Test result equivalence across interfaces, deterministic pagination, stale
  cursor handling, missing fields and invalid filters.

Implemented retained-document API: `open`, `get_metadata`,
`get_node`, `children`, `query`, `summary`, and `capabilities`. See [query contracts](docs/query-api.md) and the [web package](packages/web/README.md)
for exact names, options, ownership and examples across interfaces.

## 4. Model ingestion and USD semantics

- [x] Implement static GLB/glTF import into the neutral scene: hierarchy, indexed
  geometry, transforms, materials, textures and embedded/external buffers. Use
  caller-supplied dependency resolution and document supported extensions.
- [x] Validate accessors, component types, strides, sparse data, bounds and budgets;
  reject unsupported compression/extensions with actionable diagnostics.
- [x] Define a staged USD composition profile for sublayers, references, payloads
  and variant selection, including dependency resolution and opinion strength.
  Do not approximate composition by concatenating layer contents.
- [x] Decide whether each composition feature belongs in portable Rust or an
  optional native OpenUSD preprocessing adapter. An optional adapter must not
  become a hidden requirement for browser workflows; expose capability differences.
- [x] Add native USD instance/prototype and point-instancer handling in stages,
  preserving shared geometry where supported instead of automatically expanding it.
- [x] Expand inherited primvars, binding strength, collection/purpose bindings,
  inactive prims and supported applied schemas using conformance fixtures.
- [x] Define subdivision/hole/crease handling and explicit tessellation policies.
  Report approximation; never treat a subdivision control cage as equivalent
  polygon geometry without a requested conversion.
- [x] Keep unsupported cameras, lights, curves and other prim types visible in
  preflight; define explicit preservation or omission policies for each profile.

## 5. Material and texture compatibility

- [x] Expand external PreviewSurface handling: texture transforms, wrapping modes,
  UV-set selection, cutout opacity and additional supported parameter connections.
- [x] Resolve interface inputs and supported NodeGraph/MaterialX networks and
  render contexts. Keep unsupported connections explicit rather than guessing.
- [x] Preserve source graphs when a preview is only an approximation. Separate
  graph preservation from the ability to evaluate or bake that graph.
- [x] Add EXR/other required data-map decoders only after verifying native/WASM
  support, decoding limits and colour-space behaviour. Evaluate tiled/UDIM inputs
  against actual corpus requirements.
- [x] Generate appropriately sized preview texture tiers for external models;
  retaining full-size source images in a preview tier is not downsampling.
- [x] Add visual fixtures for opacity, normal maps, UV transforms, multiple UV
  sets, mirrored transforms and supported material conversions.

## 6. Geometry efficiency and fidelity

- [x] Emit indexed GLB geometry. Deduplicate compatible position/normal/UV/colour/tangent combinations
  while preserving seams, hard edges and material boundaries.
- [x] Use appropriate index widths and optimize vertex/index ordering after
  correctness is established. Measure output size, export time and viewer cost.
- [x] Add smooth-normal generation with explicit hard-edge/crease policy and
  consistent tangent generation validated against baked normal-map fixtures.
- [x] Add transform baking with correct normal, winding, mirror and shared-mesh
  handling. Provide a deliberate conversion path for shear that GLB export rejects.
- [x] Add explicit geometry repair tools for degenerate faces, invalid topology
  and welding, with tolerances, provenance and before/after reports.
- [x] Define handling for nonplanar polygons and numerical precision at large
  coordinate magnitudes; distinguish repair from faithful conversion.
- [x] Preserve and reuse shared meshes across compatible placements and bindings.
- [x] Add simplification and geometry LOD generation with material/UV boundaries
  and geometric error limits. Keep geometry LODs separate from texture tiers.
- [x] Evaluate quantization and mesh/texture compression only after indexed export
  and profiling. Include decode time, WASM size and browser support in the decision.

## 7. Model editing

- [x] Add rename, reparent, add/remove node, subtree duplication and model merge.
  Specify world-placement preservation, identity collisions and binding inheritance.
- [x] Add pivot adjustment, axis conversion and transform baking operations.
- [x] Add metadata updates and queries that work on the resulting scene generation.
- [x] Make operations atomic, preserve provenance and report affected identities,
  bounds, bindings, dependencies and losses. Keep failure from returning partial edits.
- [x] Avoid cloning all geometry and texture payloads for a small metadata or
  hierarchy change; introduce shared immutable storage or transactional changes.
- [x] Extend diffs to metadata and structural edits without returning mesh buffers.

## 8. Performance: measure and design for speed

- [x] Establish release-build baselines for import, preflight, metadata access,
  query/index build, repeated queries, edits, triangulation and export. Measure
  stages separately from file/network transfer and browser initialization.
- [x] Record wall time, throughput, p50/p95 latency, allocations, peak native RSS,
  WASM linear-memory high-water mark, JS buffer copies and output size. Measure
  both cold and warm runs with fixed tool versions, inputs and device descriptions.
- [x] Cover dense meshes, many small meshes, deep/wide hierarchies, repeated
  instances and texture-heavy models within configured limits. Test deliberate
  over-limit inputs separately. The existing synthetic debug grid is only a baseline.
- [x] Profile repeated parsing and full-scene passes. Reuse parsed data between
  dependency discovery, preflight, import and inspection where practical.
- [x] Replace repeated global scans with adjacency/path/material indexes. Target
  approximately linear scans and bounded per-query work; measure adversarial shapes.
- [x] Cache derived data by content/revision and options. Invalidate correctly on
  changes to units, hierarchy, geometry or bindings; keep cache growth bounded.
- [x] Reduce allocation churn with borrowed views, shared byte storage, reusable
  buffers and pre-sizing. Avoid image/mesh copies across format and ABI boundaries.
- [x] Avoid repeating expensive conversion for C's two-pass buffer sizing. Explore
  explicit result handles or prepared outputs while preserving ownership and ABI safety.
- [x] Separate encoded input, decoded geometry, decoded images, indexes, scratch
  buffers and output budgets. Reject excessive work before large allocations where
  possible; content-size checks are not guarantees of peak memory consumption.
- [x] Stream or process in chunks where supported, especially dependency hashing,
  image tiers and output writing. Document where complete buffers remain necessary.
- [x] Keep portable scalar implementations. Gate SIMD or native parallel execution
  behind measured optional paths and retain deterministic output or documented tolerances.
- [x] Measure browser transfer/copy costs and time to first metadata as well as
  total conversion time. A fast Rust loop can still produce a slow web experience.
- [x] Add repeatable performance reports and a controlled regression job. Establish
  numerical latency/memory/package-size budgets from representative baselines before
  claiming limits; investigate sustained regressions, not single noisy timings.

Initial performance acceptance criteria:

- [x] Repeated ID/path lookup uses an index and does not rescan the scene.
- [x] Metadata-only responses exclude geometry and image buffers by default.
- [x] Small metadata edits do not duplicate geometry or texture storage.
- [x] Repeated queries and open/query/close cycles have bounded retained memory.
- [x] Long-running work exposes progress and can be cancelled without blocking the UI.
- [x] Indexed export reduces the representative smooth-mesh preview size; report
  the measured improvement and any CPU tradeoff instead of assuming it.
- [ ] Publish measured p95 query latency, time to first metadata, peak memory and
  conversion throughput for at least a desktop browser and a constrained device.

## 9. WASM, web and native portability

- [x] Keep the portable core and format spokes independent of filesystem access,
  network access, a particular async runtime, browser globals and native USD binaries.
  Dependency reads are supplied by the host with explicit resolution/error rules.
- [x] Ship a typed ESM/TypeScript package with documented initialization, ownership,
  error types, feature discovery and schema/version compatibility.
- [x] Provide a Web Worker wrapper for import, preflight, query, edits and export.
  Support transferable buffers and request IDs; avoid JSON serialization of bulk
  binary data and explain when transferring a buffer gives up caller ownership.
- [x] Implement cooperative progress/cancellation with scheduling/yield points.
  A cancellation message alone cannot interrupt a synchronous WASM call. Define
  worker termination as a fallback and dispose of incomplete results safely.
- [x] Define handle disposal and memory-view lifetime rules. Avoid exposing views
  that silently become stale after WASM memory growth, edits or handle destruction.
- [x] Distinguish reusable WASM allocations from memory returned to the browser;
  test worker teardown and repeated model replacement, including failure paths.
- [x] Make the standard web build single-threaded and usable without cross-origin
  isolation. Offer threaded/SIMD builds only as optional, capability-detected paths.
- [x] Feature-gate large codecs and optional integrations to control download size
  and startup time. Publish raw/compressed WASM size and initialization benchmarks.
- [x] Test actual browser execution in Chromium, Firefox and WebKit, including
  worker loading, cancellation, metadata queries and a constrained/mobile environment.
  Native tests or Node-based WASM parity alone do not cover browser behaviour.
- [x] Verify direct ESM loading and a representative bundler integration, including
  WASM/worker URL resolution, MIME requirements and deployment instructions.
- [x] Extend native/WASM parity to query results, metadata, diagnostics, edits and
  exports. Use byte equality where promised and explicit tolerances where needed.
- [x] Exercise Linux GNU/musl, Windows MSVC and macOS Intel/ARM native builds and
  C consumers. Check numeric widths, Unicode paths, linking and ABI ownership rules.
- [x] Turn existing CI artifacts into versioned releases/npm packages with
  TypeScript declarations, headers, checksums, licence notices and executable examples.

## 10. Robustness and release evidence

- [x] Add fuzzing for USDA, USDC, USDZ, MaterialX, glTF, images, metadata and query
  requests; keep minimized regression fixtures for discovered failures.
- [x] Test malformed sizes/indices, deep nesting, cyclic dependencies, excessive
  metadata and cancellation during each expensive stage.
- [x] Validate representative exports with OpenUSD, full MaterialX-enabled shader
  registries and Khronos validators. Add visual checks alongside structural checks.
- [x] Version query, capability, metadata and diagnostic contracts across all APIs.
  Document supported profiles and migrations with each release.
- [x] Require correctness, browser usability and performance evidence together
  before raising advertised model-size limits or broadening compatibility claims.

## Later, after the static asset workflow is mature

- [x] Evaluate CAD/BIM adapters, RVT/RFA integration and domain-specific metadata
  at explicit adapter boundaries; retain a general-purpose neutral core.
- [x] Scope animation, skeletons, skinning and additional geometry schemas as
  separate projects driven by actual consumers.
- [x] Keep rendering, shader compilation and AI generation in consumer systems;
  the toolbox supplies portable data, transformations, queries and diagnostics.
