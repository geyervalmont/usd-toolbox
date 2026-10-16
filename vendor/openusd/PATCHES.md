# Local OpenUSD Rust parser patch

Base: openusd 0.7.0 from crates.io, MIT licence in LICENSE. Upstream source:
https://github.com/mxpv/openusd (crate metadata records upstream revision 93252c5a2402fe0272367532c0e002e06315589d).

The upstream copyright and permission notice is retained byte-for-byte in `LICENSE`.
It was restored on 2026-10-16 from the official repository's [root licence at the
exact crate source revision](https://raw.githubusercontent.com/mxpv/openusd/93252c5a2402fe0272367532c0e002e06315589d/LICENSE), matching
`.cargo_vcs_info.json` for openusd 0.7.0. SHA-256:
`ed864f20ae0294c6b61b3c88bed43bedf46d93f90c8cbc0bb4597c5131c6bb6f`. The vendored source previously omitted
this root-level file; the MIT identifier alone does not contain its notice.

This copy is a narrow portable parser-hardening fork. It adds `usdc::ReadLimits`
and `CrateData::open_with_limits`: count/byte/offset checks before allocation,
cumulative decoded-work accounting, recursive-value and path traversal limits,
checked integer widths, index validation, and errors for unsupported LZ4 chunking
and path-expression evaluation. It also checks compressed integer stream slices
and stops at the declared integer count. Decoder debug assertions on file-controlled
data return errors in every build. No native USD dependency was added.

The regression was found by the toolbox USDC libFuzzer target: a token-list count
in a tiny corrupted crate caused an unchecked enormous allocation. The minimized
seed and original reproducer are kept in `fuzz/regressions` and covered by the
USD preflight/import regression test. Error context retains the underlying reason. Limits bound decoded work and
individual allocations, not exact process RSS; the host still supplies input-byte
limits and scene limits. The default decoder allows 128 MiB per allocation and
512 MiB cumulative decoded work; scene APIs pass their configured limits.

Keep this patch reviewed when updating upstream. Optional native normalization
uses the separately installed OpenUSD Python runtime and does not use this fork.
