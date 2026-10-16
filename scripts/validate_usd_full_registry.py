#!/usr/bin/env python3
"""Fail-closed OpenUSD validation with actual MaterialX Sdr plugins loaded.

Run in a separate full OpenUSD environment; usd-core alone is insufficient.
No validator findings or missing shader identifiers are exempted.
"""
from __future__ import annotations

import argparse
import hashlib
import importlib.metadata
import json
import os
from pathlib import Path
import platform
import sys

from pxr import Plug, Sdr, Tf, Usd, UsdShade, UsdValidation


def digest(path):
    value = hashlib.sha256()
    with Path(path).open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            value.update(block)
    return value.hexdigest()


def node_report(node):
    definition = node.GetResolvedDefinitionURI()
    return {"identifier": str(node.GetIdentifier()), "valid": bool(node.IsValid()),
            "source_type": str(node.GetSourceType()), "definition": definition,
            "definition_sha256": digest(definition) if Path(definition).is_file() else None,
            "inputs": sorted(str(name) for name in node.GetShaderInputNames()),
            "outputs": sorted(str(name) for name in node.GetShaderOutputNames())}


def validate(paths):
    report = {"schema": "usd-toolbox.full-registry-validation.v1", "success": False,
              "python": platform.python_version(), "openusd": ".".join(map(str, Usd.GetVersion())),
              "runtime_packages": {}, "materialx_search_paths": os.environ.get("PXR_MTLX_STDLIB_SEARCH_PATHS", ""),
              "plugins": [], "registry_probe": [],
              "validators": [], "assets": [], "failures": [], "exemptions": []}
    for package in ("usd-exchange", "usd-core", "MaterialX"):
        try:
            report["runtime_packages"][package] = importlib.metadata.version(package)
        except importlib.metadata.PackageNotFoundError:
            pass
    if "usd-core" in report["runtime_packages"] and "usd-exchange" in report["runtime_packages"]:
        report["failures"].append("usd-core and usd-exchange must not share an environment")
        return report
    plugin = Plug.Registry().GetPluginWithName("usdMtlx")
    if not plugin or not plugin.Load() or not plugin.isLoaded:
        report["failures"].append("usdMtlx native plugin is missing or could not load")
        return report
    report["plugins"].append({"name": plugin.name, "path": plugin.path,
                              "sha256": digest(plugin.path), "loaded": plugin.isLoaded})
    for name in ("UsdMtlxDiscoveryPlugin", "UsdMtlxParserPlugin"):
        ty = Tf.Type.FindByName(name)
        owner = Plug.Registry().GetPluginForType(ty) if ty else None
        if not ty or not owner or owner.name != "usdMtlx":
            report["failures"].append(f"{name} is not registered by usdMtlx")
        else:
            report["plugins"].append({"type": name, "owner": owner.name, "loaded": owner.isLoaded})
    registry = Sdr.Registry()
    for identifier in ("UsdPreviewSurface", "ND_open_pbr_surface_surfaceshader", "ND_image_color3", "ND_image_float", "ND_normalmap_float", "ND_displacement_float", "ND_standard_surface_surfaceshader", "ND_noise3d_color3FA"):
        node = registry.GetShaderNodeByIdentifier(identifier)
        if not node or not node.IsValid() or (identifier.startswith("ND_") and node.GetSourceType() != "mtlx"):
            report["failures"].append(f"Required shader {identifier} did not resolve through the expected native registry")
        else:
            report["registry_probe"].append(node_report(node))
    validators = UsdValidation.ValidationRegistry().GetOrLoadAllValidators()
    report["validators"] = sorted(str(validator.GetMetadata().name) for validator in validators)
    if "usdShadeValidators:ShaderSdrCompliance" not in report["validators"]:
        report["failures"].append("Native ShaderSdrCompliance validator is missing")
    if report["failures"]:
        return report
    context = UsdValidation.ValidationContext(validators)
    for path in paths:
        asset = {"path": str(path), "sha256": digest(path), "shaders": [], "findings": [], "success": False}
        report["assets"].append(asset)
        try:
            stage = Usd.Stage.Open(str(path))
            if not stage:
                raise ValueError("OpenUSD could not open stage")
            for prim in stage.Traverse():
                if not prim.IsA(UsdShade.Shader):
                    continue
                shader = UsdShade.Shader(prim)
                identifier = shader.GetIdAttr().Get()
                if shader.GetImplementationSource() != "id":
                    asset["findings"].append({"identifier": "full_registry.non_id_shader", "path": str(prim.GetPath()), "message": "Fixture requires a source-code shader validation policy"})
                    continue
                node = registry.GetShaderNodeByIdentifier(identifier) if identifier else None
                if not node or not node.IsValid():
                    asset["findings"].append({"identifier": "full_registry.shader_missing", "path": str(prim.GetPath()), "message": f"Shader {identifier!r} is absent or invalid in Sdr"})
                else:
                    asset["shaders"].append({"path": str(prim.GetPath()), **node_report(node)})
            for finding in context.Validate(stage):
                asset["findings"].append({"identifier": str(finding.GetIdentifier()), "type": str(finding.GetType()), "message": finding.GetErrorAsString()})
            asset["success"] = not asset["findings"]
        except Exception as error:
            asset["findings"].append({"identifier": "full_registry.exception", "message": str(error)})
    report["success"] = bool(report["assets"]) and not report["failures"] and all(asset["success"] for asset in report["assets"])
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("files", type=Path, nargs="+")
    parser.add_argument("--report", type=Path)
    options = parser.parse_args()
    report = validate(options.files)
    encoded = json.dumps(report, ensure_ascii=False, indent=2) + "\n"
    if options.report:
        options.report.parent.mkdir(parents=True, exist_ok=True)
        options.report.write_text(encoded)
    print(encoded, end="")
    return 0 if report["success"] else 1


if __name__ == "__main__":
    sys.exit(main())
