#!/usr/bin/env python3
"""Native adapter conformance; run with the optional usd-core Python runtime."""
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

from pxr import Gf, Sdf, Usd, UsdGeom, UsdShade, Vt

SPEC = importlib.util.spec_from_file_location("normalize_usd", Path(__file__).with_name("normalize_usd.py"))
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


def mesh(stage, path):
    prim = UsdGeom.Mesh.Define(stage, path)
    prim.CreatePointsAttr([(0, 0, 0), (1, 0, 0), (0, 1, 0)])
    prim.CreateFaceVertexCountsAttr([3])
    prim.CreateFaceVertexIndicesAttr([0, 1, 2])
    prim.CreateSubdivisionSchemeAttr("none")
    return prim


class NativeNormalization(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        part = Usd.Stage.CreateNew(str(self.root / "part.usda"))
        model = UsdGeom.Xform.Define(part, "/Part")
        mesh(part, "/Part/Mesh")
        part.SetDefaultPrim(model.GetPrim())
        part.GetRootLayer().Save()

    def tearDown(self):
        self.temp.cleanup()

    def run_adapter(self, source, *extra):
        staging = self.root / "normalized"
        staging.mkdir()
        arguments = [str(source), "--root", str(self.root), "--output", str(staging), "--time", "2", "--purpose", "render", "--payloads", "load", "--variants", "authored", *extra]
        adapter = MODULE.Adapter(MODULE.parser().parse_args(arguments))
        report = adapter.normalize(staging)
        return report, Usd.Stage.Open(str(staging / "normalized.usda"))

    def composed_source(self):
        base = Usd.Stage.CreateNew(str(self.root / "base.usda"))
        base.DefinePrim("/World", "Xform").CreateAttribute("layerStrength", Sdf.ValueTypeNames.Int, True).Set(1)
        base.GetRootLayer().Save()
        stage = Usd.Stage.CreateNew(str(self.root / "root.usda"))
        stage.GetRootLayer().subLayerPaths = ["base.usda"]
        world = stage.DefinePrim("/World", "Xform")
        world.CreateAttribute("layerStrength", Sdf.ValueTypeNames.Int, True).Set(2)
        stage.SetDefaultPrim(world)
        for name, x in [("A", 1), ("B", 3)]:
            placed = stage.DefinePrim("/World/" + name, "Xform")
            placed.GetReferences().AddReference("part.usda", "/Part")
            placed.SetInstanceable(True)
            UsdGeom.Xformable(placed).AddTranslateOp().Set((x, 0, 0))
        payload = stage.DefinePrim("/World/Payload", "Xform")
        payload.GetPayloads().AddPayload("part.usda", "/Part")
        variant = stage.DefinePrim("/World/Variant", "Xform")
        variant.GetReferences().AddReference("part.usda", "/Part")
        variants = variant.GetVariantSets().AddVariantSet("catalogue")
        for value in ["first", "second"]:
            variants.AddVariant(value)
            variants.SetVariantSelection(value)
            with variants.GetVariantEditContext():
                variant.CreateAttribute("selection", Sdf.ValueTypeNames.String, True).Set(value)
        animated = stage.DefinePrim("/World/Animated", "Xform")
        animated.GetReferences().AddReference("part.usda", "/Part")
        op = UsdGeom.Xformable(animated).AddTranslateOp()
        op.Set((0, 0, 0), 1)
        op.Set((10, 0, 0), 3)
        stage.GetRootLayer().Save()
        return self.root / "root.usda"

    def test_real_composition_strength_variants_payloads_time_and_native_instances(self):
        source = self.composed_source()
        report, output = self.run_adapter(source, "--variant", "/World/Variant:catalogue=first")
        self.assertTrue(report["succeeded"])
        self.assertEqual(output.GetPrimAtPath("/World").GetAttribute("layerStrength").Get(), 2)
        self.assertEqual(output.GetPrimAtPath("/World/Variant").GetAttribute("selection").Get(), "first")
        self.assertTrue(output.GetPrimAtPath("/World/Payload/Mesh"))
        self.assertEqual(UsdGeom.XformCache().GetLocalToWorldTransform(output.GetPrimAtPath("/World/Animated")).ExtractTranslation(), Gf.Vec3d(5, 0, 0))
        self.assertFalse(any(p.IsInstance() for p in output.Traverse()))
        self.assertEqual(report["shared_meshes"], 1)
        a, b = output.GetPrimAtPath("/World/A/Mesh"), output.GetPrimAtPath("/World/B/Mesh")
        self.assertEqual(a.GetCustomDataByKey("usd_toolbox_shared_mesh"), b.GetCustomDataByKey("usd_toolbox_shared_mesh"))
        self.assertEqual(UsdGeom.XformCache().GetLocalToWorldTransform(a).ExtractTranslation(), Gf.Vec3d(1, 0, 0))
        self.assertEqual(UsdGeom.XformCache().GetLocalToWorldTransform(b).ExtractTranslation(), Gf.Vec3d(3, 0, 0))
        self.assertEqual(len(report["sources"]), 3)
        binary = os.environ.get("USD_TOOLBOX_BIN")
        if binary:
            inspected = subprocess.run([binary, "model", "inspect", "--input", str(self.root / "normalized/normalized.usda")], capture_output=True, text=True)
            self.assertEqual(inspected.returncode, 0, inspected.stderr)
            result = json.loads(inspected.stdout)
            self.assertEqual(result["unique_meshes"], 1)
            self.assertEqual(result["mesh_occurrences"], 5)

    def test_point_instances_use_openusd_transforms_masks_and_shared_meshes(self):
        stage = Usd.Stage.CreateNew(str(self.root / "points.usda"))
        instancer = UsdGeom.PointInstancer.Define(stage, "/Scatter")
        prototype = mesh(stage, "/Scatter/Prototype")
        UsdGeom.Xformable(prototype).AddTranslateOp().Set((0, 2, 0))
        instancer.CreatePrototypesRel().SetTargets([prototype.GetPath()])
        instancer.CreateProtoIndicesAttr([0, 0, 0])
        instancer.CreatePositionsAttr([(1, 0, 0), (3, 0, 0), (9, 0, 0)])
        instancer.CreateIdsAttr([10, 20, 30])
        instancer.CreateInvisibleIdsAttr([30])
        stage.GetRootLayer().Save()
        report, output = self.run_adapter(self.root / "points.usda")
        self.assertEqual(report["shared_meshes"], 1)
        self.assertFalse(output.GetPrimAtPath("/Scatter/Prototype"))
        self.assertFalse(output.GetPrimAtPath("/Scatter/Instance_30"))
        a = output.GetPrimAtPath("/Scatter/Instance_10/Prototype")
        b = output.GetPrimAtPath("/Scatter/Instance_20/Prototype")
        self.assertEqual(UsdGeom.XformCache().GetLocalToWorldTransform(a).ExtractTranslation(), Gf.Vec3d(1, 2, 0))
        self.assertEqual(UsdGeom.XformCache().GetLocalToWorldTransform(b).ExtractTranslation(), Gf.Vec3d(3, 2, 0))

    def test_external_roots_and_unselected_variant_dependencies_are_confined(self):
        source = self.composed_source()
        with tempfile.TemporaryDirectory() as outside:
            external = Path(outside) / "outside.usda"
            external.write_text('#usda 1.0\ndef Xform "Outside" {}\n')
            layer = Sdf.Layer.FindOrOpen(str(source))
            layer.subLayerPaths.append(str(external))
            layer.Save()
            with self.assertRaises(MODULE.Rejected) as rejected:
                self.run_adapter(source)
            self.assertEqual(rejected.exception.code, "adapter.dependency_escape")

    def test_subdivision_is_rejected_and_omission_requires_explicit_policy(self):
        stage = Usd.Stage.CreateNew(str(self.root / "subdiv.usda"))
        shape = mesh(stage, "/Surface")
        shape.CreateSubdivisionSchemeAttr("catmullClark")
        stage.GetRootLayer().Save()
        with self.assertRaises(MODULE.Rejected) as rejected:
            self.run_adapter(self.root / "subdiv.usda")
        self.assertEqual(rejected.exception.code, "adapter.subdivision")

    def test_payload_none_and_required_variant_selection_are_reported(self):
        source = self.composed_source()
        report, output = self.run_adapter(source, "--payloads", "none")
        self.assertFalse(output.GetPrimAtPath("/World/Payload/Mesh"))
        self.assertTrue(any(i["code"] == "adapter.payload_unloaded" for i in report["issues"]))

    def test_authored_metadata_fixture_matches_openusd_strength_inheritance_and_interfaces(self):
        source = self.root / "metadata.usda"
        source.write_bytes((Path(__file__).parent.parent / "fixtures/models/metadata-semantics.usda").read_bytes())
        report, output = self.run_adapter(source)
        product = output.GetPrimAtPath("/Product")
        self.assertEqual(product.GetAttribute("product:serial").Get(), 9007199254740993)
        self.assertEqual(product.GetCustomDataByKey("productId"), 18446744073709551615)
        self.assertEqual(product.GetCustomDataByKey("negativeId"), -9223372036854775808)
        self.assertIn('uint64 productId = 18446744073709551615', output.GetRootLayer().ExportToString())
        self.assertIn('token classification = "Furniture.Chair"', output.GetRootLayer().ExportToString())
        surface = output.GetPrimAtPath("/Product/Surface")
        self.assertEqual(UsdGeom.PrimvarsAPI(surface).GetPrimvar("displayColor").Get(), Vt.Vec3fArray([Gf.Vec3f(.25, .5, .75)]))
        material, unused = UsdShade.MaterialBindingAPI(surface).ComputeBoundMaterial()
        shader, unused, unused = material.ComputeSurfaceSource()
        attributes = shader.GetInput("diffuseColor").GetValueProducingAttributes()
        self.assertEqual(attributes[0].Get(), Gf.Vec3f(.8, .2, .1))
        self.assertFalse(output.GetPrimAtPath("/Product/Disabled"))
        self.assertTrue(any(i["code"] == "adapter.layer_metadata_projection" for i in report["issues"]))

    def test_explicit_variants_require_each_visible_selection(self):
        with self.assertRaises(MODULE.Rejected) as rejected:
            self.run_adapter(self.composed_source(), "--variants", "explicit")
        self.assertEqual(rejected.exception.code, "adapter.variant_required")

    def test_composed_hierarchy_depth_is_bounded(self):
        stage = Usd.Stage.CreateNew(str(self.root / "deep.usda"))
        mesh(stage, "/A/B/C/D")
        stage.GetRootLayer().Save()
        with self.assertRaises(MODULE.Rejected) as rejected:
            self.run_adapter(self.root / "deep.usda", "--max-depth", "3")
        self.assertEqual(rejected.exception.code, "adapter.depth_limit")

    def test_inherited_primvars_prevent_incorrect_mesh_deduplication(self):
        stage = Usd.Stage.CreateNew(str(self.root / "colors.usda"))
        for name, color in [("Red", (1, 0, 0)), ("Blue", (0, 0, 1))]:
            parent = stage.DefinePrim("/" + name, "Xform")
            UsdGeom.PrimvarsAPI(parent).CreatePrimvar("displayColor", Sdf.ValueTypeNames.Color3fArray, "constant").Set([color])
            mesh(stage, "/" + name + "/Mesh")
        stage.GetRootLayer().Save()
        report, output = self.run_adapter(self.root / "colors.usda")
        self.assertEqual(report["shared_meshes"], 2)
        self.assertNotEqual(output.GetPrimAtPath("/Red/Mesh").GetCustomDataByKey("usd_toolbox_shared_mesh"), output.GetPrimAtPath("/Blue/Mesh").GetCustomDataByKey("usd_toolbox_shared_mesh"))

    def test_reserved_material_scope_collision_preserves_source_geometry(self):
        stage = Usd.Stage.CreateNew(str(self.root / "collision.usda"))
        mesh(stage, "/__ToolboxLooks")
        stage.GetRootLayer().Save()
        report, output = self.run_adapter(self.root / "collision.usda")
        self.assertEqual(output.GetPrimAtPath("/__ToolboxLooks").GetTypeName(), "Mesh")
        self.assertEqual(output.GetPrimAtPath("/__ToolboxLooks_").GetTypeName(), "Scope")


if __name__ == "__main__":
    unittest.main()
