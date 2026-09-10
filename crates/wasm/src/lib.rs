//! WebAssembly bindings for all bytes-in, bytes-out material operations.

use serde::de::DeserializeOwned;
use usd_toolbox_core::{Export, Exporter, Importer, Input, Material, Target, dry_run};
use usd_toolbox_gltf::{GltfExportOptions, GltfExporter};
use usd_toolbox_materials::{
    AuditProfile, EmbeddingOptions, EmbeddingRecord, TextureAttachment, TierPolicy, apply_merge_patch,
    attach_texture as attach_neutral_texture, audit_library, cluster_embeddings, diff_materials,
    extract_texture as extract_neutral_texture, generate_missing_downscales, inspect, migrate_document_json,
    plan_tiers, prepare_embedding_input, remove_texture as remove_neutral_texture,
};
use usd_toolbox_materialx::{MaterialXExportOptions, MaterialXExporter, MaterialXImportOptions, MaterialXImporter};
use usd_toolbox_omniverse::{OmniverseExportOptions, OmniverseExporter};
use usd_toolbox_procedural::{ProceduralDefinition, bake as bake_procedural_definition};
use usd_toolbox_revit::{RevitExportOptions, RevitExporter};
use usd_toolbox_textures::{TextureImportOptions, TextureInput, TextureSetImporter};
use usd_toolbox_usd::{UsdExportOptions, UsdExporter, UsdImportOptions, UsdImporter};
use wasm_bindgen::prelude::*;

/// Export bytes plus a typed JavaScript loss array.
#[wasm_bindgen]
pub struct WasmExport {
    bytes: Vec<u8>,
    losses: JsValue,
}

#[wasm_bindgen]
impl WasmExport {
    /// Returns encoded bytes as a JavaScript `Uint8Array` copy.
    #[wasm_bindgen(getter)]
    pub fn bytes(&self) -> Vec<u8> {
        self.bytes.clone()
    }

    /// Returns the deterministic structured loss report.
    #[wasm_bindgen(getter)]
    pub fn losses(&self) -> JsValue {
        self.losses.clone()
    }
}

/// Texture ingest result with a typed material and conversion report.
#[wasm_bindgen]
pub struct WasmTextureImport {
    material: JsValue,
    losses: JsValue,
}

#[wasm_bindgen]
impl WasmTextureImport {
    /// Returns the imported neutral material object.
    #[wasm_bindgen(getter)]
    pub fn material(&self) -> JsValue {
        self.material.clone()
    }

    /// Returns convention and tier-generation conversions.
    #[wasm_bindgen(getter)]
    pub fn losses(&self) -> JsValue {
        self.losses.clone()
    }
}

/// Imports encoded texture objects into one neutral material.
#[wasm_bindgen]
pub fn import_texture_set(inputs: JsValue, options: Option<JsValue>) -> Result<WasmTextureImport, JsError> {
    let inputs: Vec<TextureInput> = from_js(inputs)?;
    let options: TextureImportOptions = options_from_js(options)?;
    let result = TextureSetImporter.import_set(&inputs, &options).map_err(js_error)?;
    Ok(WasmTextureImport {
        material: to_js(&result.material)?,
        losses: to_js(&result.losses)?,
    })
}

/// Bakes a versioned procedural definition to deterministic PBR maps and hatches.
#[wasm_bindgen]
pub fn bake_procedural(definition: JsValue) -> Result<JsValue, JsError> {
    let definition: ProceduralDefinition = from_js(definition)?;
    to_js(&bake_procedural_definition(&definition).map_err(js_error)?)
}

/// Exports neutral materials as USDA, USDC, or USDZ.
#[wasm_bindgen]
pub fn export_usd(materials: JsValue, options: Option<JsValue>) -> Result<WasmExport, JsError> {
    export_with(&materials, &options_from_js::<UsdExportOptions>(options)?, &UsdExporter)
}

/// Imports a USDA, USDC, or USDZ byte buffer.
#[wasm_bindgen]
pub fn import_usd(bytes: &[u8], options: Option<JsValue>) -> Result<JsValue, JsError> {
    let options: UsdImportOptions = options_from_js(options)?;
    let materials = UsdImporter.import(Input::Bytes(bytes), &options).map_err(js_error)?;
    to_js(&materials)
}

