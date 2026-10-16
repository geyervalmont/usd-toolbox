//! Opaque integer handles avoid exposing Rust allocation pointers. Registry locks
//! are held only while retrieving handles; separate documents can run concurrently.
use super::*;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, OnceLock};
use usd_toolbox_core::scene::*;

struct Document {
    document: SceneDocument,
    diagnostics: Vec<SceneDiagnostic>,
    limits: SceneLimits,
}
struct Prepared {
    bytes: Vec<u8>,
    report: Vec<u8>,
}
#[derive(Default)]
struct Registry {
    next: u64,
    documents: BTreeMap<u64, Arc<Mutex<Document>>>,
    results: BTreeMap<u64, Arc<Prepared>>,
    result_bytes: usize,
}
static REGISTRY: OnceLock<Mutex<Registry>> = OnceLock::new();
const MAX_HANDLES: usize = 256;
const MAX_RESULTS_BYTES: usize = 512 * 1024 * 1024;

fn registry() -> Result<std::sync::MutexGuard<'static, Registry>, FfiError> {
    REGISTRY
        .get_or_init(Default::default)
        .lock()
        .map_err(|_| FfiError::invalid("handle registry poisoned"))
}
fn next_handle(registry: &mut Registry) -> Result<u64, FfiError> {
    registry.next = registry
        .next
        .checked_add(1)
        .ok_or_else(|| FfiError::invalid("handle identifiers exhausted"))?;
    Ok(registry.next)
}
fn document(handle: u64) -> Result<Arc<Mutex<Document>>, FfiError> {
    registry()?
        .documents
        .get(&handle)
        .cloned()
        .ok_or_else(|| FfiError::invalid("unknown or released document handle"))
}
fn retain_document(imported: SceneImport, options: DocumentOptions) -> Result<u64, FfiError> {
    let value = Document {
        limits: options.scene_limits.clone(),
        document: SceneDocument::with_options(imported.scene, options).map_err(FfiError::invalid)?,
        diagnostics: imported.diagnostics,
    };
    let mut registry = registry()?;
    if registry.documents.len() >= MAX_HANDLES {
        return Err(FfiError::invalid("document handle budget exceeded"));
    }
    let handle = next_handle(&mut registry)?;
    registry.documents.insert(handle, Arc::new(Mutex::new(value)));
    Ok(handle)
}
fn prepare(value: Prepared) -> Result<u64, FfiError> {
    let size = value
        .bytes
        .len()
        .checked_add(value.report.len())
        .ok_or_else(|| FfiError::invalid("result size overflow"))?;
    let mut registry = registry()?;
    if registry.results.len() >= MAX_HANDLES || size > MAX_RESULTS_BYTES.saturating_sub(registry.result_bytes) {
        return Err(FfiError::invalid(
            "prepared-result budget exceeded; release previous results",
        ));
    }
    let handle = next_handle(&mut registry)?;
    registry.result_bytes += size;
    registry.results.insert(handle, Arc::new(value));
    Ok(handle)
}

fn capabilities() -> serde_json::Value {
    serde_json::json!({"schema":"usd-toolbox.model-capabilities.v1", "version":env!("CARGO_PKG_VERSION"), "geometry_lods":{"schema":"usd-toolbox.geometry-lods.v1","formats":["glb","usdz"],"error_policy":"original_vertex_displacement"}, "core":scene_capabilities(), "usd":usd_toolbox_usd::scene::usd_scene_capabilities(), "gltf":usd_toolbox_gltf::scene::gltf_scene_capabilities()})
}

