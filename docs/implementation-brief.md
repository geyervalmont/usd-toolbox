# USD Toolbox — materials implementation brief

A standalone Rust library for reading, writing and converting material definitions
between USD/USDZ, MaterialX and glTF. Built as a general-purpose interchange
library that happens to be delivered for materials first; geometry follows later
and must not require restructuring.

Consumed by OPAL (the Olsyn Parametric Asset Library) from both the browser and
the server, so it compiles to WebAssembly and exposes a C ABI.

---

## 1. What this is for

OPAL stores every material as a USDZ package: a MaterialX shading graph, the
textures it references, and provenance metadata, in one self-contained file. This
library is what writes those packages, reads them back, and converts them to
whatever a consumer needs.

Three properties matter more than feature count:

1. **Conversions are lossy and must say so.** OpenPBR carries roughly forty
   parameters; glTF core has eight plus extensions. Every exporter declares what
   it can represent and reports what it discarded.
2. **Provenance survives every operation.** Lineage is the only thing in the
   system that cannot be recomputed. An operation that loses it is a bug.
3. **The browser and the server run the same code.** The viewer is being rebuilt
   on this library. A conversion must not behave differently in WASM.

---

## 2. Scope

### In scope

- A neutral in-memory material model, OpenPBR-aligned
- USD read and write, including `.usda`, `.usdc` and `.usdz`
- MaterialX read and write (document authoring and parsing)
- glTF 2.0 / GLB material export
- Texture-set import: a directory or list of images with roles, into the model
- Capability declaration and loss reporting per exporter
- Provenance read/write via USD `assetInfo` and `customLayerData`
- USDZ packaging: layout, alignment, resolution tiers
- WASM bindings and a C ABI

### Out of scope

- **Geometry, meshes, skinning, animation, scene graphs.** Designed for, not built.
- **Shader code generation** (GLSL, MDL, OSL). The MaterialX C++ library does this
  and reimplementing it is a project of its own. Emit MaterialX; let the reference
  implementation compile it.
- **RFA parsing.** A Revit family is reached through the Revit API, in OPAL's
  existing extension. Not this library's problem, even though the format is
  understood.
- **Image processing beyond resampling.** Downscaling for tiers is in scope.
  Denoising, upscaling and map synthesis belong to the upgrade pipeline.
- **Rendering.** No preview rasterisation.

### Designed for, not built now

Geometry arrives later. The model must therefore keep materials addressable
independently of what they are bound to: a material is a first-class object with
an identity, not a property of a mesh. Do not thread a mesh or a stage handle
through the material APIs, and do not assume a material has exactly one binding.
When geometry lands it should add a `Geometry` spoke and a binding table, not
reshape `Material`.

---

## 3. Architecture

### Hub and spoke

One neutral model at the centre. Importers produce it, exporters consume it.
Never convert format-to-format directly — that is how an N-format library grows
N² converters and inconsistent behaviour.

```
   texture set ─┐                        ┌─→ glTF / GLB
   MaterialX  ──┼─→  neutral model  ─────┼─→ MaterialX
   USD / USDZ ──┘                        └─→ USD / USDZ
```

Each spoke is a module implementing one of two traits, and nothing else:

```rust
pub trait Importer {
    type Options: Default;
    fn import(&self, input: Input<'_>, options: &Self::Options)
        -> Result<Vec<Material>, ImportError>;
}

pub trait Exporter {
    type Options: Default;
    /// What this target can represent. Used to compute losses before writing.
    fn capabilities(&self) -> Capabilities;
    fn export(&self, materials: &[Material], options: &Self::Options)
        -> Result<Export, ExportError>;
}

pub struct Export {
    pub bytes: Vec<u8>,
    pub losses: Vec<Loss>,
}
```

### Crate layout

A workspace. The core carries no I/O opinions and no async, so it drops into WASM
without ceremony.

