# Scene preview textures and portable decoder profile

Measured 2026-10-16. `SceneGltfOptions.texture_tier` now limits the actual longest
image edge: preview 512, 1k 1024, 2k 2048, 4k 4096, and 8k 8192 pixels. The default
remains 2k. Source selection still prefers the requested tier, then a larger tier,
then the largest smaller tier. Encoded dimensions determine whether resizing is
needed, regardless of the source's tier label or supplied dimension metadata.
Smaller images are never enlarged. Aspect ratio is retained with nearest-integer
rounding and a minimum one-pixel edge.

Derivatives exist only in the export's image cache. Import retains original image
bytes; exporting a preview does not mutate the scene, replace sources or clone the
whole scene. Material-only `GltfExportOptions` retains its existing tier-selection
behavior. `gltf_scene_capabilities()` exposes the preview profile and exclusions.

## Filtering and limits

- sRGB RGB samples are converted to linear light before pixel-area averaging and
  encoded back to sRGB. Raw/linear data channels are averaged independently.
- Normal maps are averaged as signed vectors, then normalized. A cancelling vector
  falls back to +Z. DirectX green-channel inversion still occurs after filtering.
- Alpha is linear. Base colour uses alpha weighting for MASK/BLEND materials;
  opaque materials ignore alpha for colour filtering. Separate opacity maps retain
  the existing channel-packing behavior. Emissive alpha is independent.
- The cache distinguishes colour, raw data, normal vectors and alpha treatment,
  including when one source image is reused in several material slots. Identical
  resulting image bytes can still share an exported image.
- Generated images are deterministic RGBA8 PNG. PNG16 is decoded at its original
  precision before filtering, then quantized to 8-bit output. Diagnostics report
  dimensions, filter semantics and encoding for every reduced material parameter.
- Unique decoded-source bytes, per-image float/filter buffers, retained encoded
  derivatives and final output each obey configured scene byte limits. PNG16
  decoder bytes are counted rather than assuming all images are four bytes per
  pixel. Actual codec headers are inspected before allocating pixel buffers.
  Decoder scratch uses `image::Limits` on a best-effort basis. These checks are
  resource limits, not a promise that total peak RSS equals `max_bytes`.

This is a preview projection. It does not generate mip chains, preserve alpha-test
coverage across resolutions, filter roughness from normal-map variance, manage
arbitrary ICC/OCIO primaries, or perform HDR tone mapping. Author glTF colour maps
as sRGB and data maps as raw; arbitrary source colour spaces need an explicit
conversion policy. Large images can exceed browser working-memory limits even
when their eventual 512px derivative would be small.

## Shipping support

| Input / operation | Native | wasm32-unknown-unknown |
|---|---|---|
| PNG 8/16-bit, JPEG; caller-supplied embedded/bundle bytes | Supported | Supported |
| Real preview minification, sRGB/data/normal/alpha semantics above | Supported | Supported; executed in Node |
| EXR, HDR, TIFF, KTX2/Basis | Explicitly unsupported | Explicitly unsupported |
| UDIM/UVTILE discovery, tile assembly and cross-tile filtering | Unsupported | Unsupported |
| Arbitrary EXR channels, multilayer/deep images, float-preserving glTF image output | Unsupported | Unsupported |

The pinned five-asset corpus contains fourteen embedded PNG images and no JPEG,
EXR or UDIM inputs. Three assets are inside the static import profile; SheenChair
and RecursiveSkeletons retain their existing unsupported-feature behavior.
Synthetic tests cover JPEG and PNG16. This corpus does not establish a requirement
for EXR or tiled data maps; no decoder was added to the shipping dependency graph.
Individual files named with tile numbers are ordinary images, not an assembled
UDIM set. An application must retain and resolve the original set explicitly.

## Native/WASM EXR assessment, outside the shipping build

`assessments/exr` is an isolated Cargo workspace with a lockfile, using
`image 0.25.10` with its optional `exr` feature and `exr 1.74.2`. Its probe compiled
and executed on native Linux and bare wasm32 in Node with no WASI/browser imports.
An 8×4 RGB32F image retained negative values, values above one, and fractional
linear samples exactly through EXR encode/decode. Both produced 689 encoded bytes;
the decoder reported 384 pixel bytes and rejected an eight-byte allocation budget.
The complete standalone decoder/encoder probe WASM was 926,514 bytes. That number
is not the incremental cost in the product bundle.

