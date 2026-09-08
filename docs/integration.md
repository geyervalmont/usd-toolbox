# Integration

## Laravel worker through the CLI

Build the release executable and point `OPAL_TOOLBOX_BIN` at it:

```bash
cargo build --release -p usd-toolbox-cli
target/release/usd-toolbox build \
  --manifest /work/material/manifest.json \
  --output /work/material/package.usdz \
  --report /work/material/report.json
```

The worker stages source files beside the manifest. Every `path` is relative to
the manifest directory; absolute paths, `..`, and symlinks which escape that
directory are rejected. The CLI verifies the declared byte length, SHA-256, and
optional dimensions before decoding a texture. This keeps bulk image bytes out
of PHP and isolates decoder/native-library crashes to one subprocess.

The schema-1 request consumed by `usd-toolbox build` is:

```json
{
  "schema": 1,
  "variant": { "code": "CPT-TARKETT-ASHEN", "name": "Ashen" },
  "shading_model": "openpbr",
  "required_tiers": ["preview", "1k", "2k", "4k"],
  "tiling": {
    "width_mm": "500.00",
    "height_mm": "500.00",
    "repeat": "tile",
    "install_pattern": "Monolithic"
  },
  "channels": {
    "base_color": {
      "4k": {
        "sha256": "<64 lowercase hex characters>",
        "object_key": "files/source/base-color.png",
        "bytes": 123456,
        "colour_space": "srgb",
        "width_px": 4096,
        "height_px": 4096,
        "normal_convention": null,
        "path": "sources/<sha256>.png"
      }
    }
  },
  "provenance": {
    "material": "CPT-TARKETT",
    "variant": "CPT-TARKETT-ASHEN"
  },
  "ingested_at": "2026-09-08T03:00:00Z"
}
```

`colour_space` is mandatory for every shader texture (auxiliary files may use
`null`). `normal_convention` is also mandatory for normal maps and accepts
`opengl` or `directx`; the CLI does not infer it from `_gl`/`_dx`. DirectX normal
maps are losslessly flipped to OpenGL and that conversion is reported. Filename
convention inference exists only as an opt-in compatibility flag on the
lower-level Rust import options. The fixed `ref_image`, `render`, `thumbnail`,
and `mdl` channel roles are retained as auxiliary assets and never connected to
the shader.

Missing lower tiers are generated deterministically. Missing tiers larger than
the largest supplied image fail instead of being upscaled, because enhancement
belongs to the upgrade pipeline. `ingested_at` is written into provenance; the
Laravel caller intentionally excludes it from its build digest.

On success the report contains the fields the worker records, plus output
integrity information:

```json
{
  "schema": 1,
  "version": "0.1.0",
  "tiers": ["preview", "1k", "2k", "4k"],
  "losses": [],
  "output": { "sha256": "<sha256>", "bytes": 987654 }
}
```

Output and report files are written through same-directory temporary files, so
the worker never mistakes a partially written file for a successful build.

## Native embedding through PHP FFI

Build `usd-toolbox-ffi` as a release `cdylib` and load
`crates/ffi/include/usd_toolbox.h` with PHP FFI. Inputs and options are
UTF-8 JSON for neutral structs; encoded material documents remain raw bytes.

The primary functions use a two-pass buffer contract. This remains useful for
small in-memory calls, but the Laravel packaging pipeline uses the CLI for crash
isolation and to avoid copying texture sets into PHP memory:

1. Call with null output buffers and zero capacities.
2. Expect `USD_TOOLBOX_STATUS_BUFFER_TOO_SMALL`; read the required lengths.
3. Allocate PHP FFI byte arrays of exactly those lengths.
4. Call again and consume the initialized prefixes.
5. On any error, call `usd_toolbox_last_error` with the same two-pass pattern.

`usd_toolbox_export_alloc` is available when library-owned output is more convenient,
but each returned `UsdToolboxBuffer` must be released exactly once with
`usd_toolbox_buffer_free`. Caller-allocated functions are the safer default for a
long-running worker.

Target constants are declared in the header:

| Target | Value |
|---|---:|
| USDZ | 1 |
| USDA | 2 |
| USDC | 3 |
| MaterialX | 4 |
| glTF | 5 |
| GLB | 6 |
| Revit image-set ZIP | 7 |
| Omniverse USDZ | 8 |

The ABI exports texture-set import, USD import, MaterialX import, all exporters,
loss-only dry runs, inspection, audit, patch/diff, tier planning, texture
attachment/removal, embedding input/clustering, version lookup, and thread-local
last-error retrieval.

## Browser through WASM

Build the `usd-toolbox-wasm` crate with `wasm-pack`. The bindings expose:

- `import_texture_set(inputs, options)`
- `export_usd(materials, options)` / `import_usd(bytes, options)`
- `export_materialx(materials, options)` / `import_materialx(bytes, options)`
- `export_gltf(materials, options)`
- `export_revit(materials, options)` / `export_omniverse(materials, options)`
- `inspect_materials(materials)` / `audit_materials(materials, profile)`
- `apply_material_patch(material, patch)` / `diff_material(before, after)`
- `plan_material_tiers(material, policy)`
- `complete_material_tiers(material, policy, created)`
- `attach_texture(material, attachment)` / `remove_texture(material, role, tier)`
- `extract_texture(material, role, tier)`
- `migrate_material_document(bytes)`
- `prepare_material_embedding(material, options)`
- `cluster_material_embeddings(records, threshold, minClusterSize)`
- `dry_run_export(materials, target)`

Materials, options, and losses cross as structured JavaScript values through
`serde-wasm-bindgen`; documents and texture payloads cross as byte arrays.
Export results expose `bytes` and `losses` separately.

The browser should retain the neutral material object as its editing state and
only export at a delivery boundary. Converting an intermediate format back into
another format needlessly discards information and bypasses the loss contract.

## Versioning

The neutral manifests embedded in USD and MaterialX are schema-versioned. The C
ABI is intentionally a small fixed surface; additions should be backwards
compatible within a major release. Call `usd_toolbox_version` during worker startup and
log it beside conversion jobs for reproducibility.
