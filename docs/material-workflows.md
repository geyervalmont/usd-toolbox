# Material-library workflows

The toolbox owns deterministic, per-package material operations. OPAL owns the
catalog, queues, approvals, immutable revisions, and vector database. AI and
manual tools are interchangeable producers of bytes; neither is a dependency
of the Rust library.

## Inspect, audit, edit, and diff

```bash
usd-toolbox inspect --input material.usdz --output inspection.json
usd-toolbox audit --input material.usdz --report health.json
usd-toolbox validate --input material.usdz --report validation.json
usd-toolbox diff --before old.usdz --after new.usdz --output diff.json
usd-toolbox apply --input material.usdz --patch patch.json --output edited.usdz
```

`inspect` never emits texture byte arrays. `audit` emits
`usd-toolbox.library-audit.v1`: aggregate scores, visible health facets,
actionable diagnostics, exact duplicate hashes, and perceptual duplicate
candidates. A JSON `AuditProfile` can override required roles/tiers and metadata
rules. The default portable-PBR profile requires base colour, normal, roughness,
AO, preview through 4K, physical tiling, dimensions, and provenance.

The score is a UI convenience. Issue `code`, `severity`, `path`,
`suggested_fix`, and `fixability` are the durable automation contract.
`validate` writes the same report but returns a failing process status when any
error-severity diagnostic makes a material invalid; warnings remain non-fatal.

`apply` uses RFC 7396 merge semantics and validates the complete result before
writing a new USDZ. Domain edits which carry bytes use explicit commands:

```bash
usd-toolbox extract-texture --input material.usdz --role base_color \
  --tier 4k --output base-color.png

usd-toolbox remove-texture --input material.usdz --role ao --tier 2k \
  --output without-ao.usdz
```

Legacy raw material-array JSON can be upgraded to the stable
`usd-toolbox.material.v1` envelope with `usd-toolbox migrate`.

## Texture tiers and external generation

Texture resolution uses the word **tier**. Geometry will use **LOD**.

```bash
usd-toolbox plan-tiers --input material.usdz --policy tiers.json --report jobs.json
usd-toolbox complete-tiers --input material.usdz --policy tiers.json \
  --created 2026-09-08T03:00:00Z --output completed.usdz --report remaining.json
```

Jobs are `downscale`, `external_generation`, or `author_missing_role` and carry
the parent hash/tier/dimensions plus exact target dimensions. `complete-tiers`
performs only safe deterministic downscales. Upscaling and synthesis remain
external.

Both an AI worker and a human uploader use `attach-texture`. The caller supplies
role, tier, colour space, channel, normal convention, optional parent hash, and
provenance (`operation`, `tool`, `tool_version`, optional `model`, parameters,
and an RFC3339 timestamp). The toolbox verifies encoding, exact tier size,
metadata consistency, parent resolution, and content hash before writing a new
revision. It never overwrites the source material.

## Procedural material recipes

Procedural materials are clean, versioned inputs rather than opaque editor
state. The first schema supports `paint`, `masonry`, `timber`, `terrazzo`, and
`textile`. Every definition includes pixel size, real-world size, and a seed;
the bake produces deterministic base-colour, OpenGL normal, roughness, height,
and metallic PNGs. Masonry, timber, and textile also produce an editable SVG
hatch and a model-scale Revit PAT file.

Patterned definitions must describe a complete repeat: masonry dimensions align
to unit-plus-joint pitches (and a complete bond cycle), staggered timber aligns
to pairs of board rows, and textile dimensions align to its weave repeat. The
baker rejects cropped edges instead of labelling a visibly seamed image as
tileable.

```bash
usd-toolbox bake-procedural --definition recipe.json \
  --output-dir generated --report bake.json
```

The report is `usd-toolbox.procedural-bake.v1` and contains the canonical
definition digest plus each output path, media type, byte count, and SHA-256;
it never repeats image bytes inside JSON. The same `bake_procedural` operation
is available through WASM and the panic-safe C ABI. Applications own recipe
drafts, access control, review, and publication. A bake is a candidate revision
and must not silently replace an approved catalog representation.

Example paint definition:

```json
{
  "schema": 1,
  "width_px": 1024,
  "height_px": 1024,
  "width_mm": 1000,
  "height_mm": 1000,
  "seed": 42,
  "generator": "paint",
  "parameters": {
    "colour": "#D4CABA",
    "roughness": 0.62,
    "variation": 0.02,
    "texture_depth": 0.1
  }
}
```

## Embeddings and clustering

```bash
usd-toolbox embedding-input --input material.usdz --output embedding-input.json
usd-toolbox cluster --input embeddings.json --similarity-threshold 0.85 \
  --min-cluster-size 3 --output clusters.json
```

`embedding-input` creates a canonical text document, labelled structured PBR
features, a selected base-colour visual input, and an input digest. The caller
chooses and invokes visual/text embedding models. Store modalities separately
so “looks similar” and “specified similarly” remain distinct operations.

Every returned `EmbeddingRecord` contains entity, modality, model,
model-version, input digest, and vector. The built-in clustering algorithm is a
deterministic cosine-neighbourhood baseline with outliers; OPAL may run HDBSCAN
instead while retaining the same versioned record contract. Computed clusters
are suggestions, never canonical taxonomy.

For PostgreSQL/pgvector, a suitable application-owned schema is:

```sql
CREATE EXTENSION IF NOT EXISTS vector;

CREATE TABLE material_embeddings (
  id bigserial PRIMARY KEY,
  entity_type text NOT NULL,
  entity_id bigint NOT NULL,
  modality text NOT NULL,
  model text NOT NULL,
  model_version text NOT NULL,
  dimensions integer NOT NULL,
  input_digest char(64) NOT NULL,
  embedding vector NOT NULL,
  created_at timestamptz NOT NULL DEFAULT now(),
  UNIQUE (entity_type, entity_id, modality, model, model_version, input_digest)
);

CREATE TABLE material_cluster_runs (
  id bigserial PRIMARY KEY,
  model text NOT NULL,
  model_version text NOT NULL,
  algorithm text NOT NULL,
  parameters jsonb NOT NULL,
  created_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE material_cluster_memberships (
  run_id bigint NOT NULL REFERENCES material_cluster_runs(id),
  entity_type text NOT NULL,
  entity_id bigint NOT NULL,
  cluster_key text,
  is_outlier boolean NOT NULL,
  PRIMARY KEY (run_id, entity_type, entity_id)
);
```

Choose a dimensioned pgvector column/index in an OPAL migration once the model
is selected. Start with exact cosine search; add an approximate index only after
benchmarking the real corpus.
