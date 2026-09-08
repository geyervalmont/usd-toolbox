//! USD and USDZ golden properties.

use std::collections::BTreeMap;
use std::io::{Cursor, Read};

use sha2::{Digest, Sha256 as Sha256Hasher};
use usd_toolbox_core::{
    AuxiliaryAsset, AuxiliaryRole, ColorSpace, Exporter, Importer, Input, MapRole, Material, ParameterValue,
    ProvenanceAsset, Sha256, TextureRef, TextureSource, Tier, Value, Variant, VariantSet,
};
use usd_toolbox_usd::{UsdExportOptions, UsdExporter, UsdFormat, UsdImportOptions, UsdImporter};

fn textured_material() -> Material {
    let bytes = b"not decoded by the USD spoke".to_vec();
    let hash = Sha256(format!("{:x}", Sha256Hasher::digest(&bytes)));
    let mut material = Material::new("paint/red", "Paint Red");
    material.surface.base_color = Value::Texture {
        texture: TextureRef {
            role: MapRole::BaseColor,
            tiers: BTreeMap::from([(
                Tier::Preview,
                TextureSource {
                    hash,
                    extension: "png".into(),
                    media_type: "image/png".into(),
                    width: Some(2),
                    height: Some(2),
                    bytes,
                },
            )]),
            color_space: ColorSpace::Srgb,
            channel: Default::default(),
            normal_convention: None,
        },
    };
    material
}

fn reference_material() -> Material {
    let mut material = Material::new("reference_material", "Reference Material");
    material.surface.base_color = [0.32, 0.12, 0.04, 1.0].into();
    material.surface.base_metalness = 0.15.into();
    material.surface.specular_roughness = 0.42.into();
    material.surface.specular_ior = Some(1.5.into());
    material.variants.push(VariantSet {
        name: "colour way".into(),
        default: "warm red".into(),
        variants: vec![
            Variant {
                name: "warm red".into(),
                overrides: BTreeMap::from([(
                    "base_color".into(),
                    ParameterValue::Color4(Value::from([0.55, 0.04, 0.02, 1.0])),
                )]),
            },
            Variant {
                name: "cool blue".into(),
                overrides: BTreeMap::from([(
                    "base_color".into(),
                    ParameterValue::Color4(Value::from([0.03, 0.12, 0.55, 1.0])),
                )]),
            },
        ],
    });
    material
}

#[test]
fn usda_is_readable_and_round_trips_metadata() {
    let material = Material::new("plain", "Plain");
    let options = UsdExportOptions {
        format: UsdFormat::Usda,
        ..Default::default()
    };
    let export = UsdExporter.export(std::slice::from_ref(&material), &options).unwrap();
    let text = std::str::from_utf8(&export.bytes).unwrap();
    assert!(text.starts_with("#usda 1.0"));
    assert!(text.contains("def Material \"plain\""));
    let imported = UsdImporter
        .import(Input::Bytes(&export.bytes), &UsdImportOptions::default())
        .unwrap();
    assert_eq!(imported, vec![material]);
}

#[test]
fn standalone_usda_embeds_texture_payloads_for_round_trip() {
    let material = textured_material();
    let options = UsdExportOptions {
        format: UsdFormat::Usda,
        ..Default::default()
    };
    let export = UsdExporter.export(std::slice::from_ref(&material), &options).unwrap();
    let imported = UsdImporter
        .import(Input::Bytes(&export.bytes), &UsdImportOptions::default())
        .unwrap();
    assert_eq!(imported, vec![material]);
}

#[test]
fn usdz_is_deterministic_aligned_and_lossless() {
    let material = textured_material();
    let first = UsdExporter
        .export(std::slice::from_ref(&material), &UsdExportOptions::default())
        .unwrap();
    let second = UsdExporter
        .export(std::slice::from_ref(&material), &UsdExportOptions::default())
        .unwrap();
    assert_eq!(first.bytes, second.bytes);

    let mut zip = zip::ZipArchive::new(Cursor::new(&first.bytes)).unwrap();
    for index in 0..zip.len() {
        let mut file = zip.by_index(index).unwrap();
        assert_eq!(file.compression(), zip::CompressionMethod::Stored);
        assert_eq!(file.data_start().unwrap() % 64, 0, "{}", file.name());
        let mut ignored = Vec::new();
        file.read_to_end(&mut ignored).unwrap();
    }

    let imported = UsdImporter
        .import(Input::Bytes(&first.bytes), &UsdImportOptions::default())
        .unwrap();
    assert_eq!(imported, vec![material]);
}

