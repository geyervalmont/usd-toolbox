//! Texture-set ingest behaviour.

use std::io::Cursor;

use image::{DynamicImage, ImageFormat, Rgba, RgbaImage};
use usd_toolbox_core::{
    Channel, ColorSpace, LossKind, MapRole, NormalConvention, Operation, Tier, Value, validate_material,
};
use usd_toolbox_textures::{TextureImportOptions, TextureInput, TextureMetadata, TextureSetImporter};

fn png(width: u32, height: u32, color: Rgba<u8>) -> Vec<u8> {
    let mut out = Cursor::new(Vec::new());
    DynamicImage::ImageRgba8(RgbaImage::from_pixel(width, height, color))
        .write_to(&mut out, ImageFormat::Png)
        .unwrap();
    out.into_inner()
}

fn jpeg(width: u32, height: u32, color: Rgba<u8>) -> Vec<u8> {
    let mut out = Cursor::new(Vec::new());
    DynamicImage::ImageRgba8(RgbaImage::from_pixel(width, height, color))
        .write_to(&mut out, ImageFormat::Jpeg)
        .unwrap();
    out.into_inner()
}

#[test]
fn requires_explicit_colour_space() {
    let input = TextureInput {
        name: "cloth_base_color.png".into(),
        bytes: png(2, 2, Rgba([1, 2, 3, 255])),
        metadata: TextureMetadata::default(),
    };
    let error = TextureSetImporter
        .import_set(&[input], &TextureImportOptions::default())
        .unwrap_err();
    assert!(error.to_string().contains("color_space"));
}

#[test]
fn glossiness_is_inverted_and_source_lineage_is_retained() {
    let input = TextureInput {
        name: "cloth_gloss_preview.png".into(),
        bytes: png(2, 2, Rgba([20, 30, 40, 255])),
        metadata: TextureMetadata {
            role: Some(MapRole::Glossiness),
            tier: Some(Tier::Preview),
            color_space: Some(ColorSpace::Raw),
            channel: Some(Channel::R),
            normal_convention: None,
        },
    };
    let options = TextureImportOptions {
        material_id: "cloth".into(),
        material_name: "Cloth".into(),
        required_tiers: vec![Tier::Preview],
        ..Default::default()
    };
    let result = TextureSetImporter.import_set(&[input], &options).unwrap();
    assert_eq!(result.losses.len(), 1);
    assert_eq!(result.losses[0].kind, LossKind::Converted);
    assert_eq!(result.material.provenance.source_assets.len(), 1);
    assert!(validate_material(&result.material).is_empty());

    let Value::Texture { texture } = result.material.surface.specular_roughness else {
        panic!("expected roughness texture")
    };
    assert_eq!(texture.role, MapRole::Roughness);
    let decoded = image::load_from_memory(&texture.tiers[&Tier::Preview].bytes)
        .unwrap()
        .to_rgba8();
    assert_eq!(decoded.get_pixel(0, 0).0[0], 235);
}

#[test]
fn missing_preview_is_generated_deterministically() {
    let input = TextureInput {
        name: "paint_base_color_1k.png".into(),
        bytes: png(1024, 512, Rgba([10, 20, 30, 255])),
        metadata: TextureMetadata {
            role: None,
            tier: None,
            color_space: Some(ColorSpace::Srgb),
            channel: None,
            normal_convention: None,
        },
    };
    let options = TextureImportOptions {
        material_id: "paint".into(),
        material_name: "Paint".into(),
        required_tiers: vec![Tier::Preview, Tier::K1],
        ..Default::default()
    };
    let first = TextureSetImporter
        .import_set(std::slice::from_ref(&input), &options)
        .unwrap();
    let second = TextureSetImporter.import_set(&[input], &options).unwrap();
    assert_eq!(first, second);
    assert_eq!(first.losses.len(), 1);

    let texture = first.material.surface.base_color.texture().unwrap();
    assert_eq!(texture.tiers[&Tier::Preview].width, Some(512));
    let derivation = &first.material.provenance.derivations[&texture.tiers[&Tier::Preview].hash];
    assert_eq!(derivation.operation, Operation::Downscale);
}

