# Roadmap

The model toolbox now includes typed metadata, retained indexed queries, static
GLB/glTF ingestion, USD compatibility preflight, structural/geometry edits,
indexed previews, real texture tiers, worker lifecycle APIs and prepared C results.
See [model-toolbox.md](model-toolbox.md), [query-api.md](query-api.md) and
[todo.md](../todo.md) for implemented scope and remaining evidence gates.

Portable USD support remains a bounded static profile. The optional native
OpenUSD snapshot adapter handles explicitly selected composition/instances;
[usd-profiles.md](usd-profiles.md) describes the boundary. EXR/UDIM, arbitrary
shader evaluation and subdivision tessellation require additional corpus and
conversion policies before shipping support. Quantization/compression remains a
measured design assessment rather than an enabled codec.

Release evidence combines native correctness/allocation benchmarks, real browser
execution, reference validators, visual comparisons and bounded fuzzing. Customer
models/exporter versions, physical constrained-device measurements and successful
Windows/macOS execution remain separate requirements. Packages are built locally
and in CI with versions, headers, types, checksums and dependency notices. CI
artifacts follow the repository's visibility; the project's own distribution
licence is undeclared and the npm package remains marked private.

CAD/BIM and animation are scoped in [future-model-scope.md](future-model-scope.md).
The material workflow and its Revit/Omniverse output contracts remain supported;
those material outputs are not model adapters or a renderer.
