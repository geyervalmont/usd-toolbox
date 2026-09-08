//! Panic-safe C ABI for Laravel workers and other native hosts.
//!
//! Caller-allocated entry points use a two-pass contract: call with null output
//! buffers to receive required sizes, allocate, then call again. Convenience
//! allocation entry points return `UsdToolboxBuffer` values which must be released by
//! `usd_toolbox_buffer_free`. Inputs and structured outputs use UTF-8 JSON; binary
//! target documents remain raw bytes.

use std::cell::RefCell;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr;
use std::slice;

use serde::Deserialize;
use usd_toolbox_core::{Export, Exporter, Importer, Input, Material, Target, dry_run};
use usd_toolbox_gltf::{GltfExportOptions, GltfExporter, GltfFormat};
use usd_toolbox_materials::{
    AuditProfile, EmbeddingOptions, EmbeddingRecord, MaterialWorkflowError, TextureAttachment, TierPolicy,
    apply_merge_patch, attach_texture, audit_library, cluster_embeddings, diff_materials, extract_texture,
    generate_missing_downscales, inspect, migrate_document_json, plan_tiers, prepare_embedding_input, remove_texture,
};
use usd_toolbox_materialx::{MaterialXExportOptions, MaterialXExporter, MaterialXImportOptions, MaterialXImporter};
use usd_toolbox_omniverse::{OmniverseExportOptions, OmniverseExporter};
use usd_toolbox_revit::{RevitExportOptions, RevitExporter};
use usd_toolbox_textures::{TextureImportOptions, TextureInput, TextureSetImporter};
use usd_toolbox_usd::{UsdExportOptions, UsdExporter, UsdFormat, UsdImportOptions, UsdImporter};

/// Successful result.
pub const USD_TOOLBOX_STATUS_OK: UsdToolboxStatus = 0;
/// Null pointer, invalid UTF-8/JSON, or unknown target.
pub const USD_TOOLBOX_STATUS_INVALID_ARGUMENT: UsdToolboxStatus = 1;
/// One or more caller output buffers are too small. Required lengths are set.
pub const USD_TOOLBOX_STATUS_BUFFER_TOO_SMALL: UsdToolboxStatus = 2;
/// Source bytes could not be imported.
pub const USD_TOOLBOX_STATUS_IMPORT_ERROR: UsdToolboxStatus = 3;
/// Neutral data could not be exported.
pub const USD_TOOLBOX_STATUS_EXPORT_ERROR: UsdToolboxStatus = 4;
/// A panic was caught at the ABI boundary.
pub const USD_TOOLBOX_STATUS_PANIC: UsdToolboxStatus = 255;

/// C ABI status code.
pub type UsdToolboxStatus = u32;
/// C ABI target selector.
pub type UsdToolboxTarget = u32;

/// USDZ package target.
pub const USD_TOOLBOX_TARGET_USDZ: UsdToolboxTarget = 1;
/// USDA text target.
pub const USD_TOOLBOX_TARGET_USDA: UsdToolboxTarget = 2;
/// USDC crate target.
pub const USD_TOOLBOX_TARGET_USDC: UsdToolboxTarget = 3;
/// MaterialX document target.
pub const USD_TOOLBOX_TARGET_MATERIALX: UsdToolboxTarget = 4;
/// JSON glTF target.
pub const USD_TOOLBOX_TARGET_GLTF: UsdToolboxTarget = 5;
/// Binary GLB target.
pub const USD_TOOLBOX_TARGET_GLB: UsdToolboxTarget = 6;
/// Autodesk Revit Generic appearance image-set ZIP target.
pub const USD_TOOLBOX_TARGET_REVIT: UsdToolboxTarget = 7;
/// NVIDIA Omniverse-compatible USDZ target.
pub const USD_TOOLBOX_TARGET_OMNIVERSE: UsdToolboxTarget = 8;

/// Library-owned byte allocation.
#[repr(C)]
#[derive(Debug)]
pub struct UsdToolboxBuffer {
    /// First byte, or null for an empty/default buffer.
    pub data: *mut u8,
    /// Initialized byte count.
    pub len: usize,
    /// Allocation capacity needed by the matching Rust deallocator.
    pub capacity: usize,
}

