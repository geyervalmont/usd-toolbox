# Native MaterialX

Version 0.2.0 treats a native MaterialX graph as material content. `Material.materialx`
contains an owned document tree, the selected `surfacematerial`, and checked,
content-addressed image dependencies. It survives JSON, USDA, USDC and USDZ round
trips without flattening procedural nodes into texture slots.

The existing neutral `surface`/`geometry` fields are a partial projection for
simpler targets. When a native graph is present, it is authoritative. Revit and
glTF conversion explicitly report that they use the partial projection. Editing
those fields through the material workflows requires first clearing `materialx`,
so changing a fallback cannot silently appear to change the authored graph.

## Import and export

A complete native document needs a top-level Standard Surface or OpenPBR shader
and a `surfacematerial` binding. Import it with `MaterialXImporter` and the default
options. Textured documents use `Input::Bundle`: one `.mtlx` file plus images named
relative to the document directory. The caller supplies all bytes; the library
never opens paths or fetches URLs. Procedural nodegraphs and interface ports are
retained, along with attributes and explicit node definitions.

The CLI performs that filesystem resolution, confines dependencies to the source
directory, and rejects escaped symlinks:

```sh
usd-toolbox export --input source/material.mtlx --target usdz --output material.usdz
usd-toolbox export --input material.usdz --target materialx --tier 4k --output export/material.mtlx
usd-toolbox export --input material.usdz --target materialx-preview --tier preview --output preview.json
```

The `.mtlx` exporter writes its content-addressed `textures/` dependencies beside
the output document. Existing matching image files are reused; conflicting files
are rejected. The XML retains the neutral manifest by default for identity and
provenance. For artist-authored XML without the manifest, Rust callers can set
`include_neutral_manifest: false`. If editing an exported XML graph externally,
remove the `olsyn_neutral_manifest` property set and reimport the graph with its
images: inconsistent graph/manifest pairs are rejected rather than ignoring edits.

`export_bundle(materials, tier)` returns XML, selected material names, base64 image
payloads and conversion losses. This is the JSON contract for the browser. WASM
exposes `import_materialx_bundle` and `export_materialx_preview` in addition to the
existing import/export functions. Native C callers can carry the same material
JSON and use the existing USD/MaterialX boundaries; external image bundles are
currently a Rust, CLI and WASM import feature.

## USD shading

The core graph author is shared by XML and USD. Neutral PBR materials now use
actual MaterialX image, channel extraction, modulation, normalmap, bump and
displacement nodes, with an OpenPBR surface. Scalar packed channels are explicit;
DirectX normal maps invert green before tangent-space decoding. Ambient occlusion
has no OpenPBR surface input and is reported as omitted from shading while its
original data remains in the neutral manifest.

USD authors `outputs:mtlx:surface` and, when present, `outputs:mtlx:displacement`.
Native graphs become `UsdShadeShader` and `UsdShadeNodeGraph` prims. USDZ contains
allowed USD/image entries, with the graph preserved in its neutral metadata; it
does **not** put an unsupported `.mtlx` entry into the archive. No MDL compilation
or renderer-specific shader generation happens in this library.

## Current boundaries

- Native inputs accept MaterialX 1.38/1.39 with PNG/JPEG/EXR dependencies. Includes,
  file prefixes and external source URIs must be resolved/flattened first.
- The USD adapter supports scalar, vector, colour and shader ports. Custom node
  definitions/implementations and unsupported value types fail explicitly.
  Third-party USD graph reconstruction remains the conservative import path.
- Native graph dependency bytes are preserved at their authored resolution;
  `tier` chooses existing neutral texture tiers, not a procedural bake.
- Browser rendering is a separate Three.js adapter. Displacement, volume and
  unsupported surface features are reported by that consumer. Preserving a graph
  does not claim identical rendering across engines.

Tests cover procedural and textured graph round trips, dependency failures, path
confinement, deterministic preview bundles and correct shader connections. CI
validates both simple and textured XML with MaterialX 1.39.5 and packages with
OpenUSD 26.8. When OpenUSD lacks the MaterialX SDR plug-in, only missing identifiers
verified against the official MaterialX node definitions are exempted; all other
OpenUSD findings still fail.
