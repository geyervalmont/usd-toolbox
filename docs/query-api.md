# Model documents, metadata and queries

The reusable `SceneDocument` owns one validated scene and its indexes. Rust,
the CLI, WASM worker and C document handles use the same query implementation.
Geometry stays in the document; a query projects only requested fields.

## Contracts and migration

| Contract | Identifier |
|---|---|
| Neutral scene | `usd-toolbox.scene.v1` |
| Typed metadata registry | `usd-toolbox.metadata.v1` |
| Query page | `usd-toolbox.query-page.v1` |
| Capability discovery | `usd-toolbox.model-capabilities.v1` |
| Authored USD preflight | `usd-toolbox.preflight.v1` |
| Authored glTF preflight | `usd-toolbox.gltf-preflight.v1` |

New readers accept older scene-v1 files without the optional metadata/tangent
fields and initialize defaults. Writers preserve existing stable IDs. Unsupported
schema identifiers and unknown request fields fail explicitly. No implicit
conversion of a future schema is performed. Capability discovery distinguishes
the portable USD profile from optional native preprocessing.

Metadata is separate at scene, node, mesh and material scopes. `original` retains
source properties, types and locations; `normalized` contains application-neutral
catalogue fields. Namespaced keys such as `product_id`, `manufacturer_id`, `tags`,
`classification` and `description` are conventions rather than required BIM fields.
Computed bounds are query projections with units, derivation and generation.

Every metadata value is tagged. Signed/unsigned 64-bit integers use canonical
decimal strings, including in JavaScript, so `18446744073709551615` remains exact.
Nested dictionaries, arrays and source-typed arrays are supported. Missing keys
are absent; `{type:"null"}` is an explicit null; empty strings/arrays stay empty.
Properties record authored/defaulted/inherited/unresolved state. An unresolved
property can be returned for inspection but never matches an equality/numeric
predicate as a resolved answer. `exists` tests presence, including unresolved/null.

## Query rules

```json
{
  "filter": {
    "node_type": "mesh",
    "metadata": [{"key":"product_id","operator":"exists","exists":true}]
  },
  "fields": ["name", "source_path", "bounds", "effective_material"],
  "sort": "name",
  "limit": 50
}
```

Filters combine with AND. ID lists match any listed ID. Name/path matching is
exact by default, with optional prefix/contains and Unicode lowercase matching;
it is not locale collation or regular-expression evaluation. Metadata equality is
type-sensitive. Numeric comparisons compare i64/u64 exactly across signedness;
f64 compares with f64. Integer-to-float coercion is intentionally absent.

Visibility, purpose and direct material inheritance are resolved through the
hierarchy. Material predicates also include face-subset bindings. Subtree includes
its root. Sorting is stable by ID, name or source path, with ID as the tie-breaker;
`descending` reverses that order. Missing paths have a deterministic position.

Use `next_cursor` with the identical request for the next page. Cursors bind the
content digest, generation, filter, projection, sort and limit. The unchanged file
can be reopened by another CLI process; changed content or edits reject old
cursors with `query.stale_cursor`. Cursors are opaque continuation tokens, not
authorization credentials. Hosts must enforce their own access control.

`bounds` is the exact world-space bounds of that node's own mesh, expressed in
metres. Group nodes without meshes return null; use inspection for total scene
bounds. Local/world transforms use the scene's coordinate units. Dependencies and
source hashes are opt-in; dependency lists describe the source scene, not a
per-node reachability calculation. Source paths remain source identities after
rename/reparent; they do not pretend to be newly authored USD paths.

ID/path and hierarchy indexes are built once. Select up to 32 metadata indexes
through `DocumentOptions.metadata_indexes`; indexing every metadata value is not
automatic. World matrices and bounds have bounded caches. Mutations go through
atomic document methods and advance generation. Metadata/name edits update only
affected records/indexes and retain geometry, images and applicable cached values.
Structural/geometry edits rebuild required indexes and invalidate derived caches.

Default query limits are 1,000 rows, 2 MiB returned data, 64 predicates, 32 MiB
estimated indexes and 32 MiB estimated cache entries. Metadata has independent
depth/property/aggregate budgets. These bound work categories; they are not an
exact process-RSS guarantee. A response exceeding its budget fails explicitly.

## Interfaces

Rust: `SceneDocument::with_options(scene, options)`, then `query`, `count`,
`get_node`, `get_metadata`, `get_property`, `ids_for_path`, `children`, `ancestors`,
`descendants`, `summary` and atomic `apply`. The document exposes no mutable scene.

```sh
usd-toolbox model query --input model.usdz --request query.json
usd-toolbox model query --input model.usdz --request query.json --count
usd-toolbox model metadata --input model.usdz --node /Asset/Part
usd-toolbox model metadata --input model.usdz --node /Asset/Part --key product_id
usd-toolbox model preflight --input model.usda --target glb --geometry
usd-toolbox model capabilities
```

The CLI resolves only referenced files beneath the input directory, including
canonical/symlink confinement. Missing files remain preflight findings. The Rust,
C and browser libraries receive dependency bytes from their host and perform no
filesystem/network access. Metadata-focused preflight skips image decoding and
binary mesh materialization; the USDA text parser still parses authored values.
Dependency discovery followed by full import can parse a layer twice; this is
reported as a remaining full-buffer boundary rather than a streaming claim.

Browser usage and lifetime rules are in [the web package](../packages/web/README.md).
The package adds typed `getProperty`, `idsForPath`, `ancestors`, `descendants` and
`count` conveniences over the same structured query contract.

C: `usd_toolbox_document_open_v1` imports once; `open_bundle_v1` accepts borrowed
filename/data slices without JSON binary arrays. `document_request_v1` accepts
tagged operations (`query`, `count`, `node`, `metadata`, `property`, `children`,
`ancestors`, `descendants`, `path`, `summary`, `apply`, `export`, `scene_json`). It
returns a prepared result handle. Size/copy with `result_copy_v1`, then release it.
Sizing/copy never repeats an edit or export. Results outlive the originating
document. Release documents independently. Integer handles are process-local and
must not be persisted across runs. Duplicate release fails. Separate documents
can run concurrently; requests to one document serialize.

See the [executable C example](../examples/c/document.c). The registry caps open
documents/results at 256 each and prepared bytes at 512 MiB; callers should release
handles promptly and impose an overall document-memory budget appropriate to their
application. Input buffers remain caller-owned; no borrowed pointer escapes import.
