#!/usr/bin/env python3
"""Repeatable Linux release benchmarks with allocation/RSS and deterministic gates.

python scripts/benchmark_queries.py --output /tmp/usd-performance
Timing warnings are advisory by default. --enforce-timing is for a controlled,
matching host, never an implicit performance promise for shared CI or browsers.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]
CASES = [("repeated", 10000), ("wide", 50000), ("deep", 10000),
         ("many_small", 5000), ("dense", 128), ("textured", 100), ("over_limit", 2)]

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--toolbox", type=Path, help="Optional freshly-built release CLI for USD stages")
    parser.add_argument("--baseline", type=Path)
    parser.add_argument("--enforce-timing", action="store_true")
    parser.add_argument("--skip-build", action="store_true")
    parser.add_argument("--quick", action="store_true")
    parser.add_argument("--cpu", type=int, help="Pin this runner and its children to one available CPU")
    args = parser.parse_args()
    if sys.platform != "linux":
        parser.error("RSS runner uses Linux wait4 units; Rust examples themselves are portable")
    if args.cpu is not None:
        os.sched_setaffinity(0, {args.cpu})
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    if not args.skip_build:
        subprocess.run(["cargo", "build", "--locked", "--release", "-p", "usd-toolbox-core", "--examples"], cwd=ROOT, check=True)
    executable = ROOT / "target/release/examples/document_benchmark"
    # A tiny freshly exec'd launcher avoids counting this Python runner's
    # inherited high-water RSS as part of a very short CLI process's usage.
    helper_directory = tempfile.TemporaryDirectory(prefix="usd-performance-rss-")
    helper = Path(helper_directory.name) / "rss-runner"
    source = helper.with_suffix(".c")
    source.write_text(r"""