/// Discover portable operations and the exact format profiles without opening a document.
///
/// # Safety
/// output must be writable for capacity; required_output must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn usd_toolbox_model_capabilities_v1(
    output: *mut u8,
    capacity: usize,
    required_output: *mut usize,
) -> UsdToolboxStatus {
    boundary(|| {
        let bytes = serde_json::to_vec(&capabilities()).map_err(invalid_json)?;
        unsafe { write_single(&bytes, output, capacity, required_output) }
    })
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct PreflightRequestOptions {
    name: String,
    profile: usd_toolbox_usd::scene::PreflightOptions,
}

/// Preflight USD/glTF into a prepared JSON result, including unsupported input.
/// This does not require successful scene import; inspect the report's completeness.
///
/// # Safety
/// Input/option pointers must be readable for their lengths; out_result writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn usd_toolbox_model_preflight_v1(
    input: *const u8,
    input_len: usize,
    options_json: *const u8,
    options_len: usize,
    out_result: *mut u64,
) -> UsdToolboxStatus {
    boundary(|| {
        if out_result.is_null() {
            return Err(FfiError::invalid("out_result is null"));
        }
        unsafe {
            *out_result = 0;
        }
        if options_len > 64 * 1024 {
            return Err(FfiError::invalid("preflight options exceed limit"));
        }
        let options: PreflightRequestOptions =
            parse_options(unsafe { input_slice(options_json, options_len, "options")? })?;
        if input_len > options.profile.limits.max_bytes {
            return Err(FfiError::invalid("input exceeds byte limit"));
        }
        let bytes = unsafe { input_slice(input, input_len, "input")? };
        let value = if bytes.starts_with(b"glTF")
            || options.name.to_ascii_lowercase().ends_with(".gltf")
            || options.name.to_ascii_lowercase().ends_with(".glb")
        {
            preflight_gltf(bytes, &options.name, &options.profile).map_err(FfiError::import)?
        } else {
            serde_json::to_value(usd_toolbox_usd::scene::preflight(
                Input::Named {
                    name: if options.name.is_empty() {
                        "model.usd"
                    } else {
                        &options.name
                    },
                    bytes,
                },
                &options.profile,
            ))
            .map_err(invalid_json)?
        };
        let handle = prepare(Prepared {
            bytes: serde_json::to_vec(&value).map_err(invalid_json)?,
            report: Vec::new(),
        })?;
        unsafe {
            *out_result = handle;
        }
        Ok(USD_TOOLBOX_STATUS_OK)
    })
}

#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
struct OpenOptions {
    name: String,
    format: String,
    document: DocumentOptions,
}
impl Default for OpenOptions {
    fn default() -> Self {
        Self {
            name: "model.usd".into(),
            format: "auto".into(),
            document: DocumentOptions::default(),
        }
    }
}

/// Import a scene once and retain indexed query state. Options are JSON:
/// {name, format: auto|usd|gltf|scene_json, document: DocumentOptions}.
/// Release with usd_toolbox_document_release_v1. Existing byte-buffer APIs remain.
///
/// # Safety
/// Input pointers must be readable for their lengths. out_handle must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn usd_toolbox_document_open_v1(
    input: *const u8,
    input_len: usize,
    options_json: *const u8,
    options_len: usize,
    out_handle: *mut u64,
) -> UsdToolboxStatus {
    boundary(|| {
        if out_handle.is_null() {
            return Err(FfiError::invalid("out_handle is null"));
        }
        unsafe {
            *out_handle = 0;
        }
        if options_len > 64 * 1024 {
            return Err(FfiError::invalid("open options exceed limit"));
        }
        let options: OpenOptions = parse_options(unsafe { input_slice(options_json, options_len, "options")? })?;
        if input_len > options.document.scene_limits.max_bytes {
            return Err(FfiError::invalid("input exceeds document budget"));
        }
        let bytes = unsafe { input_slice(input, input_len, "input")? };
        let gltf = options.format == "gltf"
            || (options.format == "auto"
                && (bytes.starts_with(b"glTF") || options.name.to_ascii_lowercase().ends_with(".gltf")));
        let imported = if options.format == "scene_json" {
            SceneImport {
                scene: parse_json(bytes, "scene")?,
                diagnostics: Vec::new(),
            }
        } else if gltf {
            usd_toolbox_gltf::scene::import_scene(
                bytes,
                &options.name,
                &usd_toolbox_gltf::scene::SceneGltfImportOptions {
                    limits: options.document.scene_limits.clone(),
                    ..Default::default()
                },
            )
            .map_err(FfiError::import)?
        } else if matches!(options.format.as_str(), "auto" | "usd") {
            usd_toolbox_usd::scene::import_scene(
                Input::Named {
                    name: &options.name,
                    bytes,
                },
                &usd_toolbox_usd::scene::SceneImportOptions {
                    limits: options.document.scene_limits.clone(),
                },
            )
            .map_err(FfiError::import)?
        } else {
            return Err(FfiError::invalid("format must be auto, usd, gltf or scene_json"));
        };
        let handle = retain_document(imported, options.document)?;
        unsafe {
            *out_handle = handle;
        }
        Ok(USD_TOOLBOX_STATUS_OK)
    })
}