/// Exports neutral materials as a MaterialX 1.39 document.
#[wasm_bindgen]
pub fn export_materialx(materials: JsValue, options: Option<JsValue>) -> Result<WasmExport, JsError> {
    export_with(
        &materials,
        &options_from_js::<MaterialXExportOptions>(options)?,
        &MaterialXExporter,
    )
}

/// Imports a MaterialX document carrying an Olsyn neutral manifest.
#[wasm_bindgen]
pub fn import_materialx(bytes: &[u8], options: Option<JsValue>) -> Result<JsValue, JsError> {
    let options: MaterialXImportOptions = options_from_js(options)?;
    let materials = MaterialXImporter
        .import(Input::Bytes(bytes), &options)
        .map_err(js_error)?;
    to_js(&materials)
}

/// Exports neutral materials as glTF JSON or GLB.
#[wasm_bindgen]
pub fn export_gltf(materials: JsValue, options: Option<JsValue>) -> Result<WasmExport, JsError> {
    export_with(
        &materials,
        &options_from_js::<GltfExportOptions>(options)?,
        &GltfExporter,
    )
}

/// Exports the exact Generic-schema image set consumed by the OPAL Revit connector.
#[wasm_bindgen]
pub fn export_revit(materials: JsValue, options: Option<JsValue>) -> Result<WasmExport, JsError> {
    export_with(
        &materials,
        &options_from_js::<RevitExportOptions>(options)?,
        &RevitExporter,
    )
}

/// Exports an NVIDIA Omniverse-compatible USDZ package.
#[wasm_bindgen]
pub fn export_omniverse(materials: JsValue, options: Option<JsValue>) -> Result<WasmExport, JsError> {
    export_with(
        &materials,
        &options_from_js::<OmniverseExportOptions>(options)?,
        &OmniverseExporter,
    )
}

/// Returns compact material summaries without serializing texture payloads.
#[wasm_bindgen]
pub fn inspect_materials(materials: JsValue) -> Result<JsValue, JsError> {
    let materials: Vec<Material> = from_js(materials)?;
    to_js(&inspect(&materials))
}

/// Runs content and completeness audits against each neutral material.
#[wasm_bindgen]
pub fn audit_materials(materials: JsValue, profile: Option<JsValue>) -> Result<JsValue, JsError> {
    let materials: Vec<Material> = from_js(materials)?;
    let profile: AuditProfile = options_from_js(profile)?;
    to_js(&audit_library(&materials, &profile))
}

/// Applies a validated JSON merge patch to one material.
#[wasm_bindgen]
pub fn apply_material_patch(material: JsValue, patch: JsValue) -> Result<JsValue, JsError> {
    let material: Material = from_js(material)?;
    let patch: serde_json::Value = from_js(patch)?;
    to_js(&apply_merge_patch(&material, &patch).map_err(js_error)?)
}

/// Returns stable field-level changes between two neutral materials.
#[wasm_bindgen]
pub fn diff_material(before: JsValue, after: JsValue) -> Result<JsValue, JsError> {
    let before: Material = from_js(before)?;
    let after: Material = from_js(after)?;
    to_js(&diff_materials(&before, &after).map_err(js_error)?)
}

/// Plans downscale, external-generation, and missing-role texture work.
#[wasm_bindgen]
pub fn plan_material_tiers(material: JsValue, policy: Option<JsValue>) -> Result<JsValue, JsError> {
    let material: Material = from_js(material)?;
    let policy: TierPolicy = options_from_js(policy)?;
    to_js(&plan_tiers(&material, &policy))
}

/// Performs safe missing downscales and returns the updated material plus jobs
/// which still require an external producer.
#[wasm_bindgen]
pub fn complete_material_tiers(material: JsValue, policy: Option<JsValue>, created: &str) -> Result<JsValue, JsError> {
    let mut material: Material = from_js(material)?;
    let policy: TierPolicy = options_from_js(policy)?;
    let created =
        time::OffsetDateTime::parse(created, &time::format_description::well_known::Rfc3339).map_err(js_error)?;
    let attached_hashes = generate_missing_downscales(&mut material, &policy, created).map_err(js_error)?;
    let remaining_jobs = plan_tiers(&material, &policy);
    to_js(&serde_json::json!({
        "material": material,
        "attached_hashes": attached_hashes,
        "remaining_jobs": remaining_jobs,
    }))
}