#[test]
fn base_color_downscales_preserve_a_jpeg_source_codec() {
    let bytes = jpeg(1024, 512, Rgba([20, 40, 60, 255]));
    let input = TextureInput {
        name: "paint_base_color_1k.jpg".into(),
        bytes: bytes.clone(),
        metadata: TextureMetadata {
            role: Some(MapRole::BaseColor),
            tier: Some(Tier::K1),
            color_space: Some(ColorSpace::Srgb),
            channel: None,
            normal_convention: None,
        },
    };
    let options = TextureImportOptions {
        material_id: "paint".into(),
        material_name: "Paint".into(),
        required_tiers: vec![Tier::Preview, Tier::K1],
        ..Default::default()
    };
    let first = TextureSetImporter
        .import_set(std::slice::from_ref(&input), &options)
        .unwrap();
    let second = TextureSetImporter.import_set(&[input], &options).unwrap();
    assert_eq!(first, second);

    let texture = first.material.surface.base_color.texture().unwrap();
    assert_eq!(texture.tiers[&Tier::K1].bytes, bytes);
    assert_eq!(texture.tiers[&Tier::Preview].extension, "jpg");
    assert_eq!(texture.tiers[&Tier::Preview].media_type, "image/jpeg");
    assert_eq!(
        image::guess_format(&texture.tiers[&Tier::Preview].bytes).unwrap(),
        ImageFormat::Jpeg
    );
}

#[test]
fn data_maps_use_lossless_png_storage_by_default() {
    let bytes = jpeg(512, 512, Rgba([90, 90, 90, 255]));
    let input = TextureInput {
        name: "paint_roughness_preview.jpg".into(),
        bytes: bytes.clone(),
        metadata: TextureMetadata {
            role: Some(MapRole::Roughness),
            tier: Some(Tier::Preview),
            color_space: Some(ColorSpace::Raw),
            channel: Some(Channel::R),
            normal_convention: None,
        },
    };
    let result = TextureSetImporter
        .import_set(
            &[input],
            &TextureImportOptions {
                material_id: "paint".into(),
                material_name: "Paint".into(),
                required_tiers: vec![Tier::Preview],
                ..Default::default()
            },
        )
        .unwrap();

    let texture = result.material.surface.specular_roughness.texture().unwrap();
    assert_eq!(texture.tiers[&Tier::Preview].extension, "png");
    assert_eq!(texture.tiers[&Tier::Preview].media_type, "image/png");
    assert_eq!(result.material.provenance.source_assets.len(), 1);
    assert_eq!(
        result.material.provenance.source_assets.values().next().unwrap().bytes,
        bytes
    );
    assert!(result.losses[0].detail.contains("codec policy"));
}

#[test]
fn normal_convention_is_explicit_unless_compatibility_is_enabled() {
    let input = TextureInput {
        name: "cloth_normal_gl_preview.png".into(),
        bytes: png(2, 2, Rgba([128, 128, 255, 255])),
        metadata: TextureMetadata {
            role: Some(MapRole::Normal),
            tier: Some(Tier::Preview),
            color_space: Some(ColorSpace::Linear),
            channel: None,
            normal_convention: None,
        },
    };
    let strict_error = TextureSetImporter
        .import_set(
            std::slice::from_ref(&input),
            &TextureImportOptions {
                required_tiers: vec![Tier::Preview],
                ..Default::default()
            },
        )
        .unwrap_err();
    assert!(strict_error.to_string().contains("normal_convention"));

    let compatible = TextureSetImporter
        .import_set(
            &[input],
            &TextureImportOptions {
                required_tiers: vec![Tier::Preview],
                infer_normal_convention_from_filename: true,
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(
        compatible
            .material
            .geometry
            .normal
            .unwrap()
            .texture()
            .unwrap()
            .normal_convention,
        Some(NormalConvention::OpenGl)
    );
}

#[test]
fn missing_larger_tier_is_not_upscaled() {
    let input = TextureInput {
        name: "paint_base_color_1k.png".into(),
        bytes: png(1024, 512, Rgba([10, 20, 30, 255])),
        metadata: TextureMetadata {
            role: Some(MapRole::BaseColor),
            tier: Some(Tier::K1),
            color_space: Some(ColorSpace::Srgb),
            channel: None,
            normal_convention: None,
        },
    };
    let error = TextureSetImporter
        .import_set(
            &[input],
            &TextureImportOptions {
                required_tiers: vec![Tier::K2],
                ..Default::default()
            },
        )
        .unwrap_err();
    assert!(error.to_string().contains("upscaling is outside"));
}