Inspection of the pinned `image/src/codecs/openexr.rs` adapter shows first-valid
non-deep RGB(A) layer selection, largest-resolution selection, display-window
cropping, and dropped metadata. It allocates a second float pixel buffer internally
while decoding. Its RGB(A) projection does not establish support for a scalar
roughness channel, all original layers, deep samples, chromaticities or strict
whole-decoder memory limits. Tiled EXR and UDIM sets were not exercised. These
gaps require representative customer assets, a channel/window/colour policy,
hostile-file budget tests and browser measurements before product support.

Reproduce the optional assessment without enabling EXR in the main workspace:

```sh
CARGO_TARGET_DIR=target/exr-assessment cargo run --locked --release \
  --manifest-path crates/gltf/assessments/exr/Cargo.toml
CARGO_TARGET_DIR=target/exr-assessment cargo build --locked --release \
  --manifest-path crates/gltf/assessments/exr/Cargo.toml --target wasm32-unknown-unknown
node -e 'WebAssembly.instantiate(require("fs").readFileSync("target/exr-assessment/wasm32-unknown-unknown/release/exr-assessment.wasm"),{}).then(({instance})=>{if(instance.exports.exr_check()!==689)throw Error("EXR probe failed")})'
```

## Corpus and visual evidence

Single local release-build runs; timings are evidence, not latency guarantees.
All source textures below were 2048×2048; every output image matched the requested
512 or 1024 pixel tier. Source hashes and repeated export bytes were checked.

| Asset | Tier | Source GLB bytes | Preview GLB bytes | Export ms |
|---|---|---:|---:|---:|
| BoomBox | 512 | 10,614,184 | 954,648 | 213.03 |
| BoomBox | 1024 | 10,614,184 | 3,740,996 | 293.93 |
| NormalTangentMirrorTest | 512 | 1,567,084 | 374,740 | 119.03 |
| NormalTangentMirrorTest | 1024 | 1,567,084 | 877,216 | 181.56 |

Box remains a 3,520-byte geometry-only export. All six corpus derivatives passed
Khronos glTF validation with zero errors and warnings. Reimporting the normal-map
fixture exposed an existing duplicate material identity in double-sided variants;
these variants now receive deterministic, collision-checked derived identities.

The existing Chromium/Three comparison passed its original thresholds at three
camera angles for 512px derivatives: Box was pixel-identical; BoomBox's maximum
mean absolute RGB difference was 0.216/255 and NormalTangentMirrorTest's was
0.00843/255. BoomBox had at most 0.970% of channels differ by more than three.
These are whole-frame measurements at 384×384, not proof of fidelity at arbitrary
zoom, viewing angle or material lighting.

```sh
cargo test -p usd-toolbox-gltf
cargo check -p usd-toolbox-gltf --target wasm32-unknown-unknown
cargo test -p usd-toolbox-gltf --release --test preview_corpus -- --ignored --nocapture
CORPUS_EXPORT_DIR=target/corpus-preview CORPUS_EXPORT_SUFFIX=-preview \
  node crates/gltf/tests/render_corpus.cjs
cargo run -p usd-toolbox-gltf --release --example preview_portability
cargo build -p usd-toolbox-gltf --release --target wasm32-unknown-unknown --example preview_portability
node -e 'WebAssembly.instantiate(require("fs").readFileSync("target/wasm32-unknown-unknown/release/examples/preview_portability.wasm"),{}).then(({instance})=>console.log(String(instance.exports.preview_checksum())))'
```

The renderer accepts the existing `PLAYWRIGHT_MODULE`, `THREE_ROOT` and
`CHROMIUM_EXECUTABLE` environment variables. Corpus reports and validator/browser
artifacts are under `target/corpus-preview`. The native and bare-WASM preview probe
both returned `8541793330734408470`, checking the complete export bytes for a
non-power-of-two source shared between sRGB colour, scalar data and normal maps.

Default vertex/index ordering is unchanged. An explicitly enabled
`optimize_vertex_cache` option now provides measured opaque-primitive ordering
with cache-benefit checks and transparent-material exclusions. See
[vertex-ordering.md](vertex-ordering.md) for its fidelity evidence and limits.
