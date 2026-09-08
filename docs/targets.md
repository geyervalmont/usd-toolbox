# Application material targets

## Autodesk Revit

```bash
usd-toolbox export --input material.usdz --target revit --tier 2k \
  --output material-revit.zip --report material-revit-report.json
```

The ZIP contains `revit-material.json` using schema
`usd-toolbox.revit-material.v1` and one directory per material. It matches the
current OPAL pyRevit connector:

| Archive slot | Revit Generic property | Source |
|---|---|---|
| `base_color` | `generic_diffuse` | neutral base colour |
| `bump` | `generic_bump_map` | normal, then bump, then height |
| `glossiness` | `generic_glossiness` | inverted neutral roughness |

Each property connects a `UnifiedBitmapSchema` asset. The manifest also carries
real-world X/Y scale in millimetres and the Description, Model, Manufacturer,
and Keywords identity parameters used by the connector. OPAL can unpack the
archive into an immutable `revit` representation and retain the export loss
report beside it.

A constant neutral base colour is converted from linear RGB to a tiny sRGB PNG
because the current connector contract is file based. Non-straight repeat modes
and texture modulation are called out in the loss report rather than silently
claimed as exact.

The toolbox does not create or edit `.rvt` documents. The connector performs
those Revit API operations on Revit's UI thread and gives each material its own
Generic appearance asset before applying paths.

## NVIDIA Omniverse

```bash
usd-toolbox export --input material.usdz --target omniverse --tier 4k \
  --output material-omniverse.usdz --report material-omniverse-report.json
```

The first Omniverse contract is deliberately native OpenUSD: a deterministic,
self-contained USDZ with an OpenPBR shader graph, texture tiers, variants,
physical tiling, provenance, and auxiliary content. The target has its own crate
and options so a future connector can add Omniverse-specific MaterialX or MDL
render contexts without changing the neutral model or callers.

As with Revit, the eventual connector should consume the manifest/target and
perform application-side binding. Database access, Nucleus credentials, and
connector scheduling do not belong in this library.
