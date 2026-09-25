use sha2::{Digest, Sha256 as Hasher};
use std::collections::BTreeMap;
use usd_toolbox_core::{
    Channel, ColorSpace, Exporter, Importer, Input, InputFile, MapRole, Material, NormalConvention, Sha256, TextureRef,
    TextureSource, Tier, Value,
};
use usd_toolbox_materialx::{
    MaterialXExportOptions, MaterialXExporter, MaterialXImportOptions, MaterialXImporter, export_bundle,
    referenced_files,
};
use usd_toolbox_usd::{UsdExportOptions, UsdExporter, UsdFormat, UsdImportOptions, UsdImporter};

const NATIVE: &[u8] = br#"<materialx version="1.39">
  <image name="color" type="color3"><input name="file" type="filename" value="textures/red.png" colorspace="srgb_texture"/></image>
  <nodegraph name="NG">
    <noise3d name="noise" type="float"><input name="amplitude" type="float" value="0.3"/></noise3d>
    <output name="roughness" type="float" nodename="noise"/>
  </nodegraph>
  <standard_surface name="Surface" type="surfaceshader">
    <input name="base_color" type="color3" nodename="color"/>
    <input name="specular_roughness" type="float" nodegraph="NG" output="roughness"/>
    <input name="coat" type="float" value="0.25"/>
  </standard_surface>
  <surfacematerial name="Red" type="material"><input name="surfaceshader" type="surfaceshader" nodename="Surface"/></surfacematerial>
</materialx>"#;

fn native() -> Vec<Material> {
    MaterialXImporter
        .import(
            Input::Bundle(&[
                InputFile {
                    name: "material.mtlx",
                    bytes: NATIVE,
                },
                InputFile {
                    name: "textures/red.png",
                    bytes: b"test image payload",
                },
            ]),
            &MaterialXImportOptions::default(),
        )
        .unwrap()
}

#[test]
fn native_graphs_and_assets_survive_usdz_and_materialx_without_flattening() {
    let materials = native();
    let graph = materials[0].materialx.as_ref().unwrap();
    assert_eq!(graph.material_name, "Red");
    assert_eq!(graph.assets.len(), 1);
    assert_eq!(materials[0].surface.coat_weight, Some(Value::from(0.25)));
    let package = UsdExporter.export(&materials, &UsdExportOptions::default()).unwrap();
    let imported = UsdImporter
        .import(Input::Bytes(&package.bytes), &UsdImportOptions::default())
        .unwrap();
    assert_eq!(imported, materials);
    let xml = MaterialXExporter
        .export(&imported, &MaterialXExportOptions::default())
        .unwrap();
    assert!(String::from_utf8_lossy(&xml.bytes).contains("nodegraph=\"NG\""));
    assert_eq!(
        MaterialXImporter
            .import(Input::Bytes(&xml.bytes), &MaterialXImportOptions::default())
            .unwrap(),
        materials
    );
    let usd = UsdExporter
        .export(
            &materials,
            &UsdExportOptions {
                format: UsdFormat::Usda,
                ..Default::default()
            },
        )
        .unwrap();
    let text = String::from_utf8(usd.bytes).unwrap();
    assert!(text.contains("outputs:mtlx:surface.connect = </Materials/Red/Surface.outputs:out>"));
    assert!(text.contains("inputs:specular_roughness.connect = </Materials/Red/NG.outputs:roughness>"));
    assert!(text.contains("ND_standard_surface_surfaceshader"));
    assert!(text.contains("ND_noise3d_float"));
}

#[test]
fn native_import_requires_its_assets_and_rejects_external_resolution() {
    assert!(
        MaterialXImporter
            .import(Input::Bytes(NATIVE), &Default::default())
            .unwrap_err()
            .to_string()
            .contains("textures/red.png")
    );
    for path in [
        "../secret.png",
        "https://example.com/a.png",
        "/etc/a.png",
        "x\\a.png",
        "x/%2e%2e/a.png",
    ] {
        let xml = String::from_utf8_lossy(NATIVE).replace("textures/red.png", path);
        assert!(referenced_files(xml.as_bytes()).is_err(), "{path}");
    }
    for invalid in [
        "<!DOCTYPE materialx [<!ENTITY data SYSTEM 'file:///etc/passwd'>]><materialx version='1.39'/>",
        "<materialx version='1.39'><xi:include href='remote.mtlx'/></materialx>",
        "<materialx version='1.39'></materialx><materialx version='1.39'/>",
    ] {
        assert!(usd_toolbox_materialx::parse_document(invalid.as_bytes()).is_err());
    }
}