impl Default for UsdToolboxBuffer {
    fn default() -> Self {
        Self {
            data: ptr::null_mut(),
            len: 0,
            capacity: 0,
        }
    }
}

thread_local! {
    static LAST_ERROR: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
}

/// Exports neutral materials using caller-allocated output and loss buffers.
///
/// `materials_json` must encode an array of neutral `Material` objects.
/// `options_json` may be null/empty to select defaults. `required_output` and
/// `required_losses` must be valid pointers. No partial output is written.
///
/// # Safety
///
/// Every non-null pointer must be valid for its accompanying length/capacity,
/// and the two required-length pointers must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn usd_toolbox_export(
    materials_json: *const u8,
    materials_len: usize,
    target: UsdToolboxTarget,
    options_json: *const u8,
    options_len: usize,
    output: *mut u8,
    output_capacity: usize,
    required_output: *mut usize,
    losses_json: *mut u8,
    losses_capacity: usize,
    required_losses: *mut usize,
) -> UsdToolboxStatus {
    boundary(|| {
        let materials_bytes = unsafe { input_slice(materials_json, materials_len, "materials_json")? };
        let option_bytes = unsafe { input_slice(options_json, options_len, "options_json")? };
        let materials: Vec<Material> = parse_json(materials_bytes, "materials_json")?;
        let export = execute_export(&materials, target, option_bytes)?;
        let losses = serde_json::to_vec(&export.losses).map_err(invalid_json)?;
        unsafe {
            write_pair(
                &export.bytes,
                output,
                output_capacity,
                required_output,
                &losses,
                losses_json,
                losses_capacity,
                required_losses,
            )
        }
    })
}

/// Exports neutral materials into library-owned buffers.
///
/// Both returned buffers must be released with `usd_toolbox_buffer_free`.
///
/// # Safety
///
/// Input pointers must be valid for their lengths. Both output struct pointers
/// must be distinct, valid for writes, and either uninitialized or empty.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn usd_toolbox_export_alloc(
    materials_json: *const u8,
    materials_len: usize,
    target: UsdToolboxTarget,
    options_json: *const u8,
    options_len: usize,
    output: *mut UsdToolboxBuffer,
    losses_json: *mut UsdToolboxBuffer,
) -> UsdToolboxStatus {
    boundary(|| {
        if output.is_null() || losses_json.is_null() {
            return Err(FfiError::invalid("output buffer structs must not be null"));
        }
        if output == losses_json {
            return Err(FfiError::invalid("output buffer structs must be distinct"));
        }
        let materials_bytes = unsafe { input_slice(materials_json, materials_len, "materials_json")? };
        let option_bytes = unsafe { input_slice(options_json, options_len, "options_json")? };
        let materials: Vec<Material> = parse_json(materials_bytes, "materials_json")?;
        let export = execute_export(&materials, target, option_bytes)?;
        let losses = serde_json::to_vec(&export.losses).map_err(invalid_json)?;
        unsafe {
            output.write(into_buffer(export.bytes));
            losses_json.write(into_buffer(losses));
        }
        Ok(USD_TOOLBOX_STATUS_OK)
    })
}

/// Imports USD/USDZ bytes and writes a neutral-material JSON array.
///
/// # Safety
///
/// Input pointers must be valid for their lengths. The output pointer must be
/// valid for its capacity and `required_output` must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn usd_toolbox_import_usd(
    input: *const u8,
    input_len: usize,
    options_json: *const u8,
    options_len: usize,
    output_json: *mut u8,
    output_capacity: usize,
    required_output: *mut usize,
) -> UsdToolboxStatus {
    boundary(|| {
        let bytes = unsafe { input_slice(input, input_len, "input")? };
        let option_bytes = unsafe { input_slice(options_json, options_len, "options_json")? };
        let options: UsdImportOptions = parse_options(option_bytes)?;
        let materials = UsdImporter
            .import(Input::Bytes(bytes), &options)
            .map_err(FfiError::import)?;
        let json = serde_json::to_vec(&materials).map_err(invalid_json)?;
        unsafe { write_single(&json, output_json, output_capacity, required_output) }
    })
}

