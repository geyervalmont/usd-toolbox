# USD profiles, preservation and native preprocessing

The portable library imports an authored static polygon layer from caller-supplied
USDA, USDC, USDZ or a named byte bundle. It does not open a filesystem, contact a
resolver or compose arbitrary USD stages. The same Rust adapter runs natively and
in WASM. `usd_scene_capabilities()` reports the versioned `static-polygon-v2`
profile; callers should check capabilities rather than infer them from extensions.

## Portable scene profile

Supported geometry includes polygon meshes with `subdivisionScheme = none`,
indexed topology, hierarchy, USD transform stacks and reset stacks, normals,
multiple UV sets, vertex colors, tangents, direct material bindings and face
material subsets. Inactive subtrees are omitted with diagnostics. Constant
primvars inherit through ancestors, and direct material bindings respect
`strongerThanDescendants`. Unsupported active schemas and composition arcs fail
with source paths and stable codes.

PreviewSurface projection follows Material and NodeGraph interface connections,
including forwarded constants, with cycle/depth checks. Texture inputs support
explicit repeat, clamp and mirror wrapping, UV names `st` through `st31`, one
`UsdTransform2d`, and cutout opacity. Sampler, UV and alpha settings are retained in
`gltf:render_settings` metadata for the GLB exporter. UV transforms are conjugated
by the USD-to-glTF V-coordinate flip; rotation changes sign. USD `useMetadata`
and `black` wrapping cannot be represented by this glTF sampler profile and are
rejected. A missing wrap input has those USD fallback semantics; it is not assumed
to repeat. These semantics follow the
[OpenUSD PreviewSurface specification](https://openusd.org/dev/spec_usdpreviewsurface.html).

Shader graphs are archived in typed source metadata. The emitted shader is a
projection of the supported neutral material model. Archive retention does not
mean arbitrary shaders are evaluated or that every archived material setting is
represented by the emitted USD shader. Export diagnostics identify this boundary.
EXR decoding, UDIM evaluation, layered shader evaluation, arbitrary composition,
native instancing, point instancers, animation and subdivision evaluation are not
portable import capabilities.

## Metadata and compatibility

`usd-toolbox.metadata.v1` separates original source properties from normalized
catalogue properties. Properties carry source format/path/type, authored/defaulted/
inherited/unresolved state and optional units. Missing properties are absent keys;
unresolved values do not match resolved-value filters. Signed/unsigned wide
integers use decimal strings in typed JSON so JavaScript cannot round identifiers.
USD token/string distinctions, numeric widths, nested dictionaries and supported
arrays remain typed. Custom attributes with supported default values, customData,
assetInfo, kind and selected prim/layer metadata are preserved; unsupported values
or output fields generate diagnostics. Arbitrary relationships, custom attribute
qualifiers and unavailable authored values are not promised lossless preservation.

The scene manifest remains `usd-toolbox.scene.v1`: missing metadata in older
packages defaults to an empty registry. Readers accept legacy scalar JSON manifest
strings and the newer UTF-8-safe string-array chunks. Large or heavily escaped
manifests use chunks to avoid pathological text-lexer recursion. The material
manifest version and legacy small-file representation remain unchanged. A source
shader graph is archived on first external import only; embedded toolbox manifests
are never recursively archived. Four repeated roundtrips are tested for stable
metadata and byte size.

Unknown scene/metadata schema versions are rejected rather than silently guessed.
Applications migrating metadata should read the explicit schema, retain original
source properties, write application normalization to the normalized map, and
record the operation in provenance. Changes to the capability profile, preflight
schema and native adapter schema are versioned independently of package versions.

## Preflight and limits

`scene::preflight(input, options)` always returns a report, including parse errors.
It accumulates prim/schema/shader inventory, dependency status, typed raw metadata,
compatibility issues and remedies even when strict scene import cannot succeed.
Missing supplied dependencies do not prevent independent metadata inventory.
`complete` describes whether requested scanning finished; it does **not** mean the
file is exportable. Inspect issue severity and the `performed`/`deferred` lists.
Choose `usd`, `usdz`, `glb` or `browser_preview`; `check_geometry` additionally
projects and validates topology, bounds, triangulation and target transform limits.
Image decoding, renderer equivalence and external composition are explicitly
outside this scan. Estimates are conservative geometry measures, not RSS figures.

Limits cap encoded bytes, specs/fields, dependencies, diagnostics, metadata bytes
and scene complexity. USDA parsing materializes source values before the spec scan;
source-byte, lexical-branch and nesting guards apply first. USDC structural sections
are decoded before scanning and geometry values are read lazily. The vendored
OpenUSD Rust parser checks counts, byte arithmetic, offsets and file extents before
allocating. It charges individual allocations and cumulative decoded work, bounds
value recursion/calls and path lengths, and rejects unsupported encodings rather
than panicking. Defaults permit 128 MiB per allocation and 512 MiB cumulative
work; scene APIs tighten these against `SceneLimits`. Cumulative work can include
repeated decoding and is deliberately more conservative than live memory.

These limits are not an exact allocator/RSS guarantee. Hosts should still apply
worker/process limits for untrusted inputs. The local MIT-licensed parser changes
and update policy are documented in [the vendored patch notes](../vendor/openusd/PATCHES.md).
Compressed integer streams and the original/minimized token-list fuzz failures have
regressions. Old crate versions before 0.4, multi-chunk LZ4 and path-expression
value evaluation are rejected by this bounded parser profile.

Performance-sensitive paths index prim children, properties and shader properties
once, avoid repeated scene-wide searches, and inventory binary metadata without
reading mesh arrays. Preserve `SceneHandle` query indexes across repeated queries;
use report limits and metadata-only preflight before expensive geometry work.
Benchmarks must distinguish eager text parse, lazy binary inventory, full import,
triangulation and export; a metadata-only scan is not a full-import benchmark.

## Optional native OpenUSD adapter

`scripts/normalize_usd.py` is an explicit host preprocessing tool using separately
installed OpenUSD Python bindings. Neither the Rust crates nor the WASM package
load Python or native USD. Use it when an input requires real stage composition:

```sh
python scripts/normalize_usd.py assets/root.usda \
  --root assets --output normalized-chair \
  --time 1 --purpose render --payloads load --variants explicit \
  --variant /Chair:finish=oak
```

The output directory must not already exist. Time, geometry purpose, payload load
policy and variant policy are required. `--variants authored` explicitly accepts
authored selections; `explicit` requires every visible variant set to be selected.
Material purpose (`all`, `full`, `preview`) is separate from geometry purpose and
can be chosen with `--material-purpose`. `--render-context` selects a shader
context; its resolved surface must be PreviewSurface. Unknown schemas, unsupported
prims and unrepresented relationships reject by default. `--unsupported omit`
allows only explicitly reported omissions; it does not enable shader baking or
subdivision approximation.

Before composing, the adapter inventories possible external layers, including
unselected variants and payloads, and asset-valued defaults/time samples. Every
regular file must resolve within the caller's canonical root. URI schemes, package
paths, external roots and dependency cycles are rejected. It uses OpenUSD opinion
strength for sublayers/references/payloads/variants and binding resolution, then
samples the requested time. Collection and purpose-specific bindings become
resolved direct bindings in the static output.

Native instance proxies become ordinary placed nodes. Point-instancer transforms,
prototype-root transforms, IDs and visibility masks are evaluated by OpenUSD;
placements share stable mesh identities after portable import. Geometry hashes
include resolved inherited primvars so different colors/UVs cannot merge
incorrectly. Native normalization may duplicate geometry in the USD wire layer;
shared neutral meshes are recovered from validated identities. Nested point
instancers are rejected. Polygon subdivision, holes and creases require a real
tessellator and are rejected even with omission policy. No evaluator is bundled.

The adapter writes `normalized.usda`, confined asset sidecars and a
`usd-toolbox.native-normalization.v1` report containing explicit policies,
OpenUSD/adapter versions, source/output hashes, dependency inventory, mesh counts
and projection/loss diagnostics. Source layer metadata field names are inventoried;
fields not represented in the static output are reported. Typed selected prim and
attribute metadata is copied inside USD to avoid Python dictionary type erosion.
Inputs are checked again before atomic publication. The result is a static layer
and supplied dependencies suitable for the portable preflight/import APIs.

File/spec/node/depth/expanded-geometry/diagnostic/output budgets are enforced. Native
OpenUSD can allocate while opening/composing/flattening a stage before every
post-composition budget is measurable. Run this optional adapter in a bounded host
process for untrusted files; it is not a memory-safe replacement for the portable
parser and is not a network-enabled resolver service.

## Fixture and conformance policy

`fixtures/models/metadata-semantics.usda` is an original synthetic toolbox fixture:
its expected wide integers, nested token/string metadata, Unicode, inactive
pruning, inherited color, binding strength and shader interfaces are checked by
Rust and by OpenUSD. Native normalization tests generate their own local layers
for strength, variants, payloads, sampled transforms, native/point instances,
prototype transforms/masks, inherited primvars, confinement, depth, collisions and
subdivision refusal. No copyrighted production models or network dependencies are
needed by these tests.

```sh
cargo test -p usd-toolbox-usd
USD_TOOLBOX_BIN="$PWD/target/debug/usd-toolbox" \
  python scripts/test_normalize_usd.py -v
```

The second command requires the optional reference runtime and a built CLI; CI
pins the reference runtime separately. With `USD_TOOLBOX_BIN`, the test also
imports normalized USD through the portable adapter and verifies shared mesh
identity. Fuzz regressions under `fuzz/regressions` are byte-level parser cases,
not claims of supported malformed files. Keep failures and minimized inputs when
updating the vendored parser, and run both fuzz replay and semantic conformance.