#[test]
fn usdz_references_payloads_by_hash_without_inline_bytes_or_duplicate_sources() {
    let mut material = textured_material();
    let source = material.surface.base_color.texture().unwrap().tiers[&Tier::Preview].clone();
    material.provenance.source_assets.insert(
        source.hash.clone(),
        ProvenanceAsset {
            name: "original.png".into(),
            extension: source.extension.clone(),
            media_type: source.media_type.clone(),
            bytes: source.bytes.clone(),
        },
    );

    let export = UsdExporter
        .export(std::slice::from_ref(&material), &UsdExportOptions::default())
        .unwrap();
    let mut archive = zip::ZipArchive::new(Cursor::new(&export.bytes)).unwrap();
    let mut stage = String::new();
    archive
        .by_name("material.usda")
        .unwrap()
        .read_to_string(&mut stage)
        .unwrap();
    assert!(!stage.contains("\"bytes\":"));
    assert!(stage.lines().map(str::len).max().unwrap_or(0) < 10_000);

    let names = (0..archive.len())
        .map(|index| archive.by_index(index).unwrap().name().to_owned())
        .collect::<Vec<_>>();
    assert_eq!(
        names.iter().filter(|name| name.contains(&source.hash.0)).count(),
        1,
        "content-addressed payload must only be stored once: {names:?}"
    );
    assert!(names.contains(&source.package_path()));
    assert!(!names.contains(&format!("provenance/sources/{}.png", source.hash)));

    let imported = UsdImporter
        .import(Input::Bytes(&export.bytes), &UsdImportOptions::default())
        .unwrap();
    assert_eq!(imported, vec![material]);
}

#[test]
fn variant_overrides_are_authored_and_round_trip() {
    let material = reference_material();
    let options = UsdExportOptions {
        format: UsdFormat::Usda,
        ..Default::default()
    };
    let export = UsdExporter.export(std::slice::from_ref(&material), &options).unwrap();
    let text = std::str::from_utf8(&export.bytes).unwrap();
    assert!(text.contains("variantSet \"colour_way\""));
    assert!(text.contains("over \"OpenPBR\""));
    assert!(text.contains("color3f inputs:base_color"));

    let imported = UsdImporter
        .import(Input::Bytes(&export.bytes), &UsdImportOptions::default())
        .unwrap();
    assert_eq!(imported, vec![material]);
}

#[test]
fn usda_matches_the_readable_golden_file() {
    let export = UsdExporter
        .export(
            &[reference_material()],
            &UsdExportOptions {
                format: UsdFormat::Usda,
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(export.bytes, include_bytes!("../../../fixtures/golden/reference.usda"));
}

#[test]
fn unsupported_usdz_auxiliary_types_stay_embedded_in_metadata() {
    let bytes = b"mdl 1.8;".to_vec();
    let hash = Sha256(format!("{:x}", Sha256Hasher::digest(&bytes)));
    let mut material = Material::new("with-mdl", "With MDL");
    material.auxiliary.push(AuxiliaryAsset {
        role: AuxiliaryRole::Mdl,
        name: "source.mdl".into(),
        hash,
        media_type: "application/x-mdl".into(),
        bytes,
    });
    let export = UsdExporter
        .export(std::slice::from_ref(&material), &UsdExportOptions::default())
        .unwrap();
    let mut zip = zip::ZipArchive::new(Cursor::new(&export.bytes)).unwrap();
    assert!((0..zip.len()).all(|index| !zip.by_index(index).unwrap().name().ends_with(".mdl")));
    let imported = UsdImporter
        .import(Input::Bytes(&export.bytes), &UsdImportOptions::default())
        .unwrap();
    assert_eq!(imported, vec![material]);
}
