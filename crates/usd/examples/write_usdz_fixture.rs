use std::collections::BTreeMap;
use std::path::PathBuf;

use usd_toolbox_core::{Exporter, Material, ParameterValue, Value, Variant, VariantSet};
use usd_toolbox_usd::{UsdExportOptions, UsdExporter};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let output = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("target/usd-toolbox-fixture.usdz"));

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

    let export = UsdExporter.export(&[material], &UsdExportOptions::default())?;
    std::fs::write(&output, export.bytes)?;
    println!("{}", output.display());
    Ok(())
}
