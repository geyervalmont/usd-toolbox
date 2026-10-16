# Browser execution and measured costs

Measured 2026-10-16 on Linux x86-64 / Ryzen AI 9 HX 370 with the official Playwright
1.64.1 browser container. Raw results and package digests are in
[validation-evidence](validation-evidence). These are development measurements,
not browser capacity guarantees or a physical mobile-device test.

The final release model-only build measured 8,404,755 bytes raw / 1,871,941 gzip
bytes; enabling material tools measured 12,225,272 raw / 2,745,078 gzip bytes.
The exact package hash is in each report; later builds must be compared by their
own build report, not assumed identical. Initialization for the small conformance
fixture measured 31 ms Chromium, 140 ms Firefox and 71 ms WebKit. Query p95 was
0.4 ms Chromium and 1 ms Firefox/WebKit; timer resolution limits these small values.
All engines passed worker loading, metadata/integer queries, pagination, stale
cursors, edits, GLB reimport, cancellation and repeated open/query/dispose cycles.
The fixture retained 1.875 MiB of reusable WASM linear memory with zero live models
after disposal. That is not total browser RSS or a guarantee for larger assets.

The 10,614,184-byte BoomBox corpus model measures useful whole-worker costs. These
runs used the earlier 8,122,177-byte build identified in their reports, before the
LOD packaging and explicit queued-work permit protocol were added:

| Environment | Warm import, copied input | Warm import, transferred input | Query p95 | WASM high-water allocation |
|---|---:|---:|---:|---:|
| Desktop container | 392–589 ms | 387–468 ms after first transfer | 0.1–0.2 ms | 83.75 MiB |
| Container limited to 1 CPU / 1 GiB | 393–423 ms | 390–409 ms | 0.1–0.2 ms | 83.75 MiB |

First metadata followed opening by approximately 0.2–4 ms. Initial cold import
was slower (675 ms desktop, 730 ms constrained container). Initializing fresh
workers took 29–33 ms desktop and 99–116 ms constrained. Warm encoded-input
throughput was roughly 17–26 MiB/s; export without image resizing was about
67–152 ms. Source download was recorded separately (23/76 ms), and caller buffer
preparation was excluded equally from copy/transfer cases. Both modes include
the required transfer into WASM memory. A transferred ArrayBuffer detaches from
the caller; it does not mean end-to-end zero-copy import.

The main-thread heartbeat continued while the worker imported/exported. Ten
model lifecycles retained the same WASM allocation and ended with zero handles.
The measurements do not claim allocator memory shrinks, that JS GC is immediate,
or that CPU throttling reproduces mobile memory bandwidth and thermal behaviour.
Background work was not fully isolated, so the small copy/transfer timing gap
should not be interpreted as a controlled speedup result.

```sh
python scripts/fetch_corpus.py
python scripts/build_web_package.py
PLAYWRIGHT_MODULE=/path/to/playwright node scripts/test_web_package.mjs
PLAYWRIGHT_MODULE=/path/to/playwright VITE_CLI=/path/to/vite/bin/vite.js node scripts/test_vite_package.mjs
PLAYWRIGHT_MODULE=/path/to/playwright node scripts/benchmark_web.mjs
```

Use `WEB_REPORT` to save JSON, `BROWSERS=chromium,firefox,webkit` for engine coverage,
and `BENCH_ENVIRONMENT` to label independently imposed container/device limits.
The benchmark's file/package hashes make results attributable to a real build.
Worker termination remains the fallback inside synchronous Rust operations.

Before increasing advertised model limits, repeat with permissioned customer
models and a physical constrained device. Include cold network/startup, decoded
images, JS copies, WASM high-water memory and browser process memory separately.
Native allocation gates live in [performance.md](performance.md); CI timing on a
shared host is advisory. Package size and startup should be measured for both
minimal and material-enabled builds before changing default features.
