# usd-toolbox

[![CI](https://github.com/geyervalmont/usd-toolbox/actions/workflows/ci.yml/badge.svg)](https://github.com/geyervalmont/usd-toolbox/actions/workflows/ci.yml)
[![Build artifacts](https://github.com/geyervalmont/usd-toolbox/actions/workflows/artifacts.yml/badge.svg)](https://github.com/geyervalmont/usd-toolbox/actions/workflows/artifacts.yml)

A standalone Rust interchange toolkit for portable 3D assets. USD/USDZ is the
primary container, with MaterialX and glTF/GLB spokes. Materials are the first
implemented domain; the workspace, native boundary, and browser boundary are
named for the broader geometry-and-material pipeline they will grow into.

The intended geometry path follows the same hub-and-spoke rule as materials:
USD scene data enters a neutral scene model, then the browser or exporter emits
Three.js-friendly data, glTF, or GLB without adding direct format-to-format
converters.

This repository is independent of OPAL. OPAL is its first consumer, not an
architectural dependency.

## What works

- OpenPBR-aligned materials with stable identity, physical tiling, arbitrary
  variants, auxiliary assets, complete provenance, and constant/textured/
  modulated values.
- Explicit, deterministic capability and loss reports before or during export.
- Texture-set import from PNG/JPEG bytes, explicit colour space and normal-map
  convention, gloss-to-roughness conversion, and deterministic preview–8K tiers.
- Deterministic USDA, USDC, and 64-byte-aligned uncompressed USDZ export; guarded
  import with archive limits, hash verification, and lineage validation.
- Material inspection, versioned JSON migration, validated merge-patch editing,
  semantic diffs, extraction/removal, and AI/manual texture attachment.
- Per-material and corpus health audits with actionable issue codes, tier and
  metadata policy, actual image checks, and duplicate candidates.
- Provider-agnostic tier jobs, safe automatic downscales, stable multimodal
  embedding inputs, cosine similarity, and deterministic clustering.
- MaterialX 1.39 read/write and glTF 2.0/GLB material export with ratified
  `KHR_materials_*` extensions and channel packing.
- Revit Generic appearance-asset image-set ZIP export matching the OPAL
  connector's diffuse/bump/glossiness contract.
- An independently versioned Omniverse target producing self-contained USDZ.
- A `usd-toolbox build` command which reads staged texture paths, emits USDZ, and
  writes a machine-readable tier/loss report without moving image bytes through PHP.
- CLI, `wasm-bindgen`, and panic-safe C ABI surfaces for the same workflows.

USD and MaterialX importers guarantee full fidelity for documents written by
this library, using an embedded versioned neutral manifest. They also support a
conservative inspection of direct constants in third-party USD Preview Surface,
USD OpenPBR, MaterialX OpenPBR, and MaterialX Standard Surface graphs. Unknown
connections are reported as unresolved metadata instead of being guessed.
Geometry and material bindings remain deliberately outside the model for now.

## Workspace

| Crate | Responsibility |
|---|---|
| `usd-toolbox-cli` | Isolated filesystem/process boundary for workers and scripts |
| `usd-toolbox-core` | Neutral model, validation, capabilities, loss contract |
| `usd-toolbox-materials` | Inspect/edit/audit/tier/embedding workflows |
| `usd-toolbox-textures` | Image ingest, map conventions, tier generation |
| `usd-toolbox-usd` | USDA/USDC/USDZ import and export |
| `usd-toolbox-materialx` | MaterialX graph import and export |
| `usd-toolbox-gltf` | glTF/GLB export and texture packing |
| `usd-toolbox-revit` | Revit Generic appearance image-set export |
| `usd-toolbox-omniverse` | Omniverse-compatible OpenUSD target adapter |
| `usd-toolbox-wasm` | Browser API |
| `usd-toolbox-ffi` | Native C ABI and generated header |

The format crates only communicate through `usd-toolbox-core`; there are
no format-to-format converters.

## Rust example

```rust
use usd_toolbox_core::{Exporter, Material};
use usd_toolbox_usd::{UsdExportOptions, UsdExporter};

let mut material = Material::new("paint-red", "Paint Red");
material.surface.base_color = [0.62, 0.03, 0.02, 1.0].into();
material.surface.specular_roughness = 0.36.into();

let exported = UsdExporter.export(&[material], &UsdExportOptions::default())?;
std::fs::write("paint-red.usdz", exported.bytes)?;
assert!(exported.losses.is_empty());
# Ok::<(), Box<dyn std::error::Error>>(())
```

The reusable spokes remain bytes-in/bytes-out. The CLI is the one deliberate
filesystem adapter: it resolves staged paths and then calls those same APIs.

## Build and verify

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
rustup target add wasm32-unknown-unknown
cargo check -p usd-toolbox-wasm --target wasm32-unknown-unknown
```

To build the backend worker executable:

```bash
cargo build --release -p usd-toolbox-cli
target/release/usd-toolbox --version
target/release/usd-toolbox --help
```

Point `OPAL_TOOLBOX_BIN` at `target/release/usd-toolbox`. To build the optional
native embedding library and regenerate its header:

```bash
cargo build --release -p usd-toolbox-ffi
```

Artifacts are `target/release/libusd_toolbox_ffi.so` (or the platform
equivalent) and `crates/ffi/include/usd_toolbox.h`.

To generate browser glue with `wasm-pack` installed:

```bash
wasm-pack build crates/wasm --target web --release
```

Every push to `main` publishes 30-day GitHub Actions artifacts for Linux,
macOS, Windows, and browser WASM. Native bundles contain the CLI, dynamic and
static C ABI libraries, generated C header, and README. The browser bundle is a
`wasm-pack --target web` package ready for JavaScript bundlers or direct ES
module loading. Download them from the latest
[Build artifacts run](https://github.com/geyervalmont/usd-toolbox/actions/workflows/artifacts.yml).

See [material-workflows.md](docs/material-workflows.md) for inspection, audit,
editing, tier generation, and embeddings; [targets.md](docs/targets.md) for
Revit and Omniverse; [integration.md](docs/integration.md) for CLI/FFI/WASM;
and [architecture.md](docs/architecture.md) for package invariants.

## Validation status

CI checks the native workspace, the real WASM target, generated fixture packages,
the OpenUSD and MaterialX reference runtimes, and Khronos' glTF Validator.
If a full OpenUSD installation provides `usdchecker`, run:

```bash
scripts/check-usdz.sh path/to/material.usdz
```

The minimal `usd-core` Python wheel used by CI omits the optional MaterialX SDR
plug-in. Its validation fallback therefore reports—but does not fail on—the one
specific missing-registry finding for the standard OpenPBR node. A full
MaterialX-enabled `usdchecker` remains the release gate.

## Current boundaries

- Image decoding and tier generation currently support PNG and JPEG.
- glTF is export-only.
- Generic external graphs are currently a conservative inspection path: direct
  standard constants are imported, while unresolved texture connections remain
  explicit metadata until bytes and colour interpretation can be proven.
- Shader code generation and rendering are intentionally out of scope.
- Geometry will arrive as another spoke plus a binding table; `Material` will
  remain independently addressable.

The delivery brief is retained in [implementation-brief.md](docs/implementation-brief.md).
