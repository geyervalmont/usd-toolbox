#!/usr/bin/env python3
"""Optional native OpenUSD preprocessing; never imported by Rust or web builds.

Compose with OpenUSD opinion strength, select a declared snapshot, and author a
self-contained static polygon layer plus confined texture sidecars. This is a
host adapter, not a portable composition implementation or a tessellator.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import re
import shutil
import sys
import tempfile

from pxr import Gf, Sdf, Usd, UsdGeom, UsdShade, Vt

SCHEMA = "usd-toolbox.native-normalization.v1"
SUPPORTED = {"", "Xform", "Scope", "Mesh", "GeomSubset", "Material", "Shader", "NodeGraph", "PointInstancer"}
MESH_ATTRIBUTES = {"points", "faceVertexCounts", "faceVertexIndices", "normals", "orientation", "doubleSided", "subdivisionScheme", "subsetFamily:materialBind:familyType"}
SOURCE_METADATA = {"assetInfo", "customData", "kind", "documentation", "displayName", "hidden"}


class Rejected(Exception):
    def __init__(self, code, path, detail):
        self.code, self.path, self.detail = code, str(path), detail
        super().__init__(f"{code} at {path}: {detail}")


def sha256(path):
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


class Adapter:
    def __init__(self, options):
        self.options = options
        self.root = Path(options.root).resolve(strict=True)
        self.source = self.confined(Path(options.input), self.root)
        self.files = {}
        self.layers = {}
        self.issues = []
        self.specs = 0
        self.nodes = 0
        self.points = 0
        self.corners = 0
        self.mesh_hashes = set()
        self.assets = {}
        self.materials = {}
        self.time = Usd.TimeCode(options.time)
        self.report = {"schema": SCHEMA, "adapter_version": 1, "openusd_version": ".".join(map(str, Usd.GetVersion())), "portable_core_requirement": False, "source": str(self.source.relative_to(self.root)), "policies": {"time": options.time, "purpose": options.purpose, "payloads": options.payloads, "material_purpose": options.material_purpose, "render_context": options.render_context, "variants": options.variants, "unsupported": options.unsupported, "subdivision": "reject; no tessellator is bundled"}, "issues": self.issues, "checks": {"performed": [], "deferred": ["image decoding", "renderer equivalence", "arbitrary shader evaluation"]}}

    def confined(self, name, base):
        name = str(name)
        if ":" in name or "[" in name or "]" in name or "\x00" in name:
            raise Rejected("adapter.resolver", name, "URI, resolver schemes and package paths are disabled")
        path = (base / name).resolve(strict=True)
        if not path.is_relative_to(self.root) or not path.is_file():
            raise Rejected("adapter.dependency_escape", name, "dependency must be a regular file inside the explicit root")
        return path

    def record(self, path):
        if path in self.files:
            return
        size = path.stat().st_size
        if len(self.files) >= self.options.max_files or size + sum(f["bytes"] for f in self.files.values()) > self.options.max_bytes:
            raise Rejected("adapter.input_limit", path, "file count or aggregate source-byte limit reached")
        self.files[path] = {"path": str(path.relative_to(self.root)), "bytes": size, "sha256": sha256(path)}

    def issue(self, code, path, detail, severity="warning"):
        if len(self.issues) >= self.options.max_issues:
            raise Rejected("adapter.diagnostic_limit", path, "diagnostic count exhausted; conversion stopped")
        self.issues.append({"code": code, "path": str(path), "severity": severity, "detail": detail})

    def unsupported(self, code, path, detail):
        if self.options.unsupported == "reject":
            raise Rejected(code, path, detail)
        self.issue(code, path, detail + "; omitted by explicit policy")

    def scan_layer(self, path, visiting=()):
        if path in visiting:
            raise Rejected("adapter.dependency_cycle", path, "cyclic layer dependencies are not accepted")
        if path in self.layers:
            return
        if len(visiting) >= self.options.max_depth:
            raise Rejected("adapter.depth_limit", path, "layer dependency depth exhausted")
        self.record(path)
        if path.suffix.lower() not in {".usd", ".usda", ".usdc"}:
            raise Rejected("adapter.layer_format", path, "native adapter accepts individual USD layers, not packages")
        layer = Sdf.Layer.FindOrOpen(str(path))
        if not layer:
            raise Rejected("adapter.decode", path, "OpenUSD could not open the layer")
        self.layers[path] = layer
        # Opening Sdf layers does not compose references. Every possible external
        # layer (including unselected variants/payloads) is confined before Stage.Open.
        for reference in layer.GetExternalReferences():
            if reference:
                dependency = self.confined(reference, path.parent)
                self.scan_layer(dependency, (*visiting, path))
        def inspect(spec_path):
            self.specs += 1
            if self.specs > self.options.max_specs:
                raise Rejected("adapter.spec_limit", spec_path, "authored spec count limit reached")
            spec = layer.GetObjectAtPath(spec_path)
            if isinstance(spec, Sdf.AttributeSpec) and spec.typeName in {Sdf.ValueTypeNames.Asset, Sdf.ValueTypeNames.AssetArray}:
                values = [spec.default]
                samples = layer.ListTimeSamplesForPath(spec_path)
                if len(samples) + self.specs > self.options.max_specs:
                    raise Rejected("adapter.sample_limit", spec_path, "asset sample scan limit reached")
                self.specs += len(samples)
                values.extend(layer.QueryTimeSample(spec_path, time) for time in samples)
                for value in values:
                    assets = [value] if isinstance(value, Sdf.AssetPath) else (value or [])
                    for asset in assets:
                        if isinstance(asset, Sdf.AssetPath) and asset.path:
                            self.record(self.confined(asset.path, path.parent))
        layer.Traverse(Sdf.Path.absoluteRootPath, inspect)

    def asset(self, value, attribute):
        if isinstance(value, Sdf.AssetPath):
            if not value.path:
                return value
            stack = attribute.GetPropertyStack(self.time)
            base = Path(stack[0].layer.realPath).parent if stack and stack[0].layer.realPath else self.source.parent
            source = self.confined(value.resolvedPath or value.path, base)
            self.record(source)
            digest = self.files[source]["sha256"]
            safe = re.sub(r"[^A-Za-z0-9._-]", "_", source.name)
            target = f"assets/{digest}/{safe}"
            if target not in self.assets:
                destination = self.staging / target
                destination.parent.mkdir(parents=True, exist_ok=True)
                shutil.copyfile(source, destination)
                if sha256(destination) != digest:
                    raise Rejected("adapter.source_changed", source, "dependency changed during conversion")
                self.assets[target] = self.files[source]
            return Sdf.AssetPath(target)
        if isinstance(value, Sdf.AssetPathArray):
            return Sdf.AssetPathArray([self.asset(v, attribute) for v in value])
        return value

    def flat_path(self, path):
        for _ in range(self.options.max_depth):
            if self.flattened.GetObjectAtPath(path):
                return path
            ancestor = path.GetPrimPath()
            while ancestor != Sdf.Path.absoluteRootPath:
                spec = self.flattened.GetPrimAtPath(ancestor)
                references = spec.referenceList.GetAddedOrExplicitItems() if spec else []
                if references and not references[0].assetPath:
                    path = path.ReplacePrefix(ancestor, references[0].primPath)
                    break
                ancestor = ancestor.GetParentPath()
            else:
                raise Rejected("adapter.metadata_source", path, "flattened source spec is unavailable")
        raise Rejected("adapter.depth_limit", path, "prototype metadata indirection exhausted")

    def copy_metadata(self, source, target, keys=SOURCE_METADATA):
        # Copy Vt metadata inside USD. Python dictionaries erase nested token vs
        # string and int64/uint64 widths; Sdf.CopySpec preserves those exact types.
        source_path = self.flat_path(source.GetPath())
        Sdf.CopySpec(self.flattened, source_path, self.output.GetRootLayer(), target.GetPath(),
                     lambda spec, field, *unused: str(field) in keys,
                     lambda *unused: False)

    def copy_attribute(self, attr, target, connection_map=None):
        if not attr.HasAuthoredValueOpinion() and not attr.HasAuthoredConnections():
            return
        output = target.CreateAttribute(attr.GetName(), attr.GetTypeName(), attr.IsCustom(), attr.GetVariability())
        value = attr.Get(self.time)
        if value is not None:
            output.Set(self.asset(value, attr))
        elif attr.HasAuthoredValueOpinion():
            output.Block()
        if attr.GetNumTimeSamples():
            self.issue("adapter.time_sampled", attr.GetPath(), f"evaluated at time {self.options.time}")
        self.copy_metadata(attr, output, {"customData", "documentation", "displayName", "displayGroup", "hidden", "interpolation", "elementSize", "unauthoredValuesIndex"})
        if attr.HasAuthoredConnections():
            connections = attr.GetConnections()
            if connection_map:
                old, new = connection_map
                if any(not path.HasPrefix(old) for path in connections):
                    raise Rejected("adapter.material_external_graph", attr.GetPath(), "shader connection leaves the packaged material subtree")
                connections = [p.ReplacePrefix(old, new) if p.HasPrefix(old) else p for p in connections]
            output.SetConnections(connections)

    def material(self, material):
        path = material.GetPath()
        if str(path) in self.materials:
            return self.materials[str(path)]
        # A stable path avoids collisions with copied geometry and prototypes.
        dest = self.looks.AppendChild("M_" + hashlib.sha256(str(path).encode()).hexdigest()[:20])
        self.materials[str(path)] = dest
        for prim in Usd.PrimRange(material.GetPrim(), Usd.TraverseInstanceProxies()):
            if prim.GetTypeName() not in {"Material", "Shader", "NodeGraph", "Scope", ""}:
                self.unsupported("adapter.material_prim", prim.GetPath(), f"unsupported shading prim {prim.GetTypeName()}")
                continue
            output_path = prim.GetPath().ReplacePrefix(path, dest)
            self.count_node(output_path)
            output = self.output.DefinePrim(output_path, prim.GetTypeName())
            self.copy_metadata(prim, output)
            for attr in prim.GetAttributes():
                self.copy_attribute(attr, output, (path, dest))
        surface, _, _ = material.ComputeSurfaceSource("" if self.options.render_context == "universal" else self.options.render_context)
        if not surface:
            raise Rejected("adapter.material_surface", path, "selected render context has no resolved surface shader")
        if surface.GetIdAttr().Get(self.time) != "UsdPreviewSurface":
            raise Rejected("adapter.material_shader", surface.GetPath(), "optional adapter does not evaluate or bake non-PreviewSurface shaders")
        shader_path = surface.GetPath().ReplacePrefix(path, dest)
        if not surface.GetPath().HasPrefix(path):
            raise Rejected("adapter.material_external_graph", path, "surface shader outside material subtree needs explicit graph packaging")
        UsdShade.Material(self.output.GetPrimAtPath(dest)).CreateSurfaceOutput().ConnectToSource(UsdShade.Shader(self.output.GetPrimAtPath(shader_path)).ConnectableAPI(), "surface")
        return dest

    def bind(self, source, output):
        material, _ = UsdShade.MaterialBindingAPI(source).ComputeBoundMaterial("" if self.options.material_purpose == "all" else self.options.material_purpose)
        if material:
            target = self.material(material)
            UsdShade.MaterialBindingAPI.Apply(output).Bind(UsdShade.Material(self.output.GetPrimAtPath(target)))

    def check_depth(self, path):
        if path.pathElementCount > self.options.max_depth:
            raise Rejected("adapter.depth_limit", path, "prim hierarchy depth exhausted")

    def count_node(self, path):
        self.check_depth(path)
        self.nodes += 1
        if self.nodes > self.options.max_nodes:
            raise Rejected("adapter.node_limit", path, "projected node budget reached")

    def define(self, source, destination, local=None):
        self.count_node(destination)
        kind = source.GetTypeName()
        for schema in source.GetAppliedSchemas():
            if schema != "MaterialBindingAPI":
                self.unsupported("adapter.schema_projection", source.GetPath(), f"applied schema {schema} has no evaluator in this adapter")
        for relationship in source.GetRelationships():
            name = relationship.GetName()
            if relationship.HasAuthoredTargets() and not name.startswith(("material:binding", "collection:")) and not (kind == "PointInstancer" and name == "prototypes"):
                self.unsupported("adapter.relationship_loss", relationship.GetPath(), "relationship is outside the selected static geometry/material profile")
        if kind == "PointInstancer":
            kind = "Xform"
        target = self.output.DefinePrim(destination, kind or "Scope")
        self.copy_metadata(source, target)
        if source.IsInstance() or source.IsInstanceProxy():
            target.SetCustomDataByKey("usd_toolbox_instance_source", str(source.GetPrimInPrototype().GetPath()) if source.IsInstanceProxy() else str(source.GetPrototype().GetPath()))
        if kind in {"Xform", "Mesh"}:
            transform = local
            reset = False
            if transform is None:
                transform, reset = self.cache.GetLocalTransformation(source)
            UsdGeom.Xformable(target).AddTransformOp().Set(transform)
            UsdGeom.Xformable(target).SetResetXformStack(reset)
            imageable = UsdGeom.Imageable(source)
            if imageable:
                UsdGeom.Imageable(target).CreateVisibilityAttr().Set(imageable.ComputeVisibility(self.time))
                UsdGeom.Imageable(target).CreatePurposeAttr().Set("default")
        if kind == "Mesh":
            mesh = UsdGeom.Mesh(source)
            if mesh.GetSubdivisionSchemeAttr().Get(self.time) != "none":
                raise Rejected("adapter.subdivision", source.GetPath(), "subdivision requires a real tessellation evaluator; control cages are not emitted as polygon equivalents")
            for name in ["holeIndices", "creaseIndices", "cornerIndices"]:
                if source.GetAttribute(name).Get(self.time):
                    raise Rejected("adapter.mesh_feature", source.GetPath(), f"{name} requires explicit tessellation")
            self.points += len(mesh.GetPointsAttr().Get(self.time) or [])
            self.corners += len(mesh.GetFaceVertexIndicesAttr().Get(self.time) or [])
            if self.points > self.options.max_points or self.corners > self.options.max_corners:
                raise Rejected("adapter.geometry_limit", source.GetPath(), "expanded geometry exceeds budget")
        for attr in source.GetAttributes():
            name = attr.GetName()
            if name.startswith("xformOp") or name in {"visibility", "purpose", "extent"}:
                continue
            if source.GetTypeName() == "PointInstancer" and not attr.IsCustom():
                continue
            if kind == "Mesh" and (name in MESH_ATTRIBUTES or name.startswith("primvars:")):
                self.copy_attribute(attr, target)
            elif kind == "GeomSubset" and name in {"elementType", "familyName", "indices"}:
                self.copy_attribute(attr, target)
            elif attr.IsCustom():
                self.copy_attribute(attr, target)
            elif name.startswith("primvars:"):
                self.copy_attribute(attr, target)
            elif attr.HasAuthoredValueOpinion() and name not in {"holeIndices", "creaseIndices", "creaseLengths", "creaseSharpnesses", "cornerIndices", "cornerSharpnesses", "interpolateBoundary", "faceVaryingLinearInterpolation", "triangleSubdivisionRule"}:
                self.unsupported("adapter.attribute_loss", attr.GetPath(), "attribute is outside the selected polygon profile")
        if kind == "Mesh":
            # Resolve inherited primvars through OpenUSD before deduplicating.
            # Equal authored geometry can have different effective ancestor data.
            for primvar in UsdGeom.PrimvarsAPI(source).FindPrimvarsWithInheritance():
                if primvar.GetAttr().GetPrim() != source:
                    self.copy_attribute(primvar.GetAttr(), target)
                    indices = primvar.GetIndicesAttr()
                    if indices:
                        self.copy_attribute(indices, target)
        self.bind(source, target)
        if kind == "Mesh":
            # Hash only geometry; placements and bindings remain separate nodes.
            attributes = [(a.GetName(), str(a.GetTypeName()), repr(a.Get()), repr(a.GetAllAuthoredMetadata())) for a in target.GetAttributes() if a.GetName() in MESH_ATTRIBUTES or a.GetName().startswith("primvars:")]
            digest = hashlib.sha256(repr(sorted(attributes)).encode()).hexdigest()
            target.SetCustomDataByKey("usd_toolbox_shared_mesh", "mesh_" + digest)
            self.mesh_hashes.add(digest)
        return target

    def purpose_matches(self, prim):
        imageable = UsdGeom.Imageable(prim)
        return not imageable or imageable.ComputePurpose() in {"default", self.options.purpose}

    def point_instancer(self, prim):
        instancer = UsdGeom.PointInstancer(prim)
        prototypes = instancer.GetPrototypesRel().GetTargets()
        indices = instancer.GetProtoIndicesAttr().Get(self.time) or []
        if len(indices) + self.nodes > self.options.max_nodes:
            raise Rejected("adapter.instance_limit", prim.GetPath(), "instance placements exceed node budget before expansion")
        ids = instancer.GetIdsAttr().Get(self.time)
        if ids is not None and (len(ids) != len(indices) or len(set(ids)) != len(ids)):
            raise Rejected("adapter.instance_ids", prim.GetPath(), "ids must be unique and match protoIndices")
        transforms = instancer.ComputeInstanceTransformsAtTime(self.time, self.time, UsdGeom.PointInstancer.IncludeProtoXform, UsdGeom.PointInstancer.IgnoreMask)
        mask = instancer.ComputeMaskAtTime(self.time)
        if mask and len(mask) != len(indices):
            raise Rejected("adapter.instance_mask", prim.GetPath(), "OpenUSD returned a mask with inconsistent cardinality")
        if len(transforms) != len(indices):
            raise Rejected("adapter.instance_transforms", prim.GetPath(), "OpenUSD did not produce all instance transforms")
        for index, prototype in enumerate(indices):
            if mask and not mask[index]:
                continue
            if prototype < 0 or prototype >= len(prototypes):
                raise Rejected("adapter.prototype_index", prim.GetPath(), "invalid prototype index")
            root = self.stage.GetPrimAtPath(prototypes[prototype])
            if not root:
                raise Rejected("adapter.prototype_missing", prim.GetPath(), "unresolved prototype")
            stable_id = str(ids[index] if ids is not None else index).replace("-", "neg_")
            placement_path = prim.GetPath().AppendChild("Instance_" + stable_id)
            self.count_node(placement_path)
            placement = self.output.DefinePrim(placement_path, "Xform")
            UsdGeom.Xformable(placement).AddTransformOp().Set(transforms[index])
            placement.SetCustomDataByKey("usd_toolbox_point_instance_id", stable_id)
            for child in Usd.PrimRange(root, Usd.TraverseInstanceProxies()):
                if child.GetTypeName() == "PointInstancer":
                    raise Rejected("adapter.nested_point_instancer", child.GetPath(), "nested point instancers require a separate bounded conversion")
                if child.GetTypeName() not in SUPPORTED:
                    self.unsupported("adapter.prim_unsupported", child.GetPath(), child.GetTypeName())
                    continue
                if child.GetTypeName() in {"Material", "Shader", "NodeGraph"} or not self.purpose_matches(child):
                    continue
                destination = child.GetPath().ReplacePrefix(root.GetPath(), placement_path.AppendChild("Prototype"))
                local = Gf.Matrix4d(1) if child == root else self.cache.GetLocalToWorldTransform(child) * self.cache.GetLocalToWorldTransform(child.GetParent()).GetInverse()
                self.define(child, destination, local)
        self.issue("adapter.point_instances", prim.GetPath(), f"{len(indices)} evaluated placements authored; equal mesh payloads have shared identities")

    def normalize(self, staging):
        self.staging = staging
        self.scan_layer(self.source)
        self.report["checks"]["performed"].append("pre-composition layer and asset confinement, source hashes and aggregate budgets")
        self.stage = Usd.Stage.Open(str(self.source), load=Usd.Stage.LoadAll if self.options.payloads == "load" else Usd.Stage.LoadNone)
        if not self.stage:
            raise Rejected("adapter.stage", self.source, "OpenUSD could not compose the stage")
        self.stage.SetEditTarget(self.stage.GetSessionLayer())
        variants = {}
        for selection in self.options.variant:
            try:
                location, value = selection.rsplit("=", 1)
                prim_path, set_name = location.rsplit(":", 1)
            except ValueError as error:
                raise Rejected("adapter.variant_argument", selection, "expected /Prim:set=selection") from error
            prim = self.stage.GetPrimAtPath(prim_path)
            variant_set = prim.GetVariantSet(set_name)
            if not prim or value not in variant_set.GetVariantNames():
                raise Rejected("adapter.variant", selection, "variant set or selection is unavailable")
            variant_set.SetVariantSelection(value)
            variants[(prim_path, set_name)] = value
        selected_variants = []
        for index, prim in enumerate(self.stage.TraverseAll()):
            if index >= self.options.max_nodes:
                raise Rejected("adapter.node_limit", prim.GetPath(), "composed prim count exceeds node budget")
            self.check_depth(prim.GetPath())
            for set_name in prim.GetVariantSets().GetNames():
                selected = prim.GetVariantSet(set_name).GetVariantSelection()
                if self.options.variants == "explicit" and (str(prim.GetPath()), set_name) not in variants:
                    raise Rejected("adapter.variant_required", prim.GetPath(), f"explicit --variant required for {set_name}")
                selected_variants.append({"path": str(prim.GetPath()), "set": set_name, "selection": selected})
            if prim.HasPayload() and not prim.IsLoaded():
                self.issue("adapter.payload_unloaded", prim.GetPath(), "payload deliberately omitted by --payloads=none")
            if not prim.IsActive():
                self.issue("adapter.inactive_omitted", prim.GetPath(), "inactive authored prim omitted")
        self.report["variants"] = selected_variants
        primitives = []
        for prim in Usd.PrimRange.Stage(self.stage, Usd.TraverseInstanceProxies()):
            if len(primitives) >= self.options.max_nodes:
                raise Rejected("adapter.node_limit", prim.GetPath(), "expanded composed prim count exceeds node budget")
            self.check_depth(prim.GetPath())
            primitives.append(prim)
        self.flattened = self.stage.Flatten(addSourceFileComment=False)
        self.cache = UsdGeom.XformCache(self.time)
        self.output = Usd.Stage.CreateInMemory()
        UsdGeom.SetStageMetersPerUnit(self.output, UsdGeom.GetStageMetersPerUnit(self.stage))
        UsdGeom.SetStageUpAxis(self.output, UsdGeom.GetStageUpAxis(self.stage))
        self.looks = Sdf.Path("/__ToolboxLooks")
        while self.stage.GetPrimAtPath(self.looks):
            self.looks = Sdf.Path(str(self.looks) + "_")
        self.count_node(self.looks)
        self.output.DefinePrim(self.looks, "Scope")
        self.report["source_layer_metadata"] = []
        for path, layer in self.layers.items():
            fields = sorted(str(field) for field in layer.pseudoRoot.ListInfoKeys())
            self.report["source_layer_metadata"].append({"path": str(path.relative_to(self.root)), "fields": fields})
            projected = {"metersPerUnit", "upAxis", "defaultPrim", "primChildren", "subLayers", "subLayerOffsets"}
            omitted = [field for field in fields if field not in projected]
            if omitted:
                self.issue("adapter.layer_metadata_projection", path.relative_to(self.root), "source layer metadata is inventoried with source hashes but not authored into the static layer: " + ", ".join(omitted))
        internal_prototypes = []
        for prim in primitives:
            if prim.GetTypeName() == "PointInstancer":
                internal_prototypes.extend(path for path in UsdGeom.PointInstancer(prim).GetPrototypesRel().GetTargets() if path.HasPrefix(prim.GetPath()))
        for prim in primitives:
            path, kind = prim.GetPath(), prim.GetTypeName()
            if any(path.HasPrefix(root) for root in internal_prototypes):
                continue
            if kind in {"Material", "Shader", "NodeGraph"} or any(parent.GetTypeName() == "Material" for parent in self.ancestors(prim)):
                continue
            if kind not in SUPPORTED:
                self.unsupported("adapter.prim_unsupported", path, kind)
                continue
            if kind in {"Mesh", "PointInstancer"} and not self.purpose_matches(prim):
                self.issue("adapter.purpose_omitted", path, f"geometry excluded by purpose {self.options.purpose}")
                continue
            self.define(prim, path)
            if kind == "PointInstancer":
                self.point_instancer(prim)
        default = self.stage.GetDefaultPrim()
        if default and self.output.GetPrimAtPath(default.GetPath()):
            self.output.SetDefaultPrim(self.output.GetPrimAtPath(default.GetPath()))
        files = sorted(self.files.values(), key=lambda f: f["path"])
        source_digest = hashlib.sha256(json.dumps(files, sort_keys=True, separators=(",", ":")).encode()).hexdigest()
        self.output.GetRootLayer().customLayerData = {"usd_toolbox_adapter": {"schema": SCHEMA, "openusd": self.report["openusd_version"], "sourceDigest": source_digest, "time": self.options.time, "purpose": self.options.purpose, "payloads": self.options.payloads}}
        self.report.update({"sources": files, "source_digest": source_digest, "shared_meshes": len(self.mesh_hashes), "nodes": self.nodes, "expanded_points": self.points, "expanded_corners": self.corners, "assets": sorted(self.assets)})
        self.report["checks"]["performed"].extend(["OpenUSD stage composition and selected variants/payloads", "static time evaluation", "render-purpose material binding resolution", "native and point instance placements", "shared geometry identity assignment"])
        output = staging / "normalized.usda"
        self.output.GetRootLayer().Export(str(output))
        if sum(p.stat().st_size for p in staging.rglob("*") if p.is_file()) > self.options.max_bytes:
            raise Rejected("adapter.output_limit", output, "output byte limit reached")
        # Inputs must remain the same bytes as the preflight snapshot.
        for path, description in self.files.items():
            if path.stat().st_size != description["bytes"] or sha256(path) != description["sha256"]:
                raise Rejected("adapter.source_changed", path, "source changed while conversion was running")
        self.report["output"] = {"path": "normalized.usda", "bytes": output.stat().st_size, "sha256": sha256(output)}
        self.report["succeeded"] = True
        (staging / "normalization-report.json").write_text(json.dumps(self.report, ensure_ascii=False, indent=2) + "\n")
        return self.report

    @staticmethod
    def ancestors(prim):
        parent = prim.GetParent()
        while parent and not parent.IsPseudoRoot():
            yield parent
            parent = parent.GetParent()


def parser():
    command = argparse.ArgumentParser(description=__doc__)
    command.add_argument("input")
    command.add_argument("--root", required=True, help="explicit dependency confinement root")
    command.add_argument("--output", required=True, help="new output directory; existing paths are refused")
    command.add_argument("--time", required=True, type=float)
    command.add_argument("--purpose", required=True, choices=["default", "render", "proxy"])
    command.add_argument("--payloads", required=True, choices=["load", "none"])
    command.add_argument("--variants", required=True, choices=["authored", "explicit"])
    command.add_argument("--variant", action="append", default=[])
    command.add_argument("--render-context", default="universal")
    command.add_argument("--material-purpose", choices=["all", "full", "preview"], default="all")
    command.add_argument("--unsupported", choices=["reject", "omit"], default="reject")
    for name, value in [("files", 10_000), ("bytes", 512 * 1024 * 1024), ("depth", 64), ("specs", 1_000_000), ("nodes", 100_000), ("points", 5_000_000), ("corners", 15_000_000), ("issues", 1000)]:
        command.add_argument("--max-" + name, type=int, default=value)
    return command


def main():
    options = parser().parse_args()
    if not math.isfinite(options.time) or any(value <= 0 for name, value in vars(options).items() if name.startswith("max_")):
        raise SystemExit("time must be finite and all limits must be positive")
    output = Path(options.output).resolve()
    if output.exists():
        raise SystemExit("output already exists; choose a new directory")
    output.parent.mkdir(parents=True, exist_ok=True)
    staging = Path(tempfile.mkdtemp(prefix=".usd-normalize-", dir=output.parent))
    adapter = None
    try:
        adapter = Adapter(options)
        report = adapter.normalize(staging)
        os.rename(staging, output)
        print(json.dumps(report, ensure_ascii=False))
    except Exception as error:
        report = adapter.report if adapter else {"schema": SCHEMA, "issues": []}
        report["succeeded"] = False
        report["issues"].append({"code": getattr(error, "code", "adapter.failure"), "path": getattr(error, "path", str(options.input)), "severity": "error", "detail": getattr(error, "detail", str(error))})
        print(json.dumps(report, ensure_ascii=False))
        return 2
    finally:
        if staging.exists():
            shutil.rmtree(staging)
    return 0


if __name__ == "__main__":
    sys.exit(main())
