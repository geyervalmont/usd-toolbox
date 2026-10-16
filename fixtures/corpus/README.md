# Public interoperability corpus

`manifest.json` pins each source revision, exact download, SHA-256, byte length,
credits and licence identifier. Binary models are fetched into ignored
`target/corpus`, not embedded in the source repository. Original model READMEs
travel with every download; consult them for full credits and licence links.
The unchanged Box GLB also ships as a fuzz seed, with its full upstream
[attribution notice](../../fuzz/THIRD_PARTY_NOTICES.md).

```sh
python scripts/fetch_corpus.py
python scripts/fetch_corpus.py --verify-only
cargo test -p usd-toolbox-gltf --release --test corpus -- --ignored --nocapture
```

Sources are the [Khronos glTF Sample Assets](https://github.com/KhronosGroup/glTF-Sample-Assets).
The corpus contains Box (Cesium, CC BY 4.0), BoomBox (Microsoft, CC0),
NormalTangentMirrorTest (Ed Mackey / Analytical Graphics, CC BY 4.0),
SheenChair (Eric Chadwick / Wayfair, CC0), and RecursiveSkeletons (see its
upstream SharpGLTF/Cesium credits, CC BY 4.0).

Box, BoomBox and NormalTangentMirrorTest exercise supported static ingestion,
materials, shared geometry and authored tangent fidelity. SheenChair and
RecursiveSkeletons exercise explicit unsupported-extension/animation reporting;
their presence does not claim support for those features. Expected outcomes are
checked by the corpus runner, including source hashes.

These public assets broaden interoperability evidence. They do not establish
compatibility with private customer assets or every exporter version. Add
permissioned representative inputs and retain their exact exporter/version and
licensing records as they become available. Original synthetic USD fixtures
remain under `fixtures/models` for isolated semantics and deterministic tests.
