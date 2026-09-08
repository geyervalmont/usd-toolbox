use std::io::Cursor;
use std::process::Command;

use image::{DynamicImage, ImageFormat, Rgba, RgbaImage};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use tempfile::tempdir;
use usd_toolbox_core::{
    Channel, ColorSpace, Exporter, Importer, Input, MapRole, Material, Repeat, Sha256 as DigestValue, TextureRef,
    TextureSource, Tier, Value as NeutralValue,
};
use usd_toolbox_usd::{UsdExportOptions, UsdExporter, UsdImportOptions, UsdImporter};

fn png() -> Vec<u8> {
    let mut out = Cursor::new(Vec::new());
    DynamicImage::ImageRgba8(RgbaImage::from_pixel(8, 4, Rgba([20, 40, 60, 255])))
        .write_to(&mut out, ImageFormat::Png)
        .unwrap();
    out.into_inner()
}

fn invoke(manifest: &std::path::Path, output: &std::path::Path, report: &std::path::Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_usd-toolbox"))
        .args(["build", "--manifest"])
        .arg(manifest)
        .arg("--output")
        .arg(output)
        .arg("--report")
        .arg(report)
        .output()
        .unwrap()
}

#[test]
fn version_output_is_bare_semver() {
    let process = Command::new(env!("CARGO_BIN_EXE_usd-toolbox"))
        .arg("--version")
        .output()
        .unwrap();
    assert!(process.status.success());
    assert_eq!(
        String::from_utf8(process.stdout).unwrap(),
        concat!(env!("CARGO_PKG_VERSION"), "\n")
    );
}

