"""Original static furniture fixture (project license); no third-party model data."""
from pathlib import Path
from pxr import Gf, Sdf, Usd, UsdGeom, UsdShade

root = Path(__file__).resolve().parents[1] / 'fixtures/models'
stage=Usd.Stage.CreateInMemory()
UsdGeom.SetStageMetersPerUnit(stage,1)
UsdGeom.SetStageUpAxis(stage,'Y')
desk=UsdGeom.Xform.Define(stage,'/Desk');stage.SetDefaultPrim(desk.GetPrim())
materials={}
for name,color,metal,roughness in [('Timber',(0.3,0.14,0.055),0,0.45),('Frame',(0.04,0.05,0.06),0.8,0.25)]:
    material=UsdShade.Material.Define(stage,'/Looks/'+name)
    shader=UsdShade.Shader.Define(stage,str(material.GetPath())+'/Surface')
    shader.CreateIdAttr('UsdPreviewSurface')
    shader.CreateInput('diffuseColor',Sdf.ValueTypeNames.Color3f).Set(Gf.Vec3f(*color))
    shader.CreateInput('metallic',Sdf.ValueTypeNames.Float).Set(metal)
    shader.CreateInput('roughness',Sdf.ValueTypeNames.Float).Set(roughness)
    material.CreateSurfaceOutput().ConnectToSource(shader.ConnectableAPI(),'surface')
    materials[name]=material
UsdGeom.Xform.Define(stage,'/Desk/Frame')
# Outward-facing quads on a unit cube, transformed into furniture parts.
points=[(-.5,-.5,-.5),(.5,-.5,-.5),(.5,.5,-.5),(-.5,.5,-.5),(-.5,-.5,.5),(.5,-.5,.5),(.5,.5,.5),(-.5,.5,.5)]
indices=[0,3,2,1,4,5,6,7,0,4,7,3,1,2,6,5,0,1,5,4,3,7,6,2]
def box(path,position,size,material):
    mesh=UsdGeom.Mesh.Define(stage,path)
    mesh.CreatePointsAttr(points);mesh.CreateFaceVertexCountsAttr([4]*6);mesh.CreateFaceVertexIndicesAttr(indices)
    mesh.CreateSubdivisionSchemeAttr('none')
    mesh.AddTranslateOp().Set(Gf.Vec3d(*position));mesh.AddScaleOp().Set(Gf.Vec3f(*size))
    uv=UsdGeom.PrimvarsAPI(mesh).CreatePrimvar('st',Sdf.ValueTypeNames.TexCoord2fArray,'faceVarying')
    uv.Set([(0,0),(0,1),(1,1),(1,0)]);uv.SetIndices([0,1,2,3]*6)
    UsdShade.MaterialBindingAPI.Apply(mesh.GetPrim()).Bind(materials[material])
box('/Desk/Top',(0,.735,0),(1.2,.03,.6),'Timber')
for x in [-.53,.53]:
    for z in [-.23,.23]:
        box('/Desk/Frame/Leg'+str(int(x>0))+str(int(z>0)),(x,.36,z),(.04,.72,.04),'Frame')
for z in [-.23,.23]:box('/Desk/Frame/Rail'+str(int(z>0)),(0,.68,z),(1.06,.06,.025),'Frame')
stage.GetRootLayer().Export(str(root/'desk.usda'))
