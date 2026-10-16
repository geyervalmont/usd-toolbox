# Geometry and texture compression assessment

Measured 2026-10-16 on the local Linux development machine, after indexed export.
Keep the canonical neutral/USD representation and ordinary GLB export unchanged.
Consider optional lossy preview encodings only after a caller specifies physical
error and target-browser requirements.

## Baseline and transport compression

The 65,536-quad synthetic grid now exports to a 3,159,340-byte indexed GLB in a
single 72.5 ms release run. Its previous unindexed vertex data alone occupied
9,437,184 bytes. The 66.52% reduction was achieved without quantizing attributes.
This isolates the useful first improvement from later codec decisions.

Pinned Khronos corpus exports were compressed with Node's gzip level 6 and Brotli
quality 5, then decompressed and checked byte-for-byte. Ten local iterations:

| Indexed GLB | Original bytes | gzip bytes / reduction | Brotli bytes / reduction | gzip decode p50 / p95 | Brotli decode p50 / p95 |
|---|---:|---:|---:|---:|---:|
| Box | 3,520 | 1,135 / 67.76% | 1,063 / 69.80% | 0.029 / 0.107 ms | 0.020 / 0.128 ms |
| BoomBox | 10,617,444 | 10,512,322 / 0.99% | 10,518,121 / 0.94% | 11.64 / 13.76 ms | 4.75 / 6.82 ms |
| NormalTangentMirrorTest | 1,571,360 | 1,460,566 / 7.05% | 1,444,273 / 8.09% | 3.81 / 4.21 ms | 5.38 / 5.53 ms |

Reproduce with `node crates/gltf/tests/benchmark_compression.cjs` after the corpus
roundtrip test. The generated report records Node version, CPU and timings.
Compression can be done once when preparing a derivative, rather than on each
request. Already compressed textures explain why compressing all of BoomBox has
little benefit. These are native Node measurements, not browser download or WASM
startup benchmarks. Normal HTTP content encoding requires no glTF extension, but
does not reduce decoded geometry or texture memory.

## Candidate decisions

| Candidate | Potential benefit | Cost and required evidence | Current decision |
|---|---|---|---|
| 16-bit positions / UVs, 8-bit normals / tangents | Smaller GPU vertex buffers and downloads | Physical displacement error, normal angular error, seam compatibility, quantized bounds, transform composition and browser visual comparisons | Optional preview profile, not canonical data; implement only with caller error bounds |
| Meshoptimizer buffer compression | Reduced geometry transfer while retaining an indexed mesh | Decoder bundle/startup and scratch memory, decode time on constrained devices, deterministic encoder version, required-extension versus fallback size | Evaluate a portable optional adapter; do not add a hidden native dependency |
| Draco mesh compression | Smaller mesh delivery for selected models | Extra decoder, decode latency, attribute/primitive reconstruction and correspondence to editable source objects | Compare against meshoptimizer on the same assets before selecting either |
| KTX2/Basis Universal textures | Texture transfer and GPU residency improvements | Transcoder startup/WASM size, target texture format selection, normal/data-map fidelity and alpha edge tests | Texture-specific optional delivery profile; keep original PNG/JPEG sources |
| HTTP gzip/Brotli | Portable transfer reduction with byte-identical decompression | Full decoded memory remains; small benefit on image-heavy files | Supported at the host/delivery layer; choose using measured content benefit |

The [Khronos quantization specification](https://github.com/KhronosGroup/glTF/blob/main/extensions/2.0/Khronos/KHR_mesh_quantization/README.md)
requires quantized attributes to be declared as a required extension and keeps
four-byte vertex alignment. For the grid, aligned 16-bit positions plus aligned
8-bit normals would reduce vertex storage from 24 to 12 bytes per unique vertex;
indices would remain unchanged. That is an estimated additional 792,588 bytes,
about 25.1% of this GLB, before metadata overhead. It is an estimate, not an encoded
artifact. With a uniform 256-unit position extent, a 16-bit quantizer has a
half-step bound of 0.001953 units per axis; this may already exceed a model's
physical error allowance. Small displacements across independently quantized
adjacent meshes can create visible cracks.

[Meshoptimizer's upstream design](https://github.com/zeux/meshoptimizer) and the
[buffer compression extension](https://github.com/KhronosGroup/glTF/blob/main/extensions/2.0/Vendor/EXT_meshopt_compression/README.md)
separate data preparation, compression and decoding. A comparison must measure
reordering/quantization separately from the codec, and include native/WASM
implementation and decoder licensing/size. A small compressed file alone is not
sufficient evidence of a faster web workflow.

The [KTX2/Basis Universal extension](https://github.com/KhronosGroup/glTF/blob/main/extensions/2.0/Khronos/KHR_texture_basisu/README.md)
introduces a different texture delivery path. The initial image-heavy corpus
suggests measuring real preview downsampling before adding a large codec. Preserve
raw data-map interpretation, verify normal-map angular error, and compare alpha
edges and texture seams in actual browsers.

No quantization, Draco, meshoptimizer or Basis codec is claimed as implemented by
this report. The current importer reports these unsupported required features
explicitly. Native and browser decoder size, constrained-device decode p95, peak
memory and normal-map visual checks remain gates for choosing an optional codec.
