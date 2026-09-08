//! glTF and GLB export behaviour.

use usd_toolbox_core::{Exporter, LossKind, Material};
use usd_toolbox_gltf::{GltfExportOptions, GltfExporter, GltfFormat};

#[test]
fn writes_ratified_extensions_and_reports_subsurface() {
    let mut material = Material::new("fabric", "Fabric");
    material.surface.specular_ior = Some(1.4.into());
    material.surface.transmission_weight = Some(0.7.into());
    material.surface.subsurface_weight = Some(0.3.into());
    let export = GltfExporter
        .export(
            &[material],
            &GltfExportOptions {
                format: GltfFormat::Gltf,
                ..Default::default()
            },
        )
        .unwrap();
    let document: serde_json::Value = serde_json::from_slice(&export.bytes).unwrap();
    assert_eq!(document["asset"]["version"], "2.0");
    let ior = document["materials"][0]["extensions"]["KHR_materials_ior"]["ior"]
        .as_f64()
        .unwrap();
    let transmission = document["materials"][0]["extensions"]["KHR_materials_transmission"]["transmissionFactor"]
        .as_f64()
        .unwrap();
    assert!((ior - 1.4).abs() < 1e-6);
    assert!((transmission - 0.7).abs() < 1e-6);
    assert!(
        export
            .losses
            .iter()
            .any(|loss| loss.parameter == "subsurface_weight" && loss.kind == LossKind::Dropped)
    );
}

#[test]
fn writes_valid_glb_envelope_deterministically() {
    let material = Material::new("plain", "Plain");
    let first = GltfExporter
        .export(std::slice::from_ref(&material), &GltfExportOptions::default())
        .unwrap();
    let second = GltfExporter.export(&[material], &GltfExportOptions::default()).unwrap();
    assert_eq!(first.bytes, second.bytes);
    assert_eq!(&first.bytes[0..4], b"glTF");
    assert_eq!(u32::from_le_bytes(first.bytes[4..8].try_into().unwrap()), 2);
    assert_eq!(
        u32::from_le_bytes(first.bytes[8..12].try_into().unwrap()) as usize,
        first.bytes.len()
    );
    assert_eq!(&first.bytes[16..20], b"JSON");
}

#[test]
fn converts_neutral_millimetres_to_gltf_metres() {
    let mut material = Material::new("glass", "Glass");
    material.surface.transmission_thickness = Some(12.0.into());
    let export = GltfExporter
        .export(
            &[material],
            &GltfExportOptions {
                format: GltfFormat::Gltf,
                ..Default::default()
            },
        )
        .unwrap();
    let document: serde_json::Value = serde_json::from_slice(&export.bytes).unwrap();
    let thickness = document["materials"][0]["extensions"]["KHR_materials_volume"]["thicknessFactor"]
        .as_f64()
        .unwrap();
    assert!((thickness - 0.012).abs() < 1e-6);
}

#[test]
fn uses_emissive_strength_for_hdr_emission() {
    let mut material = Material::new("light", "Light");
    material.surface.emission_color = Some([4.0, 1.0, 0.0].into());
    let export = GltfExporter
        .export(
            &[material],
            &GltfExportOptions {
                format: GltfFormat::Gltf,
                ..Default::default()
            },
        )
        .unwrap();
    let document: serde_json::Value = serde_json::from_slice(&export.bytes).unwrap();
    assert_eq!(
        document["materials"][0]["emissiveFactor"],
        serde_json::json!([1.0, 0.25, 0.0])
    );
    assert_eq!(
        document["materials"][0]["extensions"]["KHR_materials_emissive_strength"]["emissiveStrength"],
        4.0
    );
}
