# Architecture and invariants

## Hub and spoke

Every importer produces neutral types from `usd-toolbox-core`; every exporter
consumes those types. Format crates do not call each other. This keeps conversion
behaviour consistent and lets geometry add scene data without making materials
properties of meshes.

```text
texture set ─┐                         ┌─> glTF / GLB
MaterialX ───┼─> neutral materials ────┼─> MaterialX
USD / USDZ ──┘           │             ├─> USD / USDZ / Omniverse
                         │             └─> Revit image set
                         ├─> inspect / edit / audit
                         ├─> tier jobs <─ AI or human output
                         └─> embedding inputs ─> external model / vector store
```

That diagram is the implemented material slice, not the limit of the project.
The planned scene slice follows the same shape:

```text
USD / USDZ ──> neutral scene + bindings ──┬─> glTF / GLB
                                          └─> browser scene data
```

Scene/geometry types and a material-binding table will be added alongside the
current material types. They will not be folded into `Material`, and exporters
will continue to consume the neutral model rather than another format crate.

`Material` owns identity, surface parameters, surface-frame geometry inputs,
physical tiling, auxiliary assets, provenance, and variant sets. It owns no mesh,
stage, or binding handle.

## Fidelity contract

Each exporter exposes a capability matrix. `dry_run` computes an ordered list of
`Loss` records before any bytes are written, and `export` always returns the same
report beside its bytes. A loss can be dropped, approximated, reduced, or a
lossless but non-reversible conversion.

Provenance is not a best-effort extra. Validation rejects non-canonical hashes,
payload/hash mismatches, unresolved derivation parents, invalid normal-map
metadata, bad defaults, and duplicate variants.

## Texture invariants

- Colour space is supplied explicitly; it is never guessed from a role.
- Normal-map convention is carried explicitly. Green-channel flips are recorded.
- Roles use the fixed OPAL vocabulary.
- Tiers are `preview`, `1k`, `2k`, `4k`, and `8k` in stable order.
- Texture filenames are SHA-256-addressed and deduplicated in packages.
- Empty payload fields are omitted from packaged metadata. Provenance refers to
  retained sources by SHA-256 and reuses an existing texture or auxiliary entry
  when that exact content is already present.
- The default codec policy preserves JPEG/PNG for base colour. Normal, roughness,
  metallic, AO, height, and other data maps use lossless PNG. Callers of the
  lower-level texture API can override the policy per role.
- Tiling is a physical millimetre repeat, not a context-dependent UV multiplier.

## USDZ layout

```text
material.usdz
├── material.usda
├── textures/<sha256>.<ext>
├── auxiliary/<sha256>.<ext>
├── provenance/sources/<sha256>.<ext>  # only when content is not already packaged
└── provenance/provenance.usda
```

The root layer comes first. Every entry is stored without ZIP compression and its
payload begins on a 64-byte boundary. Entry order, timestamps, filenames, and
serialization are deterministic.

The full neutral manifest is embedded in root `customLayerData`; the separate
USDA provenance layer is only a mirror. JSON files are not put into USDZ because
JSON is not an allowed entry type in the USDZ specification.
Auxiliary payloads with allowed USDZ types are separate content-addressed files;
unsupported types such as MDL remain embedded in the neutral manifest so the
package stays compliant without losing them.

Import sets entry-count and total-size limits before returning data, rejects path
traversal and duplicate names, enforces the default-layer/file-type/layout rules,
recomputes content hashes, restores packaged bytes, and validates every lineage
link.

## Boundary rules

- Core has no I/O, async runtime, image, XML, or USD dependency.
- Spokes accept caller-owned bytes and return owned bytes.
- Default builds use no threads.
- WASM and native code execute the same conversion implementation.
- C-callable functions catch Rust panics and return status codes.
- Caller-allocated C ABI functions use a two-pass required-length contract.
- AI providers, database access, queues, and human approval state stay outside
  the toolbox. Generated/manual bytes return through one validated attachment.
- Embeddings and clusters are reproducible derived indexes, not canonical USDZ
  metadata. Their model/version/input digest travels with every vector.
