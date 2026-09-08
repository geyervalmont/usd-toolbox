use std::collections::BTreeMap;
use std::io::{Cursor, Read};

use image::{DynamicImage, ImageFormat, Rgba, RgbaImage};
use sha2::{Digest, Sha256};
use usd_toolbox_core::{
    Channel, ColorSpace, Exporter, MapRole, Material, Sha256 as DigestValue, TextureRef, TextureSource, Tier, Value,
};
use usd_toolbox_revit::{RevitExportOptions, RevitExporter};

fn texture(role: MapRole, value: u8) -> TextureRef {
    let mut cursor = Cursor::new(Vec::new());
    DynamicImage::ImageRgba8(RgbaImage::from_pixel(512, 512, Rgba([value, value, value, 255])))
        .write_to(&mut cursor, ImageFormat::Png)
        .unwrap();
    let bytes = cursor.into_inner();
    TextureRef {
        role,
        tiers: BTreeMap::from([(
            Tier::Preview,
            TextureSource {
                hash: DigestValue(format!("{:x}", Sha256::digest(&bytes))),
                extension: "png".into(),
                media_type: "image/png".into(),
                width: Some(512),
                height: Some(512),
                bytes,
            },
        )]),
        color_space: if role == MapRole::BaseColor {
            ColorSpace::Srgb
        } else {
            ColorSpace::Raw
        },
        channel: if role == MapRole::BaseColor {
            Channel::Rgb
        } else {
            Channel::R
        },
        normal_convention: None,
    }
}

#[test]
fn emits_the_three_slots_consumed_by_the_opal_connector() {
    let mut material = Material::new("tile", "Tile");
    material.surface.base_color = Value::Texture {
        texture: texture(MapRole::BaseColor, 100),
    };
    material.surface.specular_roughness = Value::Texture {
        texture: texture(MapRole::Roughness, 64),
    };
    let first = RevitExporter
        .export(
            &[material.clone()],
            &RevitExportOptions {
                texture_tier: Tier::Preview,
                include_manifest: true,
            },
        )
        .unwrap();
    let export = RevitExporter
        .export(
            &[material],
            &RevitExportOptions {
                texture_tier: Tier::Preview,
                include_manifest: true,
            },
        )
        .unwrap();
    assert_eq!(first.bytes, export.bytes);
    let mut archive = zip::ZipArchive::new(Cursor::new(export.bytes)).unwrap();
    let mut manifest = String::new();
    archive
        .by_name("revit-material.json")
        .unwrap()
        .read_to_string(&mut manifest)
        .unwrap();
    assert!(manifest.contains("generic_diffuse"));
    assert!(manifest.contains("generic_glossiness"));
    assert!(archive.by_name("materials/tile/base_color.png").is_ok());
    assert!(archive.by_name("materials/tile/glossiness.png").is_ok());
}

#[test]
fn constant_materials_are_baked_for_the_connector_image_contract() {
    let material = Material::new("plain", "Plain");
    let export = RevitExporter
        .export(&[material], &RevitExportOptions::default())
        .unwrap();
    let mut archive = zip::ZipArchive::new(Cursor::new(export.bytes)).unwrap();
    assert!(archive.by_name("materials/plain/base_color.png").is_ok());
    assert!(
        export
            .losses
            .iter()
            .any(|loss| loss.detail.contains("constant base color"))
    );
}
