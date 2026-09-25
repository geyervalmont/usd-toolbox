//! Independent validator/browser fixture exercising every authored shader slot.
use base64::Engine;
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, path::PathBuf};
use usd_toolbox_core::*;
use usd_toolbox_materialx::{MaterialXExportOptions, MaterialXExporter, export_bundle};
use usd_toolbox_usd::{UsdExportOptions, UsdExporter};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dir = PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| "target/pbr-fixture".into()));
    std::fs::create_dir_all(dir.join("textures"))?;
    let bytes = base64::engine::general_purpose::STANDARD.decode(
        "iVBORw0KGgoAAAANSUhEUgAAAAgAAAAICAYAAADED76LAAAAEklEQVR4nGNoaPj/Hx9mGBkKAIVGv4EQSHYxAAAAAElFTkSuQmCC",
    )?;
    let source = TextureSource {
        hash: usd_toolbox_core::Sha256(format!("{:x}", Sha256::digest(&bytes))),
        extension: "png".into(),
        media_type: "image/png".into(),
        width: Some(8),
        height: Some(8),
        bytes,
    };
    std::fs::write(dir.join(source.package_path()), &source.bytes)?;
    let texture = |role, channel, normal_convention| TextureRef {
        role,
        tiers: BTreeMap::from([(Tier::Preview, source.clone())]),
        color_space: ColorSpace::Raw,
        channel,
        normal_convention,
    };
    let mut material = Material::new("pbr_fixture", "PBR fixture");
    material.surface.base_color = Value::Modulated {
        texture: TextureRef {
            color_space: ColorSpace::Srgb,
            ..texture(MapRole::BaseColor, Channel::Rgb, None)
        },
        factor: [0.4, 0.1, 0.02, 1.0],
    };
    material.surface.specular_roughness = Value::Texture {
        texture: texture(MapRole::Roughness, Channel::G, None),
    };
    material.surface.base_metalness = Value::Modulated {
        texture: texture(MapRole::Metallic, Channel::B, None),
        factor: 0.2,
    };
    material.surface.specular_anisotropy = Some(0.2.into());
    material.surface.coat_weight = Some(0.2.into());
    material.surface.fuzz_weight = Some(0.1.into());
    material.surface.subsurface_weight = Some(0.1.into());
    material.surface.transmission_weight = Some(0.1.into());
    material.surface.transmission_thickness = Some(0.1.into());
    material.surface.specular_ior = Some(1.5.into());
    material.surface.emission_color = Some([0.01, 0.01, 0.01].into());
    material.geometry.normal = Some(Value::Texture {
        texture: texture(MapRole::Normal, Channel::Rgb, Some(NormalConvention::DirectX)),
    });
    material.geometry.opacity = Some(Value::Texture {
        texture: texture(MapRole::Opacity, Channel::A, None),
    });
    material.geometry.bump = Some(Value::from(0.01));
    material.geometry.height = Some(Value::from(0.01));
    let materials = &[material];
    std::fs::write(
        dir.join("material.mtlx"),
        MaterialXExporter
            .export(
                materials,
                &MaterialXExportOptions {
                    embed_payloads: false,
                    include_neutral_manifest: false,
                    ..Default::default()
                },
            )?
            .bytes,
    )?;
    std::fs::write(
        dir.join("material.usdz"),
        UsdExporter.export(materials, &UsdExportOptions::default())?.bytes,
    )?;
    std::fs::write(
        dir.join("preview.json"),
        serde_json::to_vec(&export_bundle(materials, Tier::Preview)?)?,
    )?;
    Ok(())
}