/// Imports a MaterialX document and writes a neutral-material JSON array.
///
/// # Safety
///
/// Input pointers must be valid for their lengths. The output pointer must be
/// valid for its capacity and `required_output` must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn usd_toolbox_import_materialx(
    input: *const u8,
    input_len: usize,
    options_json: *const u8,
    options_len: usize,
    output_json: *mut u8,
    output_capacity: usize,
    required_output: *mut usize,
) -> UsdToolboxStatus {
    boundary(|| {
        let bytes = unsafe { input_slice(input, input_len, "input")? };
        let option_bytes = unsafe { input_slice(options_json, options_len, "options_json")? };
        let options: MaterialXImportOptions = parse_options(option_bytes)?;
        let materials = MaterialXImporter
            .import(Input::Bytes(bytes), &options)
            .map_err(FfiError::import)?;
        let json = serde_json::to_vec(&materials).map_err(invalid_json)?;
        unsafe { write_single(&json, output_json, output_capacity, required_output) }
    })
}

/// Imports a JSON array of texture inputs and writes one material plus losses.
///
/// # Safety
///
/// Input pointers must be valid for their lengths. The output pointer must be
/// valid for its capacity and `required_output` must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn usd_toolbox_import_texture_set(
    inputs_json: *const u8,
    inputs_len: usize,
    options_json: *const u8,
    options_len: usize,
    output_json: *mut u8,
    output_capacity: usize,
    required_output: *mut usize,
) -> UsdToolboxStatus {
    boundary(|| {
        let inputs_bytes = unsafe { input_slice(inputs_json, inputs_len, "inputs_json")? };
        let option_bytes = unsafe { input_slice(options_json, options_len, "options_json")? };
        let inputs: Vec<TextureInput> = parse_json(inputs_bytes, "inputs_json")?;
        let options: TextureImportOptions = parse_options(option_bytes)?;
        let result = TextureSetImporter
            .import_set(&inputs, &options)
            .map_err(FfiError::import)?;
        let json = serde_json::to_vec(&serde_json::json!({
            "material": result.material,
            "losses": result.losses,
        }))
        .map_err(invalid_json)?;
        unsafe { write_single(&json, output_json, output_capacity, required_output) }
    })
}

/// Computes a target loss report without writing a target document.
///
/// # Safety
///
/// The input pointer must be valid for its length. The output pointer must be
/// valid for its capacity and `required_output` must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn usd_toolbox_dry_run(
    materials_json: *const u8,
    materials_len: usize,
    target: UsdToolboxTarget,
    output_json: *mut u8,
    output_capacity: usize,
    required_output: *mut usize,
) -> UsdToolboxStatus {
    boundary(|| {
        let materials_bytes = unsafe { input_slice(materials_json, materials_len, "materials_json")? };
        let materials: Vec<Material> = parse_json(materials_bytes, "materials_json")?;
        let target = core_target(target)?;
        let json = serde_json::to_vec(&dry_run(&materials, target)).map_err(invalid_json)?;
        unsafe { write_single(&json, output_json, output_capacity, required_output) }
    })
}

/// Inspects neutral materials and writes compact JSON without texture bytes.
///
/// # Safety
///
/// Input/output pointers follow the same two-pass rules as `usd_toolbox_dry_run`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn usd_toolbox_material_inspect(
    materials_json: *const u8,
    materials_len: usize,
    output_json: *mut u8,
    output_capacity: usize,
    required_output: *mut usize,
) -> UsdToolboxStatus {
    boundary(|| {
        let bytes = unsafe { input_slice(materials_json, materials_len, "materials_json")? };
        let materials: Vec<Material> = parse_json(bytes, "materials_json")?;
        let json = serde_json::to_vec(&inspect(&materials)).map_err(invalid_json)?;
        unsafe { write_single(&json, output_json, output_capacity, required_output) }
    })
}

/// Audits neutral materials. Empty `profile_json` selects portable-PBR defaults.
///
/// # Safety
///
/// Every non-null pointer must be valid for its supplied length/capacity.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn usd_toolbox_material_audit(
    materials_json: *const u8,
    materials_len: usize,
    profile_json: *const u8,
    profile_len: usize,
    output_json: *mut u8,
    output_capacity: usize,
    required_output: *mut usize,
) -> UsdToolboxStatus {
    boundary(|| {
        let materials_bytes = unsafe { input_slice(materials_json, materials_len, "materials_json")? };
        let profile_bytes = unsafe { input_slice(profile_json, profile_len, "profile_json")? };
        let materials: Vec<Material> = parse_json(materials_bytes, "materials_json")?;
        let profile: AuditProfile = parse_options(profile_bytes)?;
        let json = serde_json::to_vec(&audit_library(&materials, &profile)).map_err(invalid_json)?;
        unsafe { write_single(&json, output_json, output_capacity, required_output) }
    })
}