/// Borrowed UTF-8 filename and encoded binary content. No trailing NUL required.
/// All pointers remain caller-owned and need live only through the open call.
#[repr(C)]
pub struct UsdToolboxInputFile {
    pub name: *const u8,
    pub name_len: usize,
    pub data: *const u8,
    pub data_len: usize,
}

/// Open a root layer/glTF plus caller-resolved dependencies without JSON binary
/// arrays. The first file is the root. Names are confined root-relative paths.
/// Options match document_open_v1; format scene_json is unsupported for bundles.
///
/// # Safety
/// files must point to file_count valid aligned entries; their byte pointers and
/// options must be readable for the specified lengths. out_handle is writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn usd_toolbox_document_open_bundle_v1(
    files: *const UsdToolboxInputFile,
    file_count: usize,
    options_json: *const u8,
    options_len: usize,
    out_handle: *mut u64,
) -> UsdToolboxStatus {
    boundary(|| {
        if out_handle.is_null() {
            return Err(FfiError::invalid("out_handle is null"));
        }
        unsafe {
            *out_handle = 0;
        }
        if options_len > 64 * 1024 {
            return Err(FfiError::invalid("open options exceed limit"));
        }
        let options: OpenOptions = parse_options(unsafe { input_slice(options_json, options_len, "options")? })?;
        if files.is_null()
            || file_count == 0
            || file_count > options.document.scene_limits.max_dependencies.saturating_add(1)
            || file_count > 4096
        {
            return Err(FfiError::invalid("invalid bundle file count/pointer"));
        }
        let inputs = unsafe { std::slice::from_raw_parts(files, file_count) };
        let mut names = std::collections::BTreeSet::new();
        let mut borrowed = Vec::with_capacity(file_count);
        let mut total = 0usize;
        for input in inputs {
            if input.name_len > 4096 {
                return Err(FfiError::invalid("bundle filename exceeds limit"));
            }
            total = total
                .checked_add(input.data_len)
                .ok_or_else(|| FfiError::invalid("bundle size overflow"))?;
            if total > options.document.scene_limits.max_bytes {
                return Err(FfiError::invalid("bundle exceeds input budget"));
            }
            let name = std::str::from_utf8(unsafe { input_slice(input.name, input.name_len, "filename")? })
                .map_err(FfiError::invalid)?;
            if !names.insert(name) {
                return Err(FfiError::invalid("duplicate bundle filename"));
            }
            borrowed.push(usd_toolbox_core::InputFile {
                name,
                bytes: unsafe { input_slice(input.data, input.data_len, "file")? },
            });
        }
        let root = &borrowed[0];
        let gltf = options.format == "gltf"
            || (options.format == "auto"
                && (root.bytes.starts_with(b"glTF")
                    || root.name.to_ascii_lowercase().ends_with(".gltf")
                    || root.name.to_ascii_lowercase().ends_with(".glb")));
        let imported = if gltf {
            let owned = borrowed
                .iter()
                .map(|file| (file.name.to_owned(), file.bytes.to_vec()))
                .collect();
            usd_toolbox_gltf::scene::import_scene_bundle(
                root.name,
                &owned,
                &usd_toolbox_gltf::scene::SceneGltfImportOptions {
                    limits: options.document.scene_limits.clone(),
                    ..Default::default()
                },
            )
            .map_err(FfiError::import)?
        } else if matches!(options.format.as_str(), "auto" | "usd") {
            usd_toolbox_usd::scene::import_scene(
                Input::Bundle(&borrowed),
                &usd_toolbox_usd::scene::SceneImportOptions {
                    limits: options.document.scene_limits.clone(),
                },
            )
            .map_err(FfiError::import)?
        } else {
            return Err(FfiError::invalid("bundle format must be auto, usd or gltf"));
        };
        let handle = retain_document(imported, options.document)?;
        unsafe {
            *out_handle = handle;
        }
        Ok(USD_TOOLBOX_STATUS_OK)
    })
}

