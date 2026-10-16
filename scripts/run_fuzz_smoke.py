#!/usr/bin/env python3
"""Run bounded, reproducible sanitizer fuzz smoke and retain every failure artifact."""
import argparse
import datetime
import json
import os
from pathlib import Path
import re
import shutil
import signal
import subprocess
import time

ROOT = Path(__file__).resolve().parents[1]
TARGETS = ('usda', 'usdc', 'usdz', 'materialx', 'gltf', 'images', 'metadata', 'query')


def run(command, log, timeout):
    started = time.monotonic()
    with log.open('w') as stream:
        environment = os.environ.copy()
        environment['ASAN_OPTIONS'] = 'quarantine_size_mb=32:malloc_context_size=10'
        process = subprocess.Popen(command, cwd=ROOT, stdout=stream, stderr=subprocess.STDOUT,
                                   start_new_session=True, env=environment)
        try:
            code = process.wait(timeout=timeout)
        except subprocess.TimeoutExpired:
            os.killpg(process.pid, signal.SIGKILL)
            process.wait()
            code = 124
    return code, round(time.monotonic() - started, 3)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--seconds', type=int, default=10, help='libFuzzer wall budget per target (1..600)')
    parser.add_argument('--targets', nargs='+', choices=TARGETS, default=TARGETS)
    parser.add_argument('--toolchain', default='nightly-2026-10-15')
    parser.add_argument('--seed', type=int, default=20261016)
    parser.add_argument('--output', type=Path, default=ROOT / 'target/fuzz-smoke')
    parser.add_argument('--skip-build', action='store_true')
    args = parser.parse_args()
    if not 1 <= args.seconds <= 600:
        parser.error('--seconds must be between 1 and 600')
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    prefix = ['cargo', '+' + args.toolchain, 'fuzz']
    if not args.skip_build:
        print('Building all sanitizer targets...', flush=True)
        code, _ = run(prefix + ['build', '--fuzz-dir', 'fuzz'], output / 'build.log', 900)
        if code:
            print((output / 'build.log').read_text()[-8000:])
            return code
    compiler = subprocess.check_output(['rustc', '+' + args.toolchain, '-vV'], text=True)
    host = next(line.split(': ', 1)[1] for line in compiler.splitlines() if line.startswith('host: '))
    binaries = ROOT / 'fuzz/target' / host / 'release'
    report = {'compiler': compiler, 'schema': 'usd-toolbox.fuzz-smoke.v1',
              'date_utc': datetime.datetime.now(datetime.timezone.utc).isoformat(),
              'toolchain': args.toolchain, 'seed': args.seed,
              'limits': {'input_bytes': 65536, 'per_input_seconds': 3,
                         'rss_mb': 512, 'single_allocation_mb': 128, 'asan_quarantine_mb': 32,
                         'seconds_per_target': args.seconds}, 'results': []}
    failed = False
    for target in args.targets:
        work = output / target
        corpus = work / 'corpus'
        artifacts = work / 'artifacts'
        corpus.mkdir(parents=True, exist_ok=True)
        artifacts.mkdir(parents=True, exist_ok=True)
        for seed in (ROOT / 'fuzz/seeds' / target).iterdir():
            if seed.is_file():
                shutil.copyfile(seed, corpus / seed.name)
        log = work / 'fuzz.log'
        command = [str(binaries / target), str(corpus),
                            f'-max_total_time={args.seconds}', '-timeout=3',
                            '-rss_limit_mb=512', '-malloc_limit_mb=128', '-max_len=65536',
                            f'-artifact_prefix={artifacts}/', '-print_final_stats=1',
                            f'-seed={args.seed}']
        print(f'Fuzzing {target} for up to {args.seconds} seconds...', flush=True)
        code, elapsed = run(command, log, args.seconds + 45)
        text = log.read_text(errors='replace')
        stats = {key: int(value) for key, value in re.findall(r'stat::([a-z_]+):\s+(\d+)', text)}
        result = {'target': target, 'exit_code': code, 'wall_seconds': elapsed,
                  'stats': stats, 'log': str(log.relative_to(output)),
                  'artifacts': [str(p.relative_to(output)) for p in sorted(artifacts.iterdir())]}
        report['results'].append(result)
        print(json.dumps(result), flush=True)
        if code:
            failed = True
            print(text[-6000:], flush=True)
        (output / 'report.json').write_text(json.dumps(report, indent=2) + '\n')
    return int(failed)


if __name__ == '__main__':
    raise SystemExit(main())
