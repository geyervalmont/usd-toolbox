#!/usr/bin/env python3
"""Reference OpenUSD validation fallback for environments without usdchecker."""

from pathlib import Path
import sys

try:
    from pxr import Sdr, Usd, UsdValidation
except ImportError as error:
    raise SystemExit(
        "OpenUSD Python modules are missing; install usd-core or use usdchecker"
    ) from error


def main() -> int:
    if len(sys.argv) != 2:
        print(f"usage: {sys.argv[0]} path/to/file.usdz", file=sys.stderr)
        return 2

    path = Path(sys.argv[1])
    try:
        stage = Usd.Stage.Open(str(path))
    except Exception as error:  # OpenUSD raises Boost.Python exception types.
        print(f"OpenUSD could not open {path}: {error}", file=sys.stderr)
        return 1
    if not stage:
        print(f"OpenUSD could not open {path}", file=sys.stderr)
        return 1

    registry = UsdValidation.ValidationRegistry()
    context = UsdValidation.ValidationContext(registry.GetOrLoadAllValidators())
    errors = context.Validate(stage)

    materialx_node_ids = {
        "ND_open_pbr_surface_surfaceshader",
        "ND_image_color3",
        "ND_image_float",
    }
    missing_materialx_plugin = not Sdr.Registry().GetShaderNodeByIdentifier(
        "ND_open_pbr_surface_surfaceshader"
    )
    ignored_identifier = (
        "usdShadeValidators:ShaderSdrCompliance.MissingShaderIdInRegistry"
    )
    failures = []
    for error in errors:
        known_materialx_node = any(
            f"'{identifier}'" in error.GetMessage()
            for identifier in materialx_node_ids
        )
        if (
            missing_materialx_plugin
            and str(error.GetIdentifier()) == ignored_identifier
            and known_materialx_node
        ):
            print(
                "NOTE: OpenPBR SDR lookup skipped because this OpenUSD build omits "
                "the optional MaterialX plug-in",
                file=sys.stderr,
            )
            continue
        failures.append(error)
        print(error.GetErrorAsString(), file=sys.stderr)

    if failures:
        print(f"OpenUSD validation failed with {len(failures)} finding(s)", file=sys.stderr)
        return 1

    print(f"OpenUSD opened and validated {path}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