/// Applies a validated JSON merge patch to one neutral material.
///
/// # Safety
///
/// Every non-null pointer must be valid for its supplied length/capacity.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn usd_toolbox_material_patch(
    material_json: *const u8,
    material_len: usize,
    patch_json: *const u8,
    patch_len: usize,
    output_json: *mut u8,
    output_capacity: usize,
    required_output: *mut usize,
) -> UsdToolboxStatus {
    boundary(|| {
        let material_bytes = unsafe { input_slice(material_json, material_len, "material_json")? };
        let patch_bytes = unsafe { input_slice(patch_json, patch_len, "patch_json")? };
        let material: Material = parse_json(material_bytes, "material_json")?;
        let patch: serde_json::Value = parse_json(patch_bytes, "patch_json")?;
        let updated = apply_merge_patch(&material, &patch).map_err(workflow_error)?;
        let json = serde_json::to_vec(&updated).map_err(invalid_json)?;
        unsafe { write_single(&json, output_json, output_capacity, required_output) }
    })
}

/// Diffs two neutral materials at stable field paths.
///
/// # Safety
///
/// Every non-null pointer must be valid for its supplied length/capacity.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn usd_toolbox_material_diff(
    before_json: *const u8,
    before_len: usize,
    after_json: *const u8,
    after_len: usize,
    output_json: *mut u8,
    output_capacity: usize,
    required_output: *mut usize,
) -> UsdToolboxStatus {
    boundary(|| {
        let before_bytes = unsafe { input_slice(before_json, before_len, "before_json")? };
        let after_bytes = unsafe { input_slice(after_json, after_len, "after_json")? };
        let before: Material = parse_json(before_bytes, "before_json")?;
        let after: Material = parse_json(after_bytes, "after_json")?;
        let changes = diff_materials(&before, &after).map_err(workflow_error)?;
        let json = serde_json::to_vec(&changes).map_err(invalid_json)?;
        unsafe { write_single(&json, output_json, output_capacity, required_output) }
    })
}

/// Plans missing texture-tier work without invoking an external generator.
///
/// # Safety
///
/// Every non-null pointer must be valid for its supplied length/capacity.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn usd_toolbox_material_plan_tiers(
    material_json: *const u8,
    material_len: usize,
    policy_json: *const u8,
    policy_len: usize,
    output_json: *mut u8,
    output_capacity: usize,
    required_output: *mut usize,
) -> UsdToolboxStatus {
    boundary(|| {
        let material_bytes = unsafe { input_slice(material_json, material_len, "material_json")? };
        let policy_bytes = unsafe { input_slice(policy_json, policy_len, "policy_json")? };
        let material: Material = parse_json(material_bytes, "material_json")?;
        let policy: TierPolicy = parse_options(policy_bytes)?;
        let json = serde_json::to_vec(&plan_tiers(&material, &policy)).map_err(invalid_json)?;
        unsafe { write_single(&json, output_json, output_capacity, required_output) }
    })
}

#[derive(Deserialize)]
struct CompleteTiersRequest {
    material: Material,
    #[serde(default)]
    policy: TierPolicy,
    #[serde(with = "time::serde::rfc3339")]
    created: time::OffsetDateTime,
}

