use usd_toolbox_core::{Exporter, Material};
use usd_toolbox_omniverse::{OmniverseExportOptions, OmniverseExporter};

#[test]
fn exports_an_omniverse_readable_usdz() {
    let export = OmniverseExporter
        .export(&[Material::new("paint", "Paint")], &OmniverseExportOptions::default())
        .unwrap();
    assert!(export.bytes.starts_with(b"PK"));
}
