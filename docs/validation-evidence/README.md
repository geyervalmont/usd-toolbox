# Model toolbox validation evidence

Recorded on 2026-10-16. These reports describe the tested inputs, builds and
environments; they do not extend the supported profiles or advertised limits.
The remaining delivery gates are tracked in [todo.md](../../todo.md).

| Check | Recorded result | Evidence / reproduction |
|---|---|---|
| Rust workspace | 154 tests passed, no failures; strict all-target Clippy and formatting passed | `cargo test --workspace --locked`; `cargo clippy --workspace --all-targets --locked -- -D warnings`; `cargo fmt --all --check` |
| Browser workers | Chromium, Firefox and WebKit passed queries, edits, cancellation, LOD export and disposal | [Engine report](browser-engines.json); `scripts/test_web_package.mjs` |
| Web application integration | Production Vite build worked under `/models/` | `scripts/test_vite_package.mjs` |
| Native/WASM parity | Matching metadata, queries, pagination, diagnostics, edits and deterministic exports | `scripts/validate_model_wasm.cjs` |
| OpenUSD reference validation | Six fresh exports, 28 native validators, zero findings and exemptions | [Full shader registry report](full-shader-registry.json); [pinned runtime and negative controls](../reference-validation.md) |
| Native composition adapter | Ten conformance tests passed, including shared geometry across instances | `scripts/test_normalize_usd.py`; [supported profile](../usd-profiles.md) |
| Fuzz smoke | Eight targets, 3,459,779 executions, no failures in the final smoke run | [Report](fuzz-smoke.json); `scripts/run_fuzz_smoke.py` |
| Native performance | Eight workload groups passed deterministic allocation/lifecycle gates | [Baselines and thresholds](../performance.md) |
| Browser performance | Desktop and 1-CPU/1-GiB container measurements | [Desktop](browser-desktop.json), [constrained container](browser-constrained-container.json), [interpretation](../web-performance.md) |
| Native C consumers | Linux GNU/musl, Windows MSVC and macOS Intel/ARM builds compiled, linked and executed real C consumers | [Platform jobs and artifacts](github-ci.json); `scripts/test_c_api.py`; `examples/c` |
| Package footprint | Separate minimal and material-enabled WASM measurements | [Build sizes](web-build-sizes.json) |

The public model corpus is pinned by hashes and includes source licence records
in [its manifest](../../fixtures/corpus/manifest.json). Acceptance/rejection,
Khronos validation, texture preview comparisons and vertex-ordering results are
documented in the [glTF profile](../../crates/gltf/README.md),
[texture assessment](../../crates/gltf/texture-profile.md) and
[ordering assessment](../../crates/gltf/vertex-ordering.md).

Browser and performance reports identify the builds used. Earlier BoomBox
performance runs predate the LOD packaging and queued-request scheduling changes;
they are retained as attributed baselines, not relabelled as final-build results.
Fuzzing is a bounded smoke test, not proof that malformed inputs cannot fail.

Two gates need external inputs or execution environments: permissioned customer
models with exporter versions and a physical constrained device. A constrained
container does not supply physical-device evidence.

[Hosted CI evidence](github-ci.json) records the tested source commit, successful
correctness and package runs, individual platform jobs and artifact digests.
It includes executable C consumers on all five native targets, actual browser
engines and reference-format validators. Artifact downloads expire according to
their recorded retention dates; the recorded outcomes remain attributable to
the tested commit.

Versioned local packages include headers or TypeScript declarations, examples,
checksums and dependency notices. The local measurements below predate public CI
publication; subsequent CI artifacts follow the repository's visibility. The
project has no declared distribution licence and the npm package is marked private.

[Local package verification](local-packages.json) records final archive hashes,
verified file checksums and execution of the C example from an extracted GNU
archive. It was recorded after packaging and is kept outside those archives.