/// Performs every safe downscale and returns remaining external-generation jobs.
///
/// # Safety
///
/// Every non-null pointer must be valid for its supplied length/capacity.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn usd_toolbox_material_complete_tiers(
    request_json: *const u8,
    request_len: usize,
    output_json: *mut u8,
    output_capacity: usize,
    required_output: *mut usize,
) -> UsdToolboxStatus {
    boundary(|| {
        let bytes = unsafe { input_slice(request_json, request_len, "request_json")? };
        let mut request: CompleteTiersRequest = parse_json(bytes, "request_json")?;
        let attached_hashes = generate_missing_downscales(&mut request.material, &request.policy, request.created)
            .map_err(workflow_error)?;
        let remaining_jobs = plan_tiers(&request.material, &request.policy);
        let json = serde_json::to_vec(&serde_json::json!({
            "material": request.material,
            "attached_hashes": attached_hashes,
            "remaining_jobs": remaining_jobs,
        }))
        .map_err(invalid_json)?;
        unsafe { write_single(&json, output_json, output_capacity, required_output) }
    })
}

#[derive(Deserialize)]
struct AttachRequest {
    material: Material,
    attachment: TextureAttachment,
}

/// Attaches a generated or manually authored texture using declared lineage.
///
/// `request_json` is `{ "material": Material, "attachment": TextureAttachment }`.
///
/// # Safety
///
/// Every non-null pointer must be valid for its supplied length/capacity.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn usd_toolbox_material_attach_texture(
    request_json: *const u8,
    request_len: usize,
    output_json: *mut u8,
    output_capacity: usize,
    required_output: *mut usize,
) -> UsdToolboxStatus {
    boundary(|| {
        let bytes = unsafe { input_slice(request_json, request_len, "request_json")? };
        let mut request: AttachRequest = parse_json(bytes, "request_json")?;
        let hash = attach_texture(&mut request.material, request.attachment).map_err(workflow_error)?;
        let json = serde_json::to_vec(&serde_json::json!({
            "material": request.material,
            "attached_hash": hash,
        }))
        .map_err(invalid_json)?;
        unsafe { write_single(&json, output_json, output_capacity, required_output) }
    })
}

#[derive(Deserialize)]
struct RemoveRequest {
    material: Material,
    role: usd_toolbox_core::MapRole,
    tier: usd_toolbox_core::Tier,
}

/// Extracts one exact encoded texture role/tier.
///
/// `request_json` has the same `material`, `role`, and `tier` fields as remove.
///
/// # Safety
///
/// Every non-null pointer must be valid for its supplied length/capacity.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn usd_toolbox_material_extract_texture(
    request_json: *const u8,
    request_len: usize,
    output: *mut u8,
    output_capacity: usize,
    required_output: *mut usize,
) -> UsdToolboxStatus {
    boundary(|| {
        let bytes = unsafe { input_slice(request_json, request_len, "request_json")? };
        let request: RemoveRequest = parse_json(bytes, "request_json")?;
        let bytes = extract_texture(&request.material, request.role, request.tier)
            .ok_or_else(|| FfiError::invalid("requested texture role/tier was not found"))?;
        unsafe { write_single(&bytes, output, output_capacity, required_output) }
    })
}

/// Removes one exact texture role/tier from a neutral material.
///
/// # Safety
///
/// Every non-null pointer must be valid for its supplied length/capacity.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn usd_toolbox_material_remove_texture(
    request_json: *const u8,
    request_len: usize,
    output_json: *mut u8,
    output_capacity: usize,
    required_output: *mut usize,
) -> UsdToolboxStatus {
    boundary(|| {
        let bytes = unsafe { input_slice(request_json, request_len, "request_json")? };
        let mut request: RemoveRequest = parse_json(bytes, "request_json")?;
        remove_texture(&mut request.material, request.role, request.tier).map_err(workflow_error)?;
        let json = serde_json::to_vec(&request.material).map_err(invalid_json)?;
        unsafe { write_single(&json, output_json, output_capacity, required_output) }
    })
}

/// Builds deterministic input for caller-selected visual/text embedding models.
///
/// # Safety
///
/// Every non-null pointer must be valid for its supplied length/capacity.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn usd_toolbox_material_embedding_input(
    material_json: *const u8,
    material_len: usize,
    options_json: *const u8,
    options_len: usize,
    output_json: *mut u8,
    output_capacity: usize,
    required_output: *mut usize,
) -> UsdToolboxStatus {
    boundary(|| {
        let material_bytes = unsafe { input_slice(material_json, material_len, "material_json")? };
        let option_bytes = unsafe { input_slice(options_json, options_len, "options_json")? };
        let material: Material = parse_json(material_bytes, "material_json")?;
        let options: EmbeddingOptions = parse_options(option_bytes)?;
        let json = serde_json::to_vec(&prepare_embedding_input(&material, &options)).map_err(invalid_json)?;
        unsafe { write_single(&json, output_json, output_capacity, required_output) }
    })
}