#[test]
fn builds_the_laravel_manifest_contract_and_reports_the_result() {
    let directory = tempdir().unwrap();
    let sources = directory.path().join("sources");
    std::fs::create_dir(&sources).unwrap();
    let texture = png();
    let hash = format!("{:x}", Sha256::digest(&texture));
    let source_path = sources.join(format!("{hash}.png"));
    std::fs::write(&source_path, &texture).unwrap();

    let manifest_path = directory.path().join("manifest.json");
    std::fs::write(
        &manifest_path,
        serde_json::to_vec_pretty(&json!({
            "schema": 1,
            "variant": {"code": "CPT-TEST-BLUE", "name": "Blue"},
            "shading_model": "openpbr",
            "required_tiers": ["preview"],
            "tiling": {
                "width_mm": "500.00",
                "height_mm": "250.00",
                "repeat": "tile",
                "install_pattern": "Monolithic"
            },
            "channels": {
                "base_color": {
                    "preview": {
                        "sha256": hash,
                        "object_key": "files/source.png",
                        "bytes": texture.len(),
                        "colour_space": "srgb",
                        "width_px": 8,
                        "height_px": 4,
                        "normal_convention": null,
                        "path": source_path.strip_prefix(directory.path()).unwrap()
                    }
                },
                "render": {
                    "preview": {
                        "sha256": hash,
                        "object_key": "renders/preview.png",
                        "bytes": texture.len(),
                        "colour_space": "srgb",
                        "width_px": 8,
                        "height_px": 4,
                        "normal_convention": null,
                        "path": source_path.strip_prefix(directory.path()).unwrap()
                    }
                }
            },
            "provenance": {"material": "CPT-TEST", "variant": "CPT-TEST-BLUE"},
            "ingested_at": "2026-09-08T03:00:00Z"
        }))
        .unwrap(),
    )
    .unwrap();
    let output_path = directory.path().join("package.usdz");
    let report_path = directory.path().join("report.json");

    let process = invoke(&manifest_path, &output_path, &report_path);
    assert!(process.status.success(), "{}", String::from_utf8_lossy(&process.stderr));

    let report: Value = serde_json::from_slice(&std::fs::read(&report_path).unwrap()).unwrap();
    assert_eq!(report["schema"], 1);
    assert_eq!(report["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(report["tiers"], json!(["preview"]));
    assert_eq!(report["losses"][0]["parameter"], "tiling.repeat");
    assert_eq!(
        report["output"]["bytes"],
        std::fs::metadata(&output_path).unwrap().len()
    );
    assert_eq!(
        report["output"]["sha256"],
        format!("{:x}", Sha256::digest(std::fs::read(&output_path).unwrap()))
    );

    let package = std::fs::read(&output_path).unwrap();
    let materials = UsdImporter
        .import(Input::Bytes(&package), &UsdImportOptions::default())
        .unwrap();
    let material = &materials[0];
    assert_eq!(material.id.0, "CPT-TEST-BLUE");
    assert_eq!(material.provenance.metadata["material"], "CPT-TEST");
    assert_eq!(material.provenance.metadata["variant"], "CPT-TEST-BLUE");
    assert_eq!(material.tiling.as_ref().unwrap().repeat, Repeat::Straight);
    assert_eq!(material.auxiliary.len(), 1);
    assert_eq!(material.auxiliary[0].name, "renders/preview.png");

    let first_package = package;
    let repeated = invoke(&manifest_path, &output_path, &report_path);
    assert!(
        repeated.status.success(),
        "{}",
        String::from_utf8_lossy(&repeated.stderr)
    );
    assert_eq!(std::fs::read(&output_path).unwrap(), first_package);
}

#[test]
fn refuses_to_infer_a_normal_convention_from_the_filename() {
    let directory = tempdir().unwrap();
    let texture = png();
    let source_path = directory.path().join("cloth_normal_gl.png");
    std::fs::write(&source_path, &texture).unwrap();
    let manifest_path = directory.path().join("manifest.json");
    std::fs::write(
        &manifest_path,
        serde_json::to_vec(&json!({
            "schema": 1,
            "variant": {"code": "CLOTH", "name": "Cloth"},
            "shading_model": "openpbr",
            "required_tiers": ["preview"],
            "tiling": [],
            "channels": {"normal": {"preview": {
                "sha256": format!("{:x}", Sha256::digest(&texture)),
                "object_key": "cloth_normal_gl.png",
                "bytes": texture.len(),
                "colour_space": "linear",
                "width_px": 8,
                "height_px": 4,
                "normal_convention": null,
                "path": "cloth_normal_gl.png"
            }}},
            "provenance": [],
            "ingested_at": "2026-09-08T03:00:00Z"
        }))
        .unwrap(),
    )
    .unwrap();

    let process = invoke(
        &manifest_path,
        &directory.path().join("package.usdz"),
        &directory.path().join("report.json"),
    );
    assert!(!process.status.success());
    assert!(String::from_utf8_lossy(&process.stderr).contains("normal_convention must be explicit"));
}

#[test]
fn inspection_audit_and_revit_export_are_available_from_the_cli() {
    let directory = tempdir().unwrap();
    let mut cursor = Cursor::new(Vec::new());
    DynamicImage::ImageRgba8(RgbaImage::from_pixel(512, 512, Rgba([20, 40, 60, 255])))
        .write_to(&mut cursor, ImageFormat::Png)
        .unwrap();
    let bytes = cursor.into_inner();
    let mut material = Material::new("cli-material", "CLI Material");
    material.surface.base_color = NeutralValue::Texture {
        texture: TextureRef {
            role: MapRole::BaseColor,
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
            color_space: ColorSpace::Srgb,
            channel: Channel::Rgb,
            normal_convention: None,
        },
    };
    let package = UsdExporter.export(&[material], &UsdExportOptions::default()).unwrap();
    let input = directory.path().join("material.usdz");
    std::fs::write(&input, package.bytes).unwrap();

    let inspection = directory.path().join("inspection.json");
    let process = Command::new(env!("CARGO_BIN_EXE_usd-toolbox"))
        .args(["inspect", "--input"])
        .arg(&input)
        .arg("--output")
        .arg(&inspection)
        .output()
        .unwrap();
    assert!(process.status.success(), "{}", String::from_utf8_lossy(&process.stderr));
    let inspection: Value = serde_json::from_slice(&std::fs::read(inspection).unwrap()).unwrap();
    assert_eq!(inspection["schema"], "usd-toolbox.inspection.v1");

    let audit = directory.path().join("audit.json");
    let process = Command::new(env!("CARGO_BIN_EXE_usd-toolbox"))
        .args(["audit", "--input"])
        .arg(&input)
        .arg("--report")
        .arg(&audit)
        .output()
        .unwrap();
    assert!(process.status.success(), "{}", String::from_utf8_lossy(&process.stderr));
    let audit: Value = serde_json::from_slice(&std::fs::read(audit).unwrap()).unwrap();
    assert_eq!(audit["schema"], "usd-toolbox.library-audit.v1");

    let revit = directory.path().join("material-revit.zip");
    let process = Command::new(env!("CARGO_BIN_EXE_usd-toolbox"))
        .args(["export", "--input"])
        .arg(&input)
        .args(["--target", "revit", "--tier", "preview", "--output"])
        .arg(&revit)
        .output()
        .unwrap();
    assert!(process.status.success(), "{}", String::from_utf8_lossy(&process.stderr));
    assert!(std::fs::read(revit).unwrap().starts_with(b"PK"));
}
