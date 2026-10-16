"""Compare exported standard USD xforms against the OpenUSD source oracle."""
import json
import sys
from pathlib import Path
from pxr import Usd, UsdGeom, Vt

stage = Usd.Stage.Open(str(Path(sys.argv[1]).resolve()))
assert stage, 'OpenUSD could not open the exported stage'
manifest = stage.GetRootLayer().customLayerData['olsyn_scene']
if isinstance(manifest, Vt.StringArray):
    manifest = ''.join(manifest)
assert isinstance(manifest, str), 'Scene manifest must be a string or string array'
metadata = json.loads(manifest)
oracle = json.loads((Path(__file__).resolve().parents[1] / 'fixtures/models/transforms-reference.json').read_text())
assert {node['id'] for node in metadata['nodes'].values()} == set(oracle['nodes']), 'Exported transform coverage differs from the source oracle'
cache = UsdGeom.XformCache()
for path, node in metadata['nodes'].items():
    expected = oracle['nodes'][node['id']]
    actual = cache.GetLocalToWorldTransform(stage.GetPrimAtPath(path))
    for row in range(4):
        for col in range(4):
            assert abs(actual[col][row] - expected['world'][row][col]) < 1e-10, (node['id'], row, col)
print('OpenUSD transform export conformance passed')
