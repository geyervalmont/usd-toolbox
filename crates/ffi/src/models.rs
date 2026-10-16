use super::*;
use usd_toolbox_core::scene::*;
use usd_toolbox_gltf::scene::SceneGltfOptions;
use usd_toolbox_usd::scene::{SceneExportOptions, SceneImportOptions};

/// Imports a model USD layer or self-contained USDZ into scene JSON and diagnostics.
/// Uses the standard two-pass buffer contract. Version 1 accepts static polygons.
///
/// # Safety
/// Non-null pointers must be valid for the supplied lengths/capacities; required
/// length pointers must be writable and output buffers must not overlap.
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn usd_toolbox_model_import_v1(
    input: *const u8,
    input_len: usize,
    options_json: *const u8,
    options_len: usize,
    scene_json: *mut u8,
    scene_capacity: usize,
    required_scene: *mut usize,
    report_json: *mut u8,
    report_capacity: usize,
    required_report: *mut usize,
) -> UsdToolboxStatus {
    boundary(|| {
        let bytes = unsafe { input_slice(input, input_len, "input")? };
        let options: SceneImportOptions = parse_options(unsafe { input_slice(options_json, options_len, "options")? })?;
        let imported = usd_toolbox_usd::scene::import_scene(Input::Bytes(bytes), &options).map_err(FfiError::import)?;
        let scene = serde_json::to_vec(&imported.scene).map_err(invalid_json)?;
        let report = serde_json::to_vec(&imported.diagnostics).map_err(invalid_json)?;
        unsafe {
            write_pair(
                &scene,
                scene_json,
                scene_capacity,
                required_scene,
                &report,
                report_json,
                report_capacity,
                required_report,
            )
        }
    })
}

/// Exports scene JSON to USDZ/USDA/USDC/glTF/GLB using existing target constants.
/// The second output is an array of scene and material conversion diagnostics.
///
/// # Safety
/// Non-null pointers must be valid for supplied lengths/capacities; required
/// length pointers must be writable and output buffers must not overlap.
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn usd_toolbox_model_export_v1(
    scene_json: *const u8,
    scene_len: usize,
    target: UsdToolboxTarget,
    output: *mut u8,
    output_capacity: usize,
    required_output: *mut usize,
    report_json: *mut u8,
    report_capacity: usize,
    required_report: *mut usize,
) -> UsdToolboxStatus {
    boundary(|| {
        if scene_len > SceneLimits::default().max_bytes {
            return Err(FfiError::invalid("scene JSON exceeds limit"));
        }
        let scene: Scene = parse_json(
            unsafe { input_slice(scene_json, scene_len, "scene_json")? },
            "scene_json",
        )?;
        let result = match target {
            USD_TOOLBOX_TARGET_USDZ | USD_TOOLBOX_TARGET_USDA | USD_TOOLBOX_TARGET_USDC => {
                usd_toolbox_usd::scene::export_scene(
                    &scene,
                    &SceneExportOptions {
                        format: match target {
                            USD_TOOLBOX_TARGET_USDA => UsdFormat::Usda,
                            USD_TOOLBOX_TARGET_USDC => UsdFormat::Usdc,
                            _ => UsdFormat::Usdz,
                        },
                        ..Default::default()
                    },
                )
            }
            USD_TOOLBOX_TARGET_GLB | USD_TOOLBOX_TARGET_GLTF => usd_toolbox_gltf::scene::export_scene(
                &scene,
                &SceneGltfOptions {
                    format: if target == USD_TOOLBOX_TARGET_GLB {
                        GltfFormat::Glb
                    } else {
                        GltfFormat::Gltf
                    },
                    ..Default::default()
                },
            ),
            _ => return Err(FfiError::invalid("unsupported model export target")),
        }
        .map_err(FfiError::export)?;
        let report = serde_json::to_vec(&result.diagnostics).map_err(invalid_json)?;
        unsafe {
            write_pair(
                &result.bytes,
                output,
                output_capacity,
                required_output,
                &report,
                report_json,
                report_capacity,
                required_report,
            )
        }
    })
}

#[derive(Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
enum Request {
    Inspect { scene: Scene },
    Validate { scene: Scene },
    ExportAssets { scene: Scene },
    Apply { scene: Scene, edits: Vec<SceneEdit> },
    Extract { scene: Scene, node: String },
    Diff { before: Scene, after: Box<Scene> },
    ImportBundle { files: Vec<BundleFile> },
}
#[derive(Deserialize)]
struct BundleFile {
    name: String,
    bytes: Vec<u8>,
}

/// Executes inspect/validate/apply/extract/diff/import_bundle/export_assets JSON.
/// Returns versioned scene/inspection data or diagnostics. The root input is the
/// first file of import_bundle. All operations are pure and return new data.
///
/// # Safety
/// Every non-null pointer must be valid for supplied length/capacity and the
/// required_output pointer must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn usd_toolbox_model_operation_v1(
    request_json: *const u8,
    request_len: usize,
    output_json: *mut u8,
    output_capacity: usize,
    required_output: *mut usize,
) -> UsdToolboxStatus {
    boundary(|| {
        let limits = SceneLimits::default();
        if request_len > limits.max_bytes {
            return Err(FfiError::invalid("request exceeds byte limit"));
        }
        let request: Request = parse_json(unsafe { input_slice(request_json, request_len, "request")? }, "request")?;
        let value = match request {
            Request::Inspect { scene } => {
                serde_json::to_vec(&inspect_scene(&scene, &limits).map_err(FfiError::invalid)?)
            }
            Request::Validate { scene } => serde_json::to_vec(&validate_scene(&scene, &limits)),
            Request::ExportAssets { scene } => serde_json::to_vec(
                &usd_toolbox_usd::scene::export_scene_assets(&scene, &limits).map_err(FfiError::invalid)?,
            ),
            Request::Apply { scene, edits } => {
                serde_json::to_vec(&edit_scene(&scene, &edits, &limits).map_err(FfiError::invalid)?)
            }
            Request::Extract { scene, node } => {
                serde_json::to_vec(&extract_subtree(&scene, &node, &limits).map_err(FfiError::invalid)?)
            }
            Request::Diff { before, after } => {
                require_valid_scene(&before, &limits).map_err(FfiError::invalid)?;
                require_valid_scene(&after, &limits).map_err(FfiError::invalid)?;
                serde_json::to_vec(&diff_scenes(&before, &after))
            }
            Request::ImportBundle { files } => {
                let files: Vec<_> = files
                    .iter()
                    .map(|f| usd_toolbox_core::InputFile {
                        name: &f.name,
                        bytes: &f.bytes,
                    })
                    .collect();
                serde_json::to_vec(
                    &usd_toolbox_usd::scene::import_scene(Input::Bundle(&files), &SceneImportOptions { limits })
                        .map_err(FfiError::import)?,
                )
            }
        }
        .map_err(invalid_json)?;
        unsafe { write_single(&value, output_json, output_capacity, required_output) }
    })
}