#[derive(Deserialize)]
struct ClusterOptions {
    #[serde(default = "default_similarity_threshold")]
    similarity_threshold: f32,
    #[serde(default = "default_min_cluster_size")]
    min_cluster_size: usize,
}

impl Default for ClusterOptions {
    fn default() -> Self {
        Self {
            similarity_threshold: default_similarity_threshold(),
            min_cluster_size: default_min_cluster_size(),
        }
    }
}

const fn default_similarity_threshold() -> f32 {
    0.85
}

const fn default_min_cluster_size() -> usize {
    3
}

/// Clusters caller-produced, model-versioned embeddings.
///
/// # Safety
///
/// Every non-null pointer must be valid for its supplied length/capacity.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn usd_toolbox_material_cluster(
    records_json: *const u8,
    records_len: usize,
    options_json: *const u8,
    options_len: usize,
    output_json: *mut u8,
    output_capacity: usize,
    required_output: *mut usize,
) -> UsdToolboxStatus {
    boundary(|| {
        let record_bytes = unsafe { input_slice(records_json, records_len, "records_json")? };
        let option_bytes = unsafe { input_slice(options_json, options_len, "options_json")? };
        let records: Vec<EmbeddingRecord> = parse_json(record_bytes, "records_json")?;
        let options: ClusterOptions = parse_options(option_bytes)?;
        let result = cluster_embeddings(&records, options.similarity_threshold, options.min_cluster_size)
            .map_err(workflow_error)?;
        let json = serde_json::to_vec(&result).map_err(invalid_json)?;
        unsafe { write_single(&json, output_json, output_capacity, required_output) }
    })
}

/// Migrates legacy neutral JSON into the current versioned document envelope.
///
/// # Safety
///
/// Every non-null pointer must be valid for its supplied length/capacity.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn usd_toolbox_material_migrate(
    input_json: *const u8,
    input_len: usize,
    output_json: *mut u8,
    output_capacity: usize,
    required_output: *mut usize,
) -> UsdToolboxStatus {
    boundary(|| {
        let bytes = unsafe { input_slice(input_json, input_len, "input_json")? };
        let document = migrate_document_json(bytes).map_err(workflow_error)?;
        let json = serde_json::to_vec(&document).map_err(invalid_json)?;
        unsafe { write_single(&json, output_json, output_capacity, required_output) }
    })
}

/// Copies the current thread's last error message using the two-pass buffer contract.
///
/// # Safety
///
/// The output pointer must be valid for its capacity and `required_output` must
/// be writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn usd_toolbox_last_error(
    output: *mut u8,
    output_capacity: usize,
    required_output: *mut usize,
) -> UsdToolboxStatus {
    match catch_unwind(AssertUnwindSafe(|| {
        LAST_ERROR.with(|last| {
            let error = last.borrow();
            unsafe { write_single_raw(&error, output, output_capacity, required_output) }
        })
    })) {
        Ok(status) => status,
        Err(_) => USD_TOOLBOX_STATUS_PANIC,
    }
}

/// Releases one library-owned buffer and resets the struct to empty.
///
/// Passing null, or an already-empty buffer, is safe. Passing a copied buffer
/// struct more than once is a caller error and may double-free.
///
/// # Safety
///
/// A non-null pointer must refer to the original `UsdToolboxBuffer` returned by this
/// library and must not be freed concurrently or more than once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn usd_toolbox_buffer_free(buffer: *mut UsdToolboxBuffer) {
    if buffer.is_null() {
        return;
    }
    let buffer = unsafe { &mut *buffer };
    if !buffer.data.is_null() {
        unsafe {
            drop(Vec::from_raw_parts(buffer.data, buffer.len, buffer.capacity));
        }
    }
    *buffer = UsdToolboxBuffer::default();
}

/// Returns the static library semantic version bytes. The pointer must not be freed.
///
/// # Safety
///
/// A non-null `length` pointer must be valid for a `usize` write.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn usd_toolbox_version(length: *mut usize) -> *const u8 {
    if !length.is_null() {
        unsafe {
            *length = env!("CARGO_PKG_VERSION").len();
        }
    }
    env!("CARGO_PKG_VERSION").as_ptr()
}

