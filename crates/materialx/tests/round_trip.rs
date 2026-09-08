//! MaterialX graph and manifest behaviour.

use usd_toolbox_core::{Exporter, Importer, Input, Material};
use usd_toolbox_materialx::{MaterialXExportOptions, MaterialXExporter, MaterialXImportOptions, MaterialXImporter};

fn reference_material() -> Material {
    let mut material = Material::new("reference_material", "Reference Material");
    material.surface.base_color = [0.32, 0.12, 0.04, 1.0].into();
    material.surface.base_metalness = 0.15.into();
    material.surface.specular_roughness = 0.42.into();
    material.surface.specular_ior = Some(1.5.into());
    material
}

#[test]
fn authored_document_contains_openpbr_and_round_trips() {
    let mut material = Material::new("paint-red", "Paint Red");
    material.surface.specular_ior = Some(1.45.into());
    let exported = MaterialXExporter
        .export(std::slice::from_ref(&material), &MaterialXExportOptions::default())
        .unwrap();
    let xml = std::str::from_utf8(&exported.bytes).unwrap();
    assert!(xml.contains("<materialx version=\"1.39\""));
    assert!(xml.contains("<open_pbr_surface"));
    assert!(xml.contains("name=\"specular_ior\""));

    let imported = MaterialXImporter
        .import(Input::Bytes(&exported.bytes), &MaterialXImportOptions::default())
        .unwrap();
    assert_eq!(imported, vec![material]);
}

#[test]
fn output_is_byte_deterministic() {
    let material = Material::new("plain", "Plain");
    let first = MaterialXExporter
        .export(std::slice::from_ref(&material), &MaterialXExportOptions::default())
        .unwrap();
    let second = MaterialXExporter
        .export(&[material], &MaterialXExportOptions::default())
        .unwrap();
    assert_eq!(first.bytes, second.bytes);
}

#[test]
fn output_matches_the_readable_golden_file() {
    let export = MaterialXExporter
        .export(&[reference_material()], &MaterialXExportOptions::default())
        .unwrap();
    assert_eq!(export.bytes, include_bytes!("../../../fixtures/golden/reference.mtlx"));
}