#[derive(Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
enum DocumentRequest {
    Query {
        #[serde(default)]
        request: QueryRequest,
    },
    Count {
        #[serde(default)]
        filter: NodeFilter,
    },
    Node {
        id: String,
        #[serde(default)]
        fields: Vec<QueryField>,
    },
    Metadata {
        target: MetadataTarget,
    },
    Property {
        target: MetadataTarget,
        #[serde(default)]
        namespace: MetadataNamespace,
        key: String,
    },
    Children {
        parent: Option<String>,
    },
    Ancestors {
        id: String,
    },
    Descendants {
        id: String,
    },
    Path {
        path: String,
    },
    Summary,
    Inspect,
    Diagnostics,
    Capabilities,
    Apply {
        edits: Vec<SceneEdit>,
    },
    Export {
        target: String,
        #[serde(default)]
        texture_tier: Option<usd_toolbox_core::Tier>,
        #[serde(default)]
        include_hidden: Option<bool>,
        #[serde(default)]
        optimize_vertex_cache: Option<bool>,
    },
    ExportLods {
        options: SceneLodOptions,
        target: String,
    },
    SceneJson,
}

/// Execute once and retain the output. Buffer sizing/copy never repeats a query,
/// edit or conversion. Release each output with usd_toolbox_result_release_v1.
/// Queries never expose bulk geometry unless SceneJson/export is requested.
///
/// # Safety
/// request_json must be readable for request_len; out_result must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn usd_toolbox_document_request_v1(
    handle: u64,
    request_json: *const u8,
    request_len: usize,
    out_result: *mut u64,
) -> UsdToolboxStatus {
    boundary(|| {
        if out_result.is_null() {
            return Err(FfiError::invalid("out_result is null"));
        }
        unsafe {
            *out_result = 0;
        }
        if request_len > 8 * 1024 * 1024 {
            return Err(FfiError::invalid("document request exceeds budget"));
        }
        let request: DocumentRequest =
            parse_json(unsafe { input_slice(request_json, request_len, "request")? }, "request")?;
        let document = document(handle)?;
        let mut value = document.lock().map_err(|_| FfiError::invalid("document poisoned"))?;
        if let DocumentRequest::Apply { edits } = &request {
            let mut prepared_handle = 0;
            value
                .document
                .apply_checked(edits, |report| {
                    let bytes =
                        serde_json::to_vec(report).map_err(|e| SceneError::new("ffi.report", "/", e.to_string()))?;
                    prepared_handle = prepare(Prepared {
                        bytes,
                        report: Vec::new(),
                    })
                    .map_err(|e| SceneError::new("ffi.result_budget", "/", e.message))?;
                    Ok(())
                })
                .map_err(FfiError::invalid)?;
            unsafe {
                *out_result = prepared_handle;
            }
            return Ok(USD_TOOLBOX_STATUS_OK);
        }
        let mut report = Vec::new();
        let bytes = match request {
            DocumentRequest::Query { request } => {
                serde_json::to_vec(&value.document.query(&request).map_err(FfiError::invalid)?)
            }
            DocumentRequest::Count { filter } => {
                serde_json::to_vec(&value.document.count(&filter).map_err(FfiError::invalid)?)
            }
            DocumentRequest::Node { id, fields } => {
                serde_json::to_vec(&value.document.get_node(&id, &fields).map_err(FfiError::invalid)?)
            }
            DocumentRequest::Metadata { target } => {
                serde_json::to_vec(&value.document.get_metadata(&target).map_err(FfiError::invalid)?)
            }
            DocumentRequest::Property { target, namespace, key } => serde_json::to_vec(
                &value
                    .document
                    .get_property(&target, namespace, &key)
                    .map_err(FfiError::invalid)?,
            ),
            DocumentRequest::Children { parent } => {
                serde_json::to_vec(&value.document.children(parent.as_deref()).map_err(FfiError::invalid)?)
            }
            DocumentRequest::Ancestors { id } => {
                serde_json::to_vec(&value.document.ancestors(&id).map_err(FfiError::invalid)?)
            }
            DocumentRequest::Descendants { id } => {
                serde_json::to_vec(&value.document.descendants(&id).map_err(FfiError::invalid)?)
            }
            DocumentRequest::Path { path } => serde_json::to_vec(value.document.ids_for_path(&path)),
            DocumentRequest::Summary => serde_json::to_vec(&value.document.summary()),
            DocumentRequest::Inspect => serde_json::to_vec(&value.document.inspect().map_err(FfiError::invalid)?),
            DocumentRequest::Diagnostics => serde_json::to_vec(&value.diagnostics),
            DocumentRequest::Capabilities => serde_json::to_vec(&capabilities()),
            DocumentRequest::Apply { .. } => return Err(FfiError::invalid("edit request was not prepared")),
            DocumentRequest::SceneJson => serde_json::to_vec(value.document.scene()),
            DocumentRequest::ExportLods { options, target } => {
                let prepared = export_lods(&value, options, &target)?;
                report = prepared.report;
                Ok(prepared.bytes)
            }
            DocumentRequest::Export {
                target,
                texture_tier,
                include_hidden,
                optimize_vertex_cache,
            } => {
                if !matches!(target.as_str(), "glb" | "gltf")
                    && (texture_tier.is_some() || include_hidden.is_some() || optimize_vertex_cache.is_some())
                {
                    return Err(FfiError::invalid("texture_tier/include_hidden apply to glTF exports"));
                }
                let exported = match target.as_str() {
                    "usdz" | "usda" | "usdc" => usd_toolbox_usd::scene::export_scene(
                        value.document.scene(),
                        &usd_toolbox_usd::scene::SceneExportOptions {
                            format: match target.as_str() {
                                "usda" => UsdFormat::Usda,
                                "usdc" => UsdFormat::Usdc,
                                _ => UsdFormat::Usdz,
                            },
                            ..Default::default()
                        },
                    ),
                    "glb" | "gltf" => usd_toolbox_gltf::scene::export_scene(
                        value.document.scene(),
                        &usd_toolbox_gltf::scene::SceneGltfOptions {
                            texture_tier: texture_tier.unwrap_or(usd_toolbox_core::Tier::K2),
                            include_hidden: include_hidden.unwrap_or(false),
                            optimize_vertex_cache: optimize_vertex_cache.unwrap_or(false),
                            format: if target == "glb" {
                                GltfFormat::Glb
                            } else {
                                GltfFormat::Gltf
                            },
                            ..Default::default()
                        },
                    ),
                    _ => return Err(FfiError::invalid("unknown document export target")),
                }
                .map_err(FfiError::export)?;
                report = serde_json::to_vec(&exported.diagnostics).map_err(invalid_json)?;
                Ok(exported.bytes)
            }
        }
        .map_err(invalid_json)?;
        let handle = prepare(Prepared { bytes, report })?;
        unsafe {
            *out_result = handle;
        }
        Ok(USD_TOOLBOX_STATUS_OK)
    })
}

