# Roadmap

The material-library workflow is operational: inspect/edit/diff, corpus audits,
tier planning/attachment, embedding inputs/clustering, and Revit/Omniverse
targets are implemented across CLI, FFI, and WASM. Remaining expansion work is:

1. Expand conservative third-party graph inspection from direct constants to
   fully resolved USD/MaterialX texture networks and render contexts.
2. Integrate Autodesk/NVIDIA connector conformance fixtures as those plugins
   establish additional schemas; retain the existing target contracts.
3. Expand the readable golden corpus from the synthetic fixtures to licensed,
   representative OPAL materials.
4. Add `cargo-fuzz` harnesses for USDA, USDC, USDZ, MaterialX, and image inputs.
5. Broaden image ingest where the WASM-compatible decoders are mature (notably
   EXR for linear data maps).
6. Publish npm/native packages and version the CLI/material workflow contracts
   contract as the boundary types settle.
7. Add neutral scene/geometry types and an explicit material-binding table
   without changing the identity or surface shape of `Material`.
8. Add USD scene import and glTF/GLB scene export, then expose the same neutral
   scene data to Three.js through the WASM boundary.

Release acceptance should use a full MaterialX-enabled OpenUSD distribution so
the standard OpenPBR SDR nodes are present when `usdchecker` runs.