fn execute_export(materials: &[Material], target: UsdToolboxTarget, options: &[u8]) -> Result<Export, FfiError> {
    match target {
        USD_TOOLBOX_TARGET_USDZ | USD_TOOLBOX_TARGET_USDA | USD_TOOLBOX_TARGET_USDC => {
            let mut options: UsdExportOptions = parse_options(options)?;
            options.format = match target {
                USD_TOOLBOX_TARGET_USDZ => UsdFormat::Usdz,
                USD_TOOLBOX_TARGET_USDA => UsdFormat::Usda,
                USD_TOOLBOX_TARGET_USDC => UsdFormat::Usdc,
                _ => unreachable!(),
            };
            UsdExporter.export(materials, &options).map_err(FfiError::export)
        }
        USD_TOOLBOX_TARGET_MATERIALX => {
            let options: MaterialXExportOptions = parse_options(options)?;
            MaterialXExporter.export(materials, &options).map_err(FfiError::export)
        }
        USD_TOOLBOX_TARGET_GLTF | USD_TOOLBOX_TARGET_GLB => {
            let mut options: GltfExportOptions = parse_options(options)?;
            options.format = if target == USD_TOOLBOX_TARGET_GLTF {
                GltfFormat::Gltf
            } else {
                GltfFormat::Glb
            };
            GltfExporter.export(materials, &options).map_err(FfiError::export)
        }
        USD_TOOLBOX_TARGET_REVIT => {
            let options: RevitExportOptions = parse_options(options)?;
            RevitExporter.export(materials, &options).map_err(FfiError::export)
        }
        USD_TOOLBOX_TARGET_OMNIVERSE => {
            let options: OmniverseExportOptions = parse_options(options)?;
            OmniverseExporter.export(materials, &options).map_err(FfiError::export)
        }
        _ => Err(FfiError::invalid(format!("unknown target {target}"))),
    }
}

fn core_target(target: UsdToolboxTarget) -> Result<Target, FfiError> {
    match target {
        USD_TOOLBOX_TARGET_USDZ | USD_TOOLBOX_TARGET_USDA | USD_TOOLBOX_TARGET_USDC => Ok(Target::Usd),
        USD_TOOLBOX_TARGET_MATERIALX => Ok(Target::MaterialX),
        USD_TOOLBOX_TARGET_GLTF | USD_TOOLBOX_TARGET_GLB => Ok(Target::Gltf),
        USD_TOOLBOX_TARGET_REVIT => Ok(Target::Revit),
        USD_TOOLBOX_TARGET_OMNIVERSE => Ok(Target::Omniverse),
        _ => Err(FfiError::invalid(format!("unknown target {target}"))),
    }
}

fn parse_options<T: Default + serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T, FfiError> {
    if bytes.is_empty() {
        Ok(T::default())
    } else {
        parse_json(bytes, "options_json")
    }
}

fn parse_json<T: serde::de::DeserializeOwned>(bytes: &[u8], name: &str) -> Result<T, FfiError> {
    serde_json::from_slice(bytes).map_err(|error| FfiError::invalid(format!("invalid {name}: {error}")))
}

fn invalid_json(error: serde_json::Error) -> FfiError {
    FfiError::invalid(error.to_string())
}

fn workflow_error(error: MaterialWorkflowError) -> FfiError {
    FfiError::invalid(error.to_string())
}

unsafe fn input_slice<'a>(pointer: *const u8, length: usize, name: &str) -> Result<&'a [u8], FfiError> {
    if length == 0 {
        return Ok(&[]);
    }
    if pointer.is_null() {
        return Err(FfiError::invalid(format!("{name} is null but length is {length}")));
    }
    Ok(unsafe { slice::from_raw_parts(pointer, length) })
}