/// Copy a prepared output and optional diagnostics using the two-pass contract.
/// The result remains valid until released, independently of its document.
///
/// # Safety
/// Required-length pointers must be writable. Output pointers must be writable
/// for their capacities and must not overlap each other or the length pointers.
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn usd_toolbox_result_copy_v1(
    handle: u64,
    output: *mut u8,
    output_capacity: usize,
    required_output: *mut usize,
    report: *mut u8,
    report_capacity: usize,
    required_report: *mut usize,
) -> UsdToolboxStatus {
    boundary(|| {
        let value = registry()?
            .results
            .get(&handle)
            .cloned()
            .ok_or_else(|| FfiError::invalid("unknown or released result handle"))?;
        unsafe {
            write_pair(
                &value.bytes,
                output,
                output_capacity,
                required_output,
                &value.report,
                report,
                report_capacity,
                required_report,
            )
        }
    })
}

/// Release a document. Unknown/already-released handles return INVALID_ARGUMENT.
#[unsafe(no_mangle)]
pub extern "C" fn usd_toolbox_document_release_v1(handle: u64) -> UsdToolboxStatus {
    boundary(|| {
        registry()?
            .documents
            .remove(&handle)
            .ok_or_else(|| FfiError::invalid("unknown or released document handle"))?;
        Ok(USD_TOOLBOX_STATUS_OK)
    })
}