#[test]
fn browser_bundle_contains_graph_and_only_declared_assets() {
    let materials = native();
    let first = export_bundle(&materials, Tier::Preview).unwrap();
    let second = export_bundle(&materials, Tier::Preview).unwrap();
    assert_eq!(
        serde_json::to_vec(&first).unwrap(),
        serde_json::to_vec(&second).unwrap()
    );
    assert_eq!(first.material_names, vec!["Red"]);
    assert_eq!(first.assets.len(), 1);
    assert!(!first.document.contains("olsyn_neutral_manifest"));
    assert!(
        usd_toolbox_core::dry_run(&materials, usd_toolbox_core::Target::Revit)
            .iter()
            .any(|loss| loss.parameter == "materialx")
    );
}

#[test]
fn generated_pbr_graph_uses_real_channels_tangent_normals_and_displacement() {
    let bytes = b"image".to_vec();
    let source = TextureSource {
        hash: Sha256(format!("{:x}", Hasher::digest(&bytes))),
        extension: "png".into(),
        media_type: "image/png".into(),
        width: None,
        height: None,
        bytes,
    };
    let texture = TextureRef {
        role: MapRole::Roughness,
        tiers: BTreeMap::from([(Tier::Preview, source)]),
        color_space: ColorSpace::Raw,
        channel: Channel::G,
        normal_convention: None,
    };
    let mut material = Material::new("channels", "Channels");
    material.surface.specular_roughness = Value::Texture {
        texture: texture.clone(),
    };
    material.geometry.normal = Some(Value::Texture {
        texture: TextureRef {
            role: MapRole::Normal,
            channel: Channel::Rgb,
            normal_convention: Some(NormalConvention::DirectX),
            ..texture.clone()
        },
    });
    material.geometry.height = Some(Value::from(0.01));
    material.geometry.ambient_occlusion = Some(Value::from(0.8));
    let exported = MaterialXExporter
        .export(&[material.clone()], &Default::default())
        .unwrap();
    let xml = String::from_utf8(exported.bytes).unwrap();
    assert!(xml.contains("ND_extract_color4"));
    assert!(xml.contains("name=\"index\" type=\"integer\" value=\"1\""));
    assert!(xml.contains("ND_normalmap_float"));
    assert!(xml.contains("1, -1, 1"));
    assert!(xml.contains("ND_displacement_float"));
    assert!(!xml.contains("name=\"geometry_height\""));
    assert!(!xml.contains("name=\"ambient_occlusion\""));
    assert!(exported.losses.iter().any(|loss| loss.parameter == "ambient_occlusion"));
    let usd = UsdExporter
        .export(
            &[material],
            &UsdExportOptions {
                format: UsdFormat::Usda,
                ..Default::default()
            },
        )
        .unwrap();
    let usd = String::from_utf8(usd.bytes).unwrap();
    assert!(usd.contains("outputs:mtlx:displacement.connect"));
    assert!(usd.contains("ND_normalmap_float"));
}

#[test]
fn preview_selects_the_requested_native_material_and_detects_stale_manifests() {
    let mut materials = native();
    let graph = materials[0].materialx.as_mut().unwrap();
    let mut other = graph
        .document
        .children
        .iter()
        .find(|node| node.category == "surfacematerial")
        .unwrap()
        .clone();
    other.attributes.insert("name".into(), "Other".into());
    graph.document.children.insert(0, other);
    let preview = export_bundle(&materials, Tier::Preview).unwrap();
    assert_eq!(preview.material_names, ["Red"]);
    let exported = MaterialXExporter.export(&materials, &Default::default()).unwrap();
    let edited = String::from_utf8(exported.bytes)
        .unwrap()
        .replace("value=\"0.25\"", "value=\"0.75\"");
    assert!(
        MaterialXImporter
            .import(Input::Bytes(edited.as_bytes()), &Default::default())
            .unwrap_err()
            .to_string()
            .contains("differs from its embedded")
    );
}
