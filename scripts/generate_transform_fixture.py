"""Regenerate original transform conformance data with usd-core 26.8."""
import json
from pathlib import Path
from pxr import Gf, Usd, UsdGeom

folder = Path(__file__).resolve().parents[1] / 'fixtures/models'
stage = Usd.Stage.CreateInMemory()
UsdGeom.SetStageMetersPerUnit(stage, .01)
UsdGeom.SetStageUpAxis(stage, 'Z')
assembly = UsdGeom.Xform.Define(stage, '/Assembly')
stage.SetDefaultPrim(assembly.GetPrim())
assembly.AddTranslateOp().Set(Gf.Vec3d(12, -5, 7))
assembly.AddRotateXYZOp().Set(Gf.Vec3f(17, 29, -13))
assembly.AddScaleOp().Set(Gf.Vec3f(-2, 3, .5))
for order in ['XYZ', 'XZY', 'YXZ', 'YZX', 'ZXY', 'ZYX', 'Reset']:
    node = UsdGeom.Xform.Define(stage, '/Assembly/' + order)
    if order == 'Reset':
        node.SetResetXformStack(True)
        node.AddTranslateOp().Set(Gf.Vec3d(1, 2, 3))
        node.AddOrientOp(UsdGeom.XformOp.PrecisionDouble).Set(Gf.Rotation(Gf.Vec3d(1, 2, 3), 47).GetQuat())
    else:
        node.AddTranslateOp(opSuffix='pivot').Set(Gf.Vec3d(1, 2, 3))
        getattr(node, 'AddRotate' + order + 'Op')().Set(Gf.Vec3f(21, -32, 43))
        node.AddTranslateOp(opSuffix='pivot', isInverseOp=True)
    mesh = UsdGeom.Mesh.Define(stage, str(node.GetPath()) + '/Triangle')
    mesh.CreatePointsAttr([(-1, 0, 0), (2, 0, 0), (0, 2, 0)])
    mesh.CreateFaceVertexCountsAttr([3])
    mesh.CreateFaceVertexIndicesAttr([0, 1, 2])
    mesh.CreateSubdivisionSchemeAttr('none')
    mesh.CreateNormalsAttr([(0, 0, 1)])
    mesh.SetNormalsInterpolation('constant')
cache = UsdGeom.XformCache()
reference = {'reference': 'OpenUSD 26.8', 'nodes': {}}
for prim in stage.Traverse():
    transform = UsdGeom.Xformable(prim)
    world = cache.GetLocalToWorldTransform(prim)
    local = transform.GetLocalTransformation()
    reference['nodes'][str(prim.GetPath())] = {
        'local': [[local[c][r] for c in range(4)] for r in range(4)],
        'world': [[world[c][r] for c in range(4)] for r in range(4)],
        'reset': transform.GetResetXformStack(),
    }
stage.GetRootLayer().Export(str(folder / 'transforms.usda'))
(folder / 'transforms-reference.json').write_text(json.dumps(reference, indent=2) + '\n')