#[allow(clippy::too_many_arguments)]
unsafe fn write_pair(
    first: &[u8],
    first_output: *mut u8,
    first_capacity: usize,
    first_required: *mut usize,
    second: &[u8],
    second_output: *mut u8,
    second_capacity: usize,
    second_required: *mut usize,
) -> Result<UsdToolboxStatus, FfiError> {
    if first_required.is_null() || second_required.is_null() {
        return Err(FfiError::invalid("required length pointers must not be null"));
    }
    unsafe {
        *first_required = first.len();
        *second_required = second.len();
    }
    if first_capacity < first.len()
        || second_capacity < second.len()
        || (!first.is_empty() && first_output.is_null())
        || (!second.is_empty() && second_output.is_null())
    {
        return Ok(USD_TOOLBOX_STATUS_BUFFER_TOO_SMALL);
    }
    unsafe {
        if !first.is_empty() {
            ptr::copy_nonoverlapping(first.as_ptr(), first_output, first.len());
        }
        if !second.is_empty() {
            ptr::copy_nonoverlapping(second.as_ptr(), second_output, second.len());
        }
    }
    Ok(USD_TOOLBOX_STATUS_OK)
}

unsafe fn write_single(
    bytes: &[u8],
    output: *mut u8,
    capacity: usize,
    required: *mut usize,
) -> Result<UsdToolboxStatus, FfiError> {
    if required.is_null() {
        return Err(FfiError::invalid("required_output must not be null"));
    }
    unsafe {
        *required = bytes.len();
    }
    if capacity < bytes.len() || (!bytes.is_empty() && output.is_null()) {
        return Ok(USD_TOOLBOX_STATUS_BUFFER_TOO_SMALL);
    }
    unsafe {
        if !bytes.is_empty() {
            ptr::copy_nonoverlapping(bytes.as_ptr(), output, bytes.len());
        }
    }
    Ok(USD_TOOLBOX_STATUS_OK)
}

unsafe fn write_single_raw(bytes: &[u8], output: *mut u8, capacity: usize, required: *mut usize) -> UsdToolboxStatus {
    if required.is_null() {
        return USD_TOOLBOX_STATUS_INVALID_ARGUMENT;
    }
    unsafe {
        *required = bytes.len();
    }
    if capacity < bytes.len() || (!bytes.is_empty() && output.is_null()) {
        return USD_TOOLBOX_STATUS_BUFFER_TOO_SMALL;
    }
    unsafe {
        if !bytes.is_empty() {
            ptr::copy_nonoverlapping(bytes.as_ptr(), output, bytes.len());
        }
    }
    USD_TOOLBOX_STATUS_OK
}

fn into_buffer(mut bytes: Vec<u8>) -> UsdToolboxBuffer {
    let buffer = UsdToolboxBuffer {
        data: bytes.as_mut_ptr(),
        len: bytes.len(),
        capacity: bytes.capacity(),
    };
    std::mem::forget(bytes);
    buffer
}

fn boundary(operation: impl FnOnce() -> Result<UsdToolboxStatus, FfiError>) -> UsdToolboxStatus {
    match catch_unwind(AssertUnwindSafe(operation)) {
        Ok(Ok(status)) => {
            if status == USD_TOOLBOX_STATUS_OK {
                set_last_error("");
            }
            status
        }
        Ok(Err(error)) => {
            set_last_error(&error.message);
            error.status
        }
        Err(payload) => {
            let detail = payload
                .downcast_ref::<&str>()
                .copied()
                .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
                .unwrap_or("unknown panic");
            set_last_error(&format!("panic caught at usd-toolbox boundary: {detail}"));
            USD_TOOLBOX_STATUS_PANIC
        }
    }
}

fn set_last_error(message: &str) {
    LAST_ERROR.with(|last| {
        *last.borrow_mut() = message.as_bytes().to_vec();
    });
}

struct FfiError {
    status: UsdToolboxStatus,
    message: String,
}

impl FfiError {
    fn invalid(message: impl Into<String>) -> Self {
        Self {
            status: USD_TOOLBOX_STATUS_INVALID_ARGUMENT,
            message: message.into(),
        }
    }

    fn import(error: impl std::fmt::Display) -> Self {
        Self {
            status: USD_TOOLBOX_STATUS_IMPORT_ERROR,
            message: error.to_string(),
        }
    }

    fn export(error: impl std::fmt::Display) -> Self {
        Self {
            status: USD_TOOLBOX_STATUS_EXPORT_ERROR,
            message: error.to_string(),
        }
    }
}
