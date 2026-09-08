use std::path::PathBuf;

use usd_toolbox_core::{Exporter, Material};
use usd_toolbox_materialx::{MaterialXExportOptions, MaterialXExporter};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let output = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("target/usd-toolbox-fixture.mtlx"));

    let mut material = Material::new("reference_material", "Reference Material");
    material.surface.base_color = [0.32, 0.12, 0.04, 1.0].into();
    material.surface.base_metalness = 0.15.into();
    material.surface.specular_roughness = 0.42.into();
    material.surface.specular_ior = Some(1.5.into());

    let export = MaterialXExporter.export(&[material], &MaterialXExportOptions::default())?;
    std::fs::write(&output, export.bytes)?;
    println!("{}", output.display());
    Ok(())
}
