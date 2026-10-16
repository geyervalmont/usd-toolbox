# Full OpenUSD shader registry validation

`scripts/validate_usd_full_registry.py` validates exported stages with native
OpenUSD validators and actual MaterialX Sdr discovery/parser plugins. It fails
when `usdMtlx` is absent, its discovery/parser types are not registered, required
standard shader identifiers do not resolve, the shader compliance validator is
missing, or **any** native validator finding occurs. There are no ignored shader
IDs or validator exemptions. The report includes loaded plugin/library hashes,
resolved shader definitions and their hashes, every loaded validator, input hashes,
shader instances and findings.

This is separate from `validate_materialx.py`, which validates MaterialX documents,
and from the compatibility fallback in `validate_usdz.py`. Passing either fallback
is not evidence of passing full Sdr validation. The full validator currently
requires ID-based shader implementations; source-code shaders need a separate
explicit validation policy.

## Reproducible reference runtime

The verified environment is Linux x86_64, Python 3.12.14, OpenUSD 26.08 from the
NVIDIA `usd-exchange==3.0.1` wheel, and complete `MaterialX==1.39.5` standard
libraries. Both wheels are hash-pinned in
[`scripts/reference-registry-requirements.txt`](../scripts/reference-registry-requirements.txt).
The NVIDIA distribution includes `usdMtlx` native plugins and OpenUSD validator
plugins; it bundles a subset of MaterialX libraries, so the complete official
MaterialX library path must be supplied explicitly.

The [NVIDIA SDK installation documentation](https://github.com/NVIDIA-Omniverse/usd-exchange)
and [release notes](https://github.com/NVIDIA-Omniverse/usd-exchange/blob/main/CHANGELOG.md)
describe the bundled runtime. Do not install `usd-core` in the same environment:
both distributions provide `pxr` and would overwrite shared files. The validator
rejects that mixed configuration. The standard-library path is an official
[UsdMtlx discovery setting](https://openusd.org/release/api/_page__material_x__in__hydra__u_s_d.html);
these files are parsed by the native plugin, not checked as a substitute for it.

Using the already available official Python container keeps installation under
`target/` and changes no system Python packages. The verified image digest is
`python@sha256:2f17fc044b579bab302c2e8054d3a686e2cb9a83de48e70534b94cd8ebbe06a9`.
Run from the repository root:

```sh
mkdir -p target/reference-full-registry

docker run --rm --user "$(id -u):$(id -g)" \
  -v "$PWD/scripts:/scripts:ro" \
  -v "$PWD/target/reference-full-registry:/reference" \
  python@sha256:2f17fc044b579bab302c2e8054d3a686e2cb9a83de48e70534b94cd8ebbe06a9 \
  python -m pip install --no-cache-dir --no-deps --only-binary=:all: \
  --require-hashes --target /reference/runtime \
  -r /scripts/reference-registry-requirements.txt
```

Generate fresh exports using the same examples and CLI commands as reference CI:

```sh
mkdir -p target/reference-full-registry/exports
cargo run -q -p usd-toolbox-usd --example write_usdz_fixture -- \
  target/reference-full-registry/exports/material.usdz
cargo run -q -p usd-toolbox-materialx --example write_pbr_fixture -- \
  target/reference-full-registry/exports/pbr
cargo run -q -p usd-toolbox-cli -- export --input fixtures/materialx/procedural.mtlx \
  --target usdz --output target/reference-full-registry/exports/procedural.usdz
cargo run -q -p usd-toolbox-cli -- model pack --input fixtures/models/panels.usda \
  --output target/reference-full-registry/exports/model.usdz
cargo run -q -p usd-toolbox-cli -- model pack --input fixtures/models/desk.usda \
  --output target/reference-full-registry/exports/desk.usdz
cargo run -q -p usd-toolbox-cli -- model pack --input fixtures/models/transforms.usda \
  --output target/reference-full-registry/exports/transforms.usdz
```

Then run validation with no network, read-only container filesystem and bounded
container resources. Only the report/output directory is writable:

```sh
docker run --rm --network none --read-only --cap-drop ALL \
  --security-opt no-new-privileges --memory 2g --cpus 4 \
  --user "$(id -u):$(id -g)" \
  -v "$PWD/scripts:/scripts:ro" \
  -v "$PWD/target/reference-full-registry:/reference" \
  -e PYTHONPATH=/reference/runtime \
  -e PXR_MTLX_STDLIB_SEARCH_PATHS=/reference/runtime/MaterialX/libraries \
  python@sha256:2f17fc044b579bab302c2e8054d3a686e2cb9a83de48e70534b94cd8ebbe06a9 \
  python /scripts/validate_usd_full_registry.py --report /reference/report.json \
  /reference/exports/material.usdz /reference/exports/pbr/material.usdz \
  /reference/exports/procedural.usdz /reference/exports/model.usdz \
  /reference/exports/desk.usdz /reference/exports/transforms.usdz
```

CI can instead create an isolated Python 3.12 virtual environment, install the same
requirements and set `PXR_MTLX_STDLIB_SEARCH_PATHS` to that environment's
`MaterialX/libraries` directory. Keep the separate `usd-core` tests in their own
environment. Native references are validation dependencies only; Rust/WASM users
do not need either Python package.

## Recorded result and regression evidence

On 2026-10-16, the six fresh exports above passed all **28 loaded OpenUSD native
validators**, with zero findings and zero exemptions. The material/PBR/procedural/
model/desk exports resolve 1/17/3/3/2 shader instances respectively; transforms has
no materials. Evidence is generated at `target/reference-full-registry/report.json`.
It records `UsdMtlxDiscoveryPlugin` and `UsdMtlxParserPlugin` ownership/loading,
OpenPBR, standard_surface, displacement, image and normal-map registry probes,
and the `usdShadeValidators:ShaderSdrCompliance` validator.

The initial unfiltered run found real serialization defects: MaterialX `vector3`
inputs were authored as USD `vector3f`, while the native registry requires `float3`;
and procedural noise with scalar amplitude selected the vector-amplitude overload.
The exporter now emits `float3` and selects the MaterialX `FA` noise/fractal
overload when an authored amplitude is scalar. Original MaterialX graph data is
unchanged. Rust regression tests check emitted types, overload selection and graph
roundtrip preservation. No fixture or registry exemption was used to obtain a pass.

Negative controls were also run: the `usd-core` environment fails for missing
`usdMtlx`; the NVIDIA runtime without the complete library path fails required
standard/displacement probes; and an intentionally unknown shader ID produces the
actual native `MissingShaderIdInRegistry` error. Their local evidence is
`negative-missing-plugin.json`, `negative-incomplete-library.json` and
`negative-unknown-id.json` beside the main report. `report-before-fixes.json`
retains the original native property-type failures.

This verifies registry/schema/connection compatibility for the exported fixture
corpus at the pinned runtime versions. It does not demonstrate renderer image
parity, arbitrary procedural evaluation, or compatibility with every future
MaterialX/OpenUSD version. Keep full-registry validation in the reference gate
when upgrading either dependency or changing shader serialization.