```
usd-toolbox/
├── crates/
│   ├── core/       # neutral model, capabilities, loss reporting. No I/O.
│   ├── usd/        # USD + USDZ import/export      (depends: openusd)
│   ├── materialx/  # MaterialX document read/write (depends: quick-xml)
│   ├── gltf/       # glTF 2.0 / GLB export         (depends: serde_json)
│   ├── textures/   # texture-set import, resampling, tier generation
│   ├── wasm/       # wasm-bindgen surface
│   └── ffi/        # C ABI, cbindgen header
└── fixtures/       # golden files, round-trip corpora
```

**Dependencies to use rather than replace:**

- [`openusd`](https://crates.io/crates/openusd) — native Rust USD, no C++
  dependencies, reads and writes `.usda` / `.usdc` / `.usdz`. Currently 0.7.0:
  pre-1.0, aiming at parity rather than claiming it. Depend on it, do not fork it;
  upstream fixes instead. Its writer paths are the ones this library exercises
  hardest, and writing is where its coverage is most easily verified.
- `image` for decode/encode, `fast_image_resize` for tier generation.
- `quick-xml` for MaterialX. Do not hand-roll XML.
- `sha2` for content addressing.

Keep `core` free of these. A dependency that cannot build for `wasm32-unknown-unknown`
does not belong in `core`.

---

## 4. The neutral model

Aligned to OpenPBR, because it is the richest of the models in play and the one
MaterialX ships. Where OpenPBR defines a parameter name, use it.

### Surface parameters

Every parameter is either a constant or a texture reference, so the same field
serves a Dulux paint (constant) and a scanned fabric (texture):

```rust
pub enum Value<T> {
    Constant(T),
    Texture(TextureRef),
    /// Texture modulated by a constant, the common case for tinted maps.
    Modulated { texture: TextureRef, factor: T },
}

pub struct Material {
    pub id: MaterialId,
    pub name: String,
    pub model: ShadingModel,          // OpenPbr | StandardSurface | GltfPbr
    pub surface: Surface,
    pub geometry: SurfaceGeometry,    // normal, height, opacity — not meshes
    pub tiling: Option<Tiling>,
    pub auxiliary: Vec<AuxiliaryAsset>,
    pub provenance: Provenance,
    pub variants: Vec<VariantSet>,
}
```

`Surface` carries, at minimum, the parameters OPAL already drives. These map
directly onto both OpenPBR and the glTF KHR extensions, which is why this set and
not a longer one:

| Neutral | OpenPBR | glTF |
|---|---|---|
| `base_color` | `base_color` | `pbrMetallicRoughness.baseColorFactor/Texture` |
| `base_metalness` | `base_metalness` | `metallicFactor/Texture` |
| `specular_roughness` | `specular_roughness` | `roughnessFactor/Texture` |
| `specular_ior` | `specular_ior` | `KHR_materials_ior` |
| `specular_anisotropy` | `specular_anisotropy` | `KHR_materials_anisotropy` |
| `transmission_weight` | `transmission_weight` | `KHR_materials_transmission` |
| `transmission_thickness` | `transmission_depth` | `KHR_materials_volume` |
| `coat_weight`, `coat_roughness` | `coat_*` | `KHR_materials_clearcoat` |
| `fuzz_weight`, `fuzz_roughness` | `fuzz_*` | `KHR_materials_sheen` |
| `subsurface_weight` | `subsurface_weight` | — *(no glTF equivalent)* |
| `emission_color` | `emission_color` | `emissiveFactor/Texture` |
| `geometry_normal` | `geometry_normal` | `normalTexture` |
| `geometry_opacity` | `geometry_opacity` | `alphaMode` + baseColor alpha |
| `ambient_occlusion` | — *(not OpenPBR)* | `occlusionTexture` |

Two entries in that table are the point of the loss report: `subsurface_weight`
has no glTF representation, and `ambient_occlusion` is a rendering convenience
rather than a physical OpenPBR parameter. Neither should be silently dropped.

### Texture channels

The role vocabulary is fixed by OPAL and must round-trip exactly:

```
base_color  normal  roughness  glossiness  metallic  height  bump
ao  opacity  specular  transmission  emissive
```

`glossiness` is the inverse convention of `roughness`; convert on import and
record the conversion as a loss entry, because it is not bit-reversible.

Auxiliary assets are carried but never treated as shading inputs:

```
ref_image  render  thumbnail  mdl
```

### Texture references

```rust
pub struct TextureRef {
    pub role: MapRole,
    pub tiers: BTreeMap<Tier, TextureSource>,  // preview, 1k, 2k, 4k, 8k
    pub color_space: ColorSpace,               // Srgb | Raw | Linear
    pub channel: Channel,                      // Rgb | R | G | B | A
    pub normal_convention: Option<NormalConvention>, // OpenGl | DirectX
}
```

Two invariants an implementer will otherwise get wrong:

- **Colour space is not inferable from the role.** Base colour is sRGB; normal,
  roughness, metallic, AO and height are raw. Getting this wrong produces
  materials that look plausible and are subtly incorrect everywhere.
- **Normal convention must be carried, not assumed.** The corpus contains both
  `normal_gl` and `normal_dx`. Flipping green is lossless and must be recorded
  when applied.

### Tiling

```rust
pub struct Tiling {
    pub width_mm: Option<f64>,
    pub height_mm: Option<f64>,
    pub repeat: Repeat,          // Straight | Halfdrop | Brick | Random | None
    pub install_pattern: Option<String>,
}
```

Real-world scale in millimetres, not UV multipliers. Revit and Enscape need
physical size, and a UV scale cannot be converted back into one without knowing
the surface.

### Variants

USD variant sets, generalised. Two axes are expected, and the model must not
assume only these two:

```rust
pub struct VariantSet {
    pub name: String,            // "colourway", "lod", ...
    pub default: String,
    pub variants: Vec<Variant>,  // each overriding a subset of parameters
}
```

A Dulux fan deck is one material with a `colourway` set of several hundred
variants overriding `base_color` only. A scanned fabric is one material per
colourway with an `lod` set over tiers.

---

## 5. Capabilities and loss reporting

The contract that makes the library trustworthy. Every exporter declares what it
can represent; conversion computes the difference and reports it. No exporter
silently approximates.

```rust
pub struct Loss {
    pub material: MaterialId,
    pub parameter: &'static str,
    pub kind: LossKind,
    pub detail: String,
}

pub enum LossKind {
    /// Not representable; omitted entirely.
    Dropped,
    /// Represented by a different parameter with different behaviour.
    Approximated { as_parameter: &'static str },
    /// Representable but reduced — a texture flattened to its average, say.
    Reduced,
    /// Lossless but non-reversible, e.g. glossiness inverted to roughness.
    Converted,
}
```

Requirements:

- `export()` always returns losses alongside bytes; it is never an error to lose
  something, and never acceptable to lose it quietly.
- Losses are written into the output where the format allows it — for USD, into
  `assetInfo` on the affected property.
- A caller can ask for losses without writing: `dry_run(materials, target)`.
- Loss output is deterministic and ordered, so it can be diffed in CI.

---

## 6. USDZ packaging

USDZ is an uncompressed, aligned zip. The alignment is what allows consumers to
memory-map and range-read it; a package written with compression is not a valid
USDZ even though it opens as a zip.

Package layout for a single material:

```
<variant-code>.usdz
├── material.usda            # stage; MaterialX graph; variant sets
├── textures/
│   ├── <sha256>.<ext>       # content-addressed, deduplicated within the package
│   └── ...
└── provenance.json          # optional mirror of assetInfo, for tools that skip metadata
```

Rules:

- Textures are named by content hash. A map shared between tiers or variants
  appears once per package.
- Every tier of every channel is present. Tiers absent from the source are
  generated by resampling the largest available and recorded as a `Converted`
  provenance entry.
- The package is reproducible: the same inputs produce byte-identical output.
  No timestamps, no map iteration order, no absolute paths.
- `usdchecker` from the reference USD distribution must accept every package. Wire
  it into CI from the first package, not later — it is the only independent check
  that `openusd`'s writer produced something valid.

---

## 7. Provenance

Stamped into the package, because lineage cannot be recomputed from anything else.

Three levels, using USD's own metadata fields:

| Level | Field | Carries |
|---|---|---|
| Layer | `customLayerData` | Builder name and version, build time, source identifier |
| Prim | `assetInfo` | Material identity, supplier source, version |
| Property | `assetInfo` on `inputs:file` | Per-texture lineage |

Per-texture lineage records:

```rust
pub struct Derivation {
    pub parent: Option<Sha256>,      // the image this came from
    pub operation: Operation,        // Scanned | Downscale | Upscale | Generated | Copied | Converted
    pub tool: String,
    pub tool_version: String,
    pub model: Option<String>,       // for model-produced images
    pub parameters: serde_json::Value,
    pub created: OffsetDateTime,
}
```

The distinction that decides what gets stored:

- **Deterministic operations are recipes.** A downscale records parent, operation
  and parameters; the result can always be regenerated.
- **Non-deterministic operations are facts.** A model-generated map records what
  produced it, because rerunning the same tool at a different version produces
  different bytes.

`assetInfo` is advisory — USD never validates it. This library is therefore the
validator: importing a package must verify that every recorded parent hash
resolves, and report a broken chain rather than ignoring it.

---

## 8. WASM and FFI

The viewer is being rebuilt on this library, so WASM is a first-class target, not
an afterthought.

- `core`, `usd`, `materialx` and `gltf` must build for `wasm32-unknown-unknown`.
  CI builds that target on every commit.
- No filesystem or network access below the spokes. I/O is the caller's job:
  APIs take bytes and return bytes.
- No threads in the default build. Feature-gate any parallelism (`rayon`) off by
  default so WASM is not a special case that regresses.
- `wasm` crate: `wasm-bindgen`, returning typed objects rather than JSON strings
  where practical.
- `ffi` crate: C ABI, `cbindgen`-generated header, caller-allocated buffers, and
  an explicit free function for anything the library allocates. No panics across
  the boundary — catch and convert to error codes.

---

## 9. Testing

- **Round trip is the primary property.** For every importer/exporter pair that
  claims fidelity: import → export → import produces an equal model. Where it
  cannot, the difference must appear as a declared loss, and the test asserts the
  loss rather than the equality.
- **`usdchecker` gate.** Every written package is validated by the reference
  implementation in CI. A green build with a package the reference rejects is the
  failure mode this exists to prevent.
- **Golden files.** Small, readable `.usda` and `.mtlx` fixtures checked in, with
  byte-exact expected output. These catch writer regressions from `openusd`
  upgrades better than any assertion about the model.
- **Determinism.** Build the same package twice; assert byte equality.
- **Provenance survival.** A package with lineage, round-tripped, keeps every
  derivation record and every parent hash.
- **Fuzz the readers.** Untrusted USDZ is an attack surface: it is a zip that
  claims offsets. `cargo-fuzz` over the import paths.

---

## 10. Milestones

Ordered so each one is independently useful.

1. **Core model and capabilities.** The neutral model, `Value`, `TextureRef`,
   `Loss`, capability declaration. No spokes. Fully tested in isolation.
2. **Texture-set import and tier generation.** Takes OPAL's existing files, with
   roles and colour spaces, and produces materials with complete tiers. This alone
   unblocks OPAL's ingest.
3. **USDZ export.** Package writing, content-addressed textures, provenance
   stamping, `usdchecker` in CI, determinism tests.
4. **USDZ import.** Read back, verify lineage, round-trip tests.
5. **MaterialX read and write.** Authoring the graph inside the package, parsing
   it back.
6. **glTF/GLB export.** With the loss report for `subsurface_weight` and friends.
   This is what the rebuilt viewer consumes.
7. **WASM and FFI surfaces.** Both targets in CI.

Milestones 1–3 are what OPAL needs to start packaging its corpus. Everything after
is what makes the library general.

---

## 11. References

- OpenPBR specification — the shading model the neutral model follows
- MaterialX specification, 1.39 — document structure, standard node library
- OpenUSD glossary — `assetInfo`, `customLayerData`, variant sets
- USDZ specification — package layout, alignment and compression constraints
- glTF 2.0 specification and the `KHR_materials_*` extension registry
- [`openusd` crate](https://crates.io/crates/openusd) — the USD implementation to build on
