"""Validate the model fixture with OpenUSD, independently of neutral metadata."""
import sys
from pathlib import Path
from pxr import Usd, UsdGeom, UsdShade, UsdUtils

stage = Usd.Stage.Open(str(Path(sys.argv[1]).resolve()))
assert stage and stage.GetDefaultPrim()
assert not UsdUtils.ComputeAllDependencies(stage.GetRootLayer().identifier)[2], 'Unresolved dependency'
assert UsdGeom.GetStageMetersPerUnit(stage) == 0.001
assert UsdGeom.GetStageUpAxis(stage) == "Y"
meshes = [UsdGeom.Mesh(p) for p in stage.Traverse() if p.IsA(UsdGeom.Mesh)]
assert len(meshes) == 1
mesh = meshes[0]
assert mesh.GetSubdivisionSchemeAttr().Get() == "none"
assert list(mesh.GetFaceVertexCountsAttr().Get()) == [4, 4]
assert list(mesh.GetFaceVertexIndicesAttr().Get()) == [0, 1, 2, 3, 3, 2, 4, 5]
assert UsdGeom.Mesh.ValidateTopology(mesh.GetFaceVertexIndicesAttr().Get(), mesh.GetFaceVertexCountsAttr().Get(), len(mesh.GetPointsAttr().Get()))[0]
uv = UsdGeom.PrimvarsAPI(mesh).GetPrimvar("st")
assert uv.GetInterpolation() == "faceVarying"
assert list(uv.GetIndices()) == [0, 1, 2, 3, 0, 1, 2, 3]
binding = UsdShade.MaterialBindingAPI(mesh)
material, _ = binding.ComputeBoundMaterial()
assert material
subsets = binding.GetMaterialBindSubsets()
assert len(subsets) == 1
assert list(subsets[0].GetIndicesAttr().Get()) == [1]
subset_material, _ = UsdShade.MaterialBindingAPI(subsets[0]).ComputeBoundMaterial()
assert subset_material and subset_material.GetPath() != material.GetPath()
box = UsdGeom.BBoxCache(Usd.TimeCode.Default(), [UsdGeom.Tokens.default_, UsdGeom.Tokens.render], useExtentsHint=False).ComputeWorldBound(mesh.GetPrim()).ComputeAlignedRange()
assert tuple(box.GetMin()) == (750.0, 450.0, -250.0), box
assert tuple(box.GetMax()) == (1750.0, 450.0, 250.0), box
print("OpenUSD model conformance passed: geometry, units, transforms, UV seam and material subset")