/// Attaches a caller-produced texture using the same contract for AI and manual work.
#[wasm_bindgen]
pub fn attach_texture(material: JsValue, attachment: JsValue) -> Result<JsValue, JsError> {
    let mut material: Material = from_js(material)?;
    let attachment: TextureAttachment = from_js(attachment)?;
    attach_neutral_texture(&mut material, attachment).map_err(js_error)?;
    to_js(&material)
}

/// Removes one exact role/tier texture.
#[wasm_bindgen]
pub fn remove_texture(material: JsValue, role: JsValue, tier: JsValue) -> Result<JsValue, JsError> {
    let mut material: Material = from_js(material)?;
    let role = from_js(role)?;
    let tier = from_js(tier)?;
    remove_neutral_texture(&mut material, role, tier).map_err(js_error)?;
    to_js(&material)
}

/// Extracts one exact encoded role/tier payload.
#[wasm_bindgen]
pub fn extract_texture(material: JsValue, role: JsValue, tier: JsValue) -> Result<Vec<u8>, JsError> {
    let material: Material = from_js(material)?;
    let role = from_js(role)?;
    let tier = from_js(tier)?;
    extract_neutral_texture(&material, role, tier)
        .ok_or_else(|| JsError::new("requested texture role/tier was not found"))
}

/// Migrates legacy raw neutral JSON into the current versioned envelope.
#[wasm_bindgen]
pub fn migrate_material_document(bytes: &[u8]) -> Result<JsValue, JsError> {
    to_js(&migrate_document_json(bytes).map_err(js_error)?)
}

/// Creates stable visual/text/structured input for caller-selected models.
#[wasm_bindgen]
pub fn prepare_material_embedding(material: JsValue, options: Option<JsValue>) -> Result<JsValue, JsError> {
    let material: Material = from_js(material)?;
    let options: EmbeddingOptions = options_from_js(options)?;
    to_js(&prepare_embedding_input(&material, &options))
}

/// Clusters caller-produced vectors without coupling the toolbox to a model provider.
#[wasm_bindgen]
pub fn cluster_material_embeddings(
    records: JsValue,
    similarity_threshold: f32,
    min_cluster_size: usize,
) -> Result<JsValue, JsError> {
    let records: Vec<EmbeddingRecord> = from_js(records)?;
    to_js(&cluster_embeddings(&records, similarity_threshold, min_cluster_size).map_err(js_error)?)
}

/// Computes a target loss report without writing bytes.
#[wasm_bindgen]
pub fn dry_run_export(materials: JsValue, target: &str) -> Result<JsValue, JsError> {
    let materials: Vec<Material> = from_js(materials)?;
    let target = match target {
        "usd" | "usdz" | "usda" | "usdc" => Target::Usd,
        "materialx" | "mtlx" => Target::MaterialX,
        "gltf" | "glb" => Target::Gltf,
        "revit" => Target::Revit,
        "omniverse" => Target::Omniverse,
        _ => return Err(JsError::new(&format!("unknown export target `{target}`"))),
    };
    to_js(&dry_run(&materials, target))
}

fn export_with<E: Exporter>(materials: &JsValue, options: &E::Options, exporter: &E) -> Result<WasmExport, JsError> {
    let materials: Vec<Material> = from_js(materials.clone())?;
    exporter
        .export(&materials, options)
        .map_err(js_error)
        .and_then(wasm_export)
}

fn wasm_export(export: Export) -> Result<WasmExport, JsError> {
    Ok(WasmExport {
        bytes: export.bytes,
        losses: to_js(&export.losses)?,
    })
}

fn options_from_js<T: Default + DeserializeOwned>(value: Option<JsValue>) -> Result<T, JsError> {
    match value {
        Some(value) if !value.is_null() && !value.is_undefined() => from_js(value),
        Some(_) | None => Ok(T::default()),
    }
}

fn from_js<T: DeserializeOwned>(value: JsValue) -> Result<T, JsError> {
    serde_wasm_bindgen::from_value(value).map_err(js_error)
}

fn to_js<T: serde::Serialize + ?Sized>(value: &T) -> Result<JsValue, JsError> {
    serde_wasm_bindgen::to_value(value).map_err(js_error)
}

fn js_error(error: impl std::fmt::Display) -> JsError {
    JsError::new(&error.to_string())
}