#define _GNU_SOURCE
#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>
#include <sys/resource.h>
#include <sys/wait.h>
int main(int argc, char **argv) {
    if (argc < 3) return 125;
    pid_t child = fork();
    if (child < 0) { perror("fork"); return 125; }
    if (child == 0) { execvp(argv[2], argv + 2); perror("execvp"); _exit(127); }
    int status; struct rusage usage;
    if (wait4(child, &status, 0, &usage) < 0) { perror("wait4"); return 125; }
    FILE *output = fopen(argv[1], "w");
    if (!output) { perror("fopen"); return 125; }
    fprintf(output, "{\"peak_rss_kib\":%ld,\"user_seconds\":%.9f,\"system_seconds\":%.9f}\n",
        usage.ru_maxrss, usage.ru_utime.tv_sec + usage.ru_utime.tv_usec / 1000000.0,
        usage.ru_stime.tv_sec + usage.ru_stime.tv_usec / 1000000.0);
    fclose(output);
    return WIFEXITED(status) ? WEXITSTATUS(status) : 128 + WTERMSIG(status);
}
""")
    subprocess.run(["cc", "-O2", "-std=c11", str(source), "-o", str(helper)], check=True)
    cpu = next((line.split(":", 1)[1].strip() for line in Path("/proc/cpuinfo").read_text().splitlines() if line.startswith("model name")), "unknown")
    host = {"cpu": cpu, "platform": platform.platform(), "architecture": platform.machine(),
            "rustc": subprocess.check_output(["rustc", "--version"], text=True).strip(),
            "affinity": sorted(os.sched_getaffinity(0))}
    report = {"schema": "usd-toolbox.performance-report.v1", "host": host,
              "rss_method": "fresh C exec trampoline + child wait4; excludes Python parent high-water memory",
              "revision": subprocess.check_output(["git", "describe", "--always", "--dirty"], cwd=ROOT, text=True).strip(),
              "quick": args.quick, "executables": {"document_benchmark_sha256": hashlib.sha256(executable.read_bytes()).hexdigest(),
              "geometry_benchmark_sha256": hashlib.sha256((ROOT / "target/release/examples/geometry_benchmark").read_bytes()).hexdigest()}, "measurements": {}, "deterministic_checks": [], "timing_warnings": []}
    def run(label, command, parse_json=True):
        out = output / f"{label}.stdout"
        err = output / f"{label}.stderr"
        actions = [(os.POSIX_SPAWN_OPEN, 1, str(out), os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o644),
                   (os.POSIX_SPAWN_OPEN, 2, str(err), os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o644)]
        started = time.perf_counter()
        usage_file = output / f"{label}.usage.json"
        launch = [str(helper), str(usage_file), *[str(arg) for arg in command]]
        pid = os.posix_spawn(str(helper), launch, os.environ, file_actions=actions)
        _, status, _ = os.wait4(pid, 0)
        if os.waitstatus_to_exitcode(status):
            raise RuntimeError(f"{label} failed: {err.read_text()[-4000:]}")
        result = {"elapsed_ms": (time.perf_counter() - started) * 1000,
                  **json.loads(usage_file.read_text())}
        if parse_json:
            result["stages"] = json.loads(out.read_text())
        report["measurements"][label] = result
        return result
    samples = 100 if args.quick else 1000
    for case, size in CASES:
        if args.quick and case not in ("dense", "over_limit"):
            size = min(size, 1000)
        if args.quick and case == "dense":
            size = 32
        result = run(case, [executable, case, size, samples])["stages"]
        if case == "over_limit":
            assert result["rejected"] is True
            report["deterministic_checks"].append("over-limit nodes rejected")
            continue
        assert result["lookup"]["measurement"]["allocation_calls"] == 0, case
        assert result["bounds_warm"]["allocation_calls"] == 0, case
        assert result["payload_allocations_retained"], case
        assert result["repeated_query_live_growth_bytes"] == 0, case
        assert all(growth == 0 for growth in result["lifecycle"]["retained_growth_bytes"]), case
        # The changed metadata is two short strings. This budget catches accidental
        # whole-scene/header/index cloning independently of processor speed.
        assert result["metadata_edit"]["allocated_bytes"] <= 128 * 1024, case
        report["deterministic_checks"].append(f"{case}: zero-allocation lookup/warm bounds, <=128KiB metadata edit, retained payload pointers, zero lifecycle growth")
    geometry = run("geometry", [ROOT / "target/release/examples/geometry_benchmark", 32])["stages"]
    lod = next(stage["report"] for stage in geometry["stages"] if stage["report"]["operation"] == "simplify_mesh")
    assert lod["faces_after"] <= 1100 and lod["max_vertex_displacement"] <= 2.0
    report["deterministic_checks"].append("32x32 planar LOD: <=1100 faces and <=2 scene-unit original-vertex displacement")
    if args.toolbox:
        toolbox = args.toolbox.resolve()
        report["toolbox_sha256"] = hashlib.sha256(toolbox.read_bytes()).hexdigest()
        with tempfile.TemporaryDirectory(prefix="usd-performance-") as scratch:
            scratch = Path(scratch)
            for name in ("panels", "desk"):
                source = ROOT / f"fixtures/models/{name}.usda"
                usdz, glb = scratch / f"{name}.usdz", scratch / f"{name}.glb"
                stages = [("preflight", ["preflight", "--input", source, "--output", scratch / f"{name}-preflight.json"]),
                          ("import_inspect", ["inspect", "--input", source, "--output", scratch / f"{name}-inspect.json"]),
                          ("package", ["pack", "--input", source, "--output", usdz]),
                          ("export", ["export", "--input", usdz, "--target", "glb", "--output", glb])]
                for operation, command in stages:
                    run(f"{name}_{operation}", [toolbox, "model", *command], False)
                report["measurements"][f"{name}_artifacts"] = {"source_bytes": source.stat().st_size, "usdz_bytes": usdz.stat().st_size, "glb_bytes": glb.stat().st_size}
    if args.baseline:
        baseline = json.loads(args.baseline.read_text())
        same_host = all(host[key] == baseline["host"][key] for key in ("cpu", "platform", "architecture", "rustc", "affinity"))
        if args.enforce_timing and (not same_host or baseline["quick"] != args.quick):
            parser.error("enforced timing comparison requires matching CPU, architecture, rustc and workload mode")
        for case, measurement in report["measurements"].items():
            old = baseline["measurements"].get(case)
            if not old or "stages" not in old or case in ("geometry", "over_limit"):
                continue
            current_stages, old_stages = measurement["stages"], old["stages"]
            if current_stages.get("size") != old_stages.get("size"):
                continue
            for label, current, previous in [
                ("query p95 ns", current_stages["exact_id_query"]["p95_ns"], old_stages["exact_id_query"]["p95_ns"]),
                ("open ms", current_stages["open_and_index"]["milliseconds"], old_stages["open_and_index"]["milliseconds"]),
                ("metadata edit ms", current_stages["metadata_edit"]["milliseconds"], old_stages["metadata_edit"]["milliseconds"]),
                ("RSS KiB", measurement["peak_rss_kib"], old["peak_rss_kib"]),
            ]:
                if current > max(previous * 3, 1 if label.endswith("ms") else 1):
                    report["timing_warnings"].append(f"{case}: {label} grew from {previous:.3f} to {current:.3f} (>3x); repeat on idle matching host before diagnosing")
    (output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps({"report": str(output / "report.json"), "checks": len(report["deterministic_checks"]), "warnings": report["timing_warnings"]}, indent=2))
    if args.enforce_timing and report["timing_warnings"]:
        raise SystemExit(1)

if __name__ == "__main__":
    main()
