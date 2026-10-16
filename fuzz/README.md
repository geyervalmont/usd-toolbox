# Bounded format and query fuzzing

The eight libFuzzer targets exercise USDA, USDC, USDZ, MaterialX, glTF, PNG/JPEG,
typed metadata and structured queries. Inputs, scene depth/counts, image dimensions,
diagnostics, query results and per-input time are bounded. ASan is enabled by
cargo-fuzz. Successful imports must validate; exported static glTF must reimport;
metadata must round-trip; repeated queries must agree and edited cursors must fail.

```sh
rustup toolchain install nightly-2026-10-15
cargo install cargo-fuzz --version 0.13.2 --locked
python scripts/run_fuzz_smoke.py --seconds 10
```

The runner builds once, then runs instrumented binaries directly. Logs, execution
counts, configured limits and failure artifacts go under `target/fuzz-smoke`.
`--skip-build` is for unchanged instrumented binaries only. CI runs short campaigns
and a longer weekly campaign. A smoke pass is regression evidence, not proof that
arbitrary malformed inputs are safe or all paths have been covered.

The USDC count/allocation regression is retained in `regressions/` and covered by
the USD suite. The bounded parser patch and upstream licence are in
`vendor/openusd/PATCHES.md`. Do not discard a failure artifact before turning its
underlying bug into a deterministic test.

Most seeds are small project fixtures or hand-written schema boundary cases.
`seeds/gltf/box.seed` is the Khronos sample Box GLB (Cesium, CC BY 4.0), pinned and
attributed in `fixtures/corpus/manifest.json` and [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md),
which preserves the upstream copyright and licence notice. Other supplied model
seeds derive from the project's synthetic triangle/checker fixtures.