/// Release a prepared result. Does not affect the originating document.
#[unsafe(no_mangle)]
pub extern "C" fn usd_toolbox_result_release_v1(handle: u64) -> UsdToolboxStatus {
    boundary(|| {
        let mut registry = registry()?;
        let value = registry
            .results
            .remove(&handle)
            .ok_or_else(|| FfiError::invalid("unknown or released result handle"))?;
        registry.result_bytes -= value.bytes.len() + value.report.len();
        Ok(USD_TOOLBOX_STATUS_OK)
    })
}

fn export_lods(value: &Document, mut options: SceneLodOptions, target: &str) -> Result<Prepared, FfiError> {
    use sha2::{Digest, Sha256};
    use std::io::Write;
    if !matches!(target, "glb" | "usdz") {
        return Err(FfiError::invalid("LOD target must be glb or usdz"));
    }
    options.max_levels = options.max_levels.min(8);
    options.max_output_bytes = options
        .max_output_bytes
        .min(value.limits.max_bytes)
        .min(MAX_RESULTS_BYTES);
    options.max_output_faces = options.max_output_faces.min(value.limits.max_faces);
    options.max_source_corners = options.max_source_corners.min(value.limits.max_corners);
    let generated = generate_scene_lods(value.document.scene(), &options, &value.limits)
        .map_err(FfiError::invalid)?
        .export_with(|scene| {
            if target == "glb" {
                usd_toolbox_gltf::scene::export_scene(
                    scene,
                    &usd_toolbox_gltf::scene::SceneGltfOptions {
                        limits: value.limits.clone(),
                        ..Default::default()
                    },
                )
            } else {
                usd_toolbox_usd::scene::export_scene(
                    scene,
                    &usd_toolbox_usd::scene::SceneExportOptions {
                        limits: value.limits.clone(),
                        ..Default::default()
                    },
                )
            }
        })
        .map_err(FfiError::export)?;
    let files = generated
        .levels
        .iter()
        .enumerate()
        .map(|(i, item)| {
            serde_json::json!({
                "name":format!("lod-{i}.{target}"),"level":item.level.name,"bytes":item.export.bytes.len(),
                "sha256":format!("{:x}",Sha256::digest(&item.export.bytes)),"diagnostics":item.export.diagnostics,
            })
        })
        .collect::<Vec<_>>();
    let mut manifest = serde_json::to_value(&generated.manifest).map_err(invalid_json)?;
    manifest["target"] = serde_json::json!(target);
    manifest["files"] = serde_json::json!(files);
    manifest["import_diagnostics"] = serde_json::json!(value.diagnostics);
    let report = serde_json::to_vec_pretty(&manifest).map_err(invalid_json)?;
    // Reserve the separately retained report in addition to the ZIP byte budget.
    let archive_limit = options
        .max_output_bytes
        .checked_sub(report.len())
        .ok_or_else(|| FfiError::invalid("LOD manifest exceeds output budget"))?;
    // Reserve conservative headers before constructing a writer; ordinary
    // over-limit requests fail without attempting to finalize a partial ZIP.
    let payload_bytes = generated
        .levels
        .iter()
        .try_fold(report.len(), |n, item| n.checked_add(item.export.bytes.len()))
        .ok_or_else(|| FfiError::invalid("LOD ZIP payload count overflow"))?;
    let overhead = (generated.levels.len() + 2).saturating_mul(1024);
    if payload_bytes.saturating_add(overhead) > archive_limit {
        return Err(FfiError::invalid(
            "LOD ZIP payload, report and archive overhead exceed output budget",
        ));
    }
    let writer = BoundedLodArchive {
        cursor: std::io::Cursor::new(Vec::new()),
        maximum: archive_limit,
    };
    let mut archive = zip::ZipWriter::new(writer);
    let settings = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Stored)
        .unix_permissions(0o644);
    archive
        .start_file("manifest.json", settings)
        .map_err(FfiError::export)?;
    archive.write_all(&report).map_err(FfiError::export)?;
    for (i, item) in generated.levels.into_iter().enumerate() {
        archive
            .start_file(format!("lod-{i}.{target}"), settings)
            .map_err(FfiError::export)?;
        archive.write_all(&item.export.bytes).map_err(FfiError::export)?;
    }
    let bytes = archive.finish().map_err(FfiError::export)?.cursor.into_inner();
    Ok(Prepared { bytes, report })
}
struct BoundedLodArchive {
    cursor: std::io::Cursor<Vec<u8>>,
    maximum: usize,
}
impl std::io::Write for BoundedLodArchive {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if self
            .cursor
            .position()
            .checked_add(bytes.len() as u64)
            .is_none_or(|end| end > self.maximum as u64)
        {
            return Err(std::io::Error::other(
                "LOD ZIP including archive overhead exceeds output budget",
            ));
        }
        std::io::Write::write(&mut self.cursor, bytes)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
impl std::io::Seek for BoundedLodArchive {
    fn seek(&mut self, position: std::io::SeekFrom) -> std::io::Result<u64> {
        let old = self.cursor.position();
        let new = std::io::Seek::seek(&mut self.cursor, position)?;
        if new > self.maximum as u64 {
            self.cursor.set_position(old);
            return Err(std::io::Error::other("LOD ZIP seek exceeds output budget"));
        }
        Ok(new)
    }
}

fn preflight_gltf(
    bytes: &[u8],
    name: &str,
    options: &usd_toolbox_usd::scene::PreflightOptions,
) -> Result<serde_json::Value, SceneError> {
    let mut report = usd_toolbox_gltf::scene::preflight_scene(bytes, name, &options.limits)?;
    let imported = || {
        usd_toolbox_gltf::scene::import_scene(
            bytes,
            name,
            &usd_toolbox_gltf::scene::SceneGltfImportOptions {
                limits: options.limits.clone(),
                ..Default::default()
            },
        )
    };

    report["target"] = serde_json::to_value(options.target).expect("target is serializable");
    report["usd_only_options"] = serde_json::json!(["max_specs", "max_metadata_bytes"]);
    report["validation"]["target"] = serde_json::json!("deferred");
    if options.check_geometry {
        match imported() {
            Ok(imported) => match inspect_scene(&imported.scene, &options.limits) {
                Ok(inspection) => {
                    report["geometry_inspection"] =
                        serde_json::to_value(inspection).expect("inspection is serializable");
                    report["diagnostics"]
                        .as_array_mut()
                        .expect("preflight diagnostics")
                        .extend(
                            imported
                                .diagnostics
                                .into_iter()
                                .map(|d| serde_json::to_value(d).expect("diagnostic is serializable")),
                        );
                    report["validation"]["buffers"] = serde_json::json!("performed");
                    report["validation"]["geometry"] = serde_json::json!("performed");
                    report["validation"]["integrity"] = serde_json::json!("performed_source_and_dependency_validation");
                    report["note"] = serde_json::json!(
                        "Geometry and source/dependency validation completed; inspect validation.target and validation.images for remaining work. Render equivalence is not established."
                    );
                    if matches!(
                        options.target,
                        usd_toolbox_usd::scene::PreflightTarget::Glb
                            | usd_toolbox_usd::scene::PreflightTarget::BrowserPreview
                    ) {
                        match usd_toolbox_gltf::scene::export_scene(
                            &imported.scene,
                            &usd_toolbox_gltf::scene::SceneGltfOptions {
                                limits: options.limits.clone(),
                                ..Default::default()
                            },
                        ) {
                            Ok(exported) => {
                                report["target_diagnostics"] =
                                    serde_json::to_value(exported.diagnostics).expect("diagnostics are serializable");
                                report["validation"]["target"] = serde_json::json!("performed");
                                report["validation"]["images"] = serde_json::json!("performed_preview_decode");
                            }
                            Err(error) => {
                                report["compatible"] = serde_json::json!(false);
                                report["complete"] = serde_json::json!(false);
                                report["validation"]["target"] = serde_json::json!("failed");
                                report["target_error"] =
                                    serde_json::json!({"code":error.code,"path":error.path,"detail":error.detail});
                            }
                        }
                    }
                }
                Err(error) => {
                    report["compatible"] = serde_json::json!(false);
                    report["complete"] = serde_json::json!(false);
                    report["validation"]["geometry"] = serde_json::json!("failed");
                    report["geometry_error"] = serde_json::json!(error.to_string());
                }
            },
            Err(error) => {
                report["compatible"] = serde_json::json!(false);
                report["complete"] = serde_json::json!(false);
                report["validation"]["geometry"] = serde_json::json!("failed");
                report["geometry_error"] = serde_json::json!(error.to_string());
            }
        }
    }
    limit_gltf_preflight_diagnostics(&mut report, options.max_issues);
    Ok(report)
}

// Bound every diagnostic array in this report, including expensive-check results.
fn limit_gltf_preflight_diagnostics(report: &mut serde_json::Value, maximum: usize) {
    let mut remaining = maximum.max(1);
    let mut truncated = false;
    for path in [
        "/diagnostics",
        "/geometry_inspection/diagnostics",
        "/target_diagnostics",
    ] {
        if let Some(diagnostics) = report.pointer_mut(path).and_then(serde_json::Value::as_array_mut) {
            if diagnostics.len() > remaining {
                diagnostics.truncate(remaining);
                truncated = true;
            }
            remaining = remaining.saturating_sub(diagnostics.len());
        }
    }
    if truncated {
        report["complete"] = serde_json::json!(false);
        report["compatible"] = serde_json::json!(false);
        report["diagnostics_truncated"] = serde_json::json!(true);
        report["validation"]["diagnostics"] = serde_json::json!("truncated");
    }
}

#[cfg(test)]
mod lod_archive_tests {
    use super::BoundedLodArchive;
    use std::io::{Cursor, Seek, SeekFrom, Write};

    #[test]
    fn archive_budget_is_checked_before_growing_or_seeking() {
        let mut writer = BoundedLodArchive {
            cursor: Cursor::new(Vec::new()),
            maximum: 8,
        };
        writer.write_all(&[1; 8]).unwrap();
        assert!(writer.write_all(&[2]).is_err());
        assert_eq!(writer.cursor.get_ref(), &[1; 8]);
        assert!(writer.seek(SeekFrom::Start(u64::MAX)).is_err());
        assert_eq!(writer.cursor.position(), 8);
        writer.seek(SeekFrom::Start(0)).unwrap();
        writer.write_all(&[3; 8]).unwrap();
        assert_eq!(writer.cursor.get_ref(), &[3; 8]);
    }
}
