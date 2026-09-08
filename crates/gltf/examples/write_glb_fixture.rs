use std::path::PathBuf;

use usd_toolbox_core::{Exporter, Material};
use usd_toolbox_gltf::{GltfExportOptions, GltfExporter};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let output = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("target/usd-toolbox-fixture.glb"));

    let mut material = Material::new("reference_material", "Reference Material");
    material.surface.base_color = [0.32, 0.12, 0.04, 1.0].into();
    material.surface.base_metalness = 0.15.into();
    material.surface.specular_roughness = 0.42.into();
    material.surface.specular_ior = Some(1.5.into());
    material.surface.transmission_weight = Some(0.2.into());
    material.surface.transmission_thickness = Some(12.0.into());
    material.surface.specular_anisotropy = Some(0.1.into());
    material.surface.emission_color = Some([2.0, 0.5, 0.0].into());

    let export = GltfExporter.export(&[material], &GltfExportOptions::default())?;
    std::fs::write(&output, export.bytes)?;
    println!("{}", output.display());
    Ok(())
}
