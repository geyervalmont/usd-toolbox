//! Self-contained browser handoff: XML plus its explicitly allowed images.
use crate::{MaterialXExportOptions, MaterialXExporter};
use base64::Engine;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use usd_toolbox_core::{ExportError, Exporter, Loss, Material, Tier};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct MaterialXAsset {
    pub media_type: String,
    pub base64: String,
}
#[derive(Clone, Debug, Serialize)]
pub struct MaterialXBundle {
    pub schema: &'static str,
    pub document: String,
    pub material_names: Vec<String>,
    pub assets: BTreeMap<String, MaterialXAsset>,
    pub losses: Vec<Loss>,
}

/// Export a renderer-ready, bytes-only document. The consumer must resolve only
/// `assets` and must not fetch arbitrary filenames from the graph.
pub fn export_bundle(materials: &[Material], tier: Tier) -> Result<MaterialXBundle, ExportError> {
    let exported = MaterialXExporter.export(
        materials,
        &MaterialXExportOptions {
            graph_tier: tier,
            embed_payloads: false,
            include_neutral_manifest: false,
        },
    )?;
    let document = String::from_utf8(exported.bytes).expect("XML is UTF-8");
    let parsed = crate::parse_document(document.as_bytes()).map_err(|e| ExportError::InvalidModel(e.to_string()))?;
    let mut material_names: Vec<String> = parsed
        .children
        .iter()
        .filter(|n| n.category == "surfacematerial")
        .map(|n| n.attribute("name").to_owned())
        .collect();
    if materials.iter().all(|material| material.materialx.is_some()) {
        material_names = materials
            .iter()
            .map(|material| material.materialx.as_ref().expect("checked").material_name.clone())
            .collect();
    }
    let mut assets = BTreeMap::new();
    let mut total = 0;
    for material in materials {
        for (path, asset) in usd_toolbox_core::materialx_graph(material, tier)?.assets {
            if assets.contains_key(&path) {
                continue;
            }
            total += asset.bytes.len();
            if total > 64 * 1024 * 1024 {
                return Err(ExportError::Unsupported(
                    "MaterialX browser dependencies exceed 64 MiB".into(),
                ));
            }
            assets.insert(
                path,
                MaterialXAsset {
                    media_type: asset.media_type,
                    base64: base64::engine::general_purpose::STANDARD.encode(asset.bytes),
                },
            );
        }
    }
    Ok(MaterialXBundle {
        schema: "usd-toolbox.materialx-preview.v1",
        document,
        material_names,
        assets,
        losses: exported.losses,
    })
}
