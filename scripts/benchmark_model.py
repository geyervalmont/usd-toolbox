"""Reproducible bounded synthetic mesh workload; timings are not capacity guarantees.

python scripts/benchmark_model.py --toolbox target/release/usd-toolbox --output /tmp/model-bench
Linux/POSIX runner using wait4 resource accounting. Writes generated input, artifacts, reports and measurements.
"""
import argparse
import json
from pathlib import Path
import os
import time

parser = argparse.ArgumentParser()
parser.add_argument('--toolbox', required=True)
parser.add_argument('--output', required=True)
parser.add_argument('--size', type=int, default=256)
args = parser.parse_args()
if not 2 <= args.size <= 512:
    parser.error('size must be 2..512')
root = Path(args.output).resolve()
root.mkdir(parents=True, exist_ok=True)
n = args.size
points = ','.join(f'({x/n},0,{z/n})' for z in range(n+1) for x in range(n+1))
indices = ','.join(str(i) for z in range(n) for x in range(n)
                   for i in [z*(n+1)+x, (z+1)*(n+1)+x, (z+1)*(n+1)+x+1, z*(n+1)+x+1])
(root/'grid.usda').write_text('#usda 1.0\n(metersPerUnit = 1\nupAxis = "Y")\n'
    'def Mesh "Grid" {\nuniform token subdivisionScheme = "none"\n'
    f'point3f[] points = [{points}]\nint[] faceVertexCounts = [{",".join(["4"]*(n*n))}]\n'
    f'int[] faceVertexIndices = [{indices}]\n}}\n')
commands = [
    ['pack', '--input', str(root/'grid.usda'), '--output', str(root/'grid.usdz'), '--report', str(root/'package.json')],
    ['export', '--input', str(root/'grid.usdz'), '--target', 'glb', '--output', str(root/'grid.glb'), '--report', str(root/'preview.json')],
]
results = {'grid_cells': n*n, 'points': (n+1)**2, 'toolbox': str(Path(args.toolbox).resolve()), 'measurements': []}
for command in commands:
    executable = str(Path(args.toolbox).resolve())
    start = time.monotonic()
    pid = os.posix_spawn(executable, [executable, 'model', *command], os.environ)
    _, status, usage = os.wait4(pid, 0)
    if os.waitstatus_to_exitcode(status) != 0:
        raise RuntimeError(f'{command[0]} failed with status {status}')
    results['measurements'].append({'operation': command[0], 'elapsed_seconds': round(time.monotonic()-start, 3), 'peak_rss_kib': usage.ru_maxrss})
results['artifacts'] = {name: (root/name).stat().st_size for name in ['grid.usda', 'grid.usdz', 'grid.glb']}
(root/'measurements.json').write_text(json.dumps(results, indent=2)+'\n')
print(json.dumps(results, indent=2))
