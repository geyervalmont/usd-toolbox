use super::*;
use sha2::{Digest, Sha256};
use usd_toolbox_core::InputFile;
use usd_toolbox_core::scene::*;
use usd_toolbox_gltf::scene::SceneGltfOptions;
use usd_toolbox_usd::scene::{SceneExportOptions, SceneImportOptions};

/// Owns geometry in WASM memory so inspecting/editing does not serialize large
/// mesh arrays through JavaScript. Call free() when the model is no longer used.
#[wasm_bindgen]
pub struct WasmScene {
    document: SceneDocument,
    diagnostics: Vec<SceneDiagnostic>,
}

#[derive(Default, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
struct ModelExportOptions {
    texture_tier: Option<usd_toolbox_core::Tier>,
    include_hidden: Option<bool>,
    optimize_vertex_cache: Option<bool>,
}

#[wasm_bindgen]
impl WasmScene {
    /// Query projected metadata without exporting geometry or texture arrays.
    pub fn query(&self, request: JsValue) -> Result<JsValue, JsValue> {
        let request: QueryRequest = options_from_js(Some(request))?;
        model_js(&self.document.query(&request).map_err(model_error)?)
    }
    pub fn get_node(&self, id: &str) -> Result<JsValue, JsValue> {
        model_js(
            &self
                .document
                .get_node(
                    id,
                    &[
                        QueryField::Name,
                        QueryField::Parent,
                        QueryField::NodeType,
                        QueryField::Metadata,
                    ],
                )
                .map_err(model_error)?,
        )
    }
    pub fn get_metadata(&self, target: JsValue) -> Result<JsValue, JsValue> {
        let target: MetadataTarget = from_js(target)?;
        model_js(&self.document.get_metadata(&target).map_err(model_error)?)
    }
    pub fn get_property(&self, target: JsValue, namespace: JsValue, key: &str) -> Result<JsValue, JsValue> {
        let target: MetadataTarget = from_js(target)?;
        let namespace: MetadataNamespace = from_js(namespace)?;
        model_js(
            &self
                .document
                .get_property(&target, namespace, key)
                .map_err(model_error)?,
        )
    }
    pub fn children(&self, parent: Option<String>) -> Result<JsValue, JsValue> {
        model_js(&self.document.children(parent.as_deref()).map_err(model_error)?)
    }
    pub fn ancestors(&self, id: &str) -> Result<JsValue, JsValue> {
        model_js(&self.document.ancestors(id).map_err(model_error)?)
    }
    pub fn descendants(&self, id: &str) -> Result<JsValue, JsValue> {
        model_js(&self.document.descendants(id).map_err(model_error)?)
    }
    pub fn ids_for_path(&self, path: &str) -> Result<JsValue, JsValue> {
        model_js(self.document.ids_for_path(path))
    }
    pub fn count(&self, filter: JsValue) -> Result<usize, JsValue> {
        let filter: NodeFilter = options_from_js(Some(filter))?;
        self.document.count(&filter).map_err(model_error)
    }
    pub fn summary(&self) -> Result<JsValue, JsValue> {
        model_js(&self.document.summary())
    }
    /// Applies an atomic transaction to this document and advances query generation.
    /// Metadata and hierarchy edits retain the existing geometry/image allocation.
    pub fn update(&mut self, edits: JsValue) -> Result<JsValue, JsValue> {
        let edits: Vec<SceneEdit> = from_js(edits)?;
        model_js(&self.document.apply(&edits).map_err(model_error)?)
    }
    pub fn inspect(&self) -> Result<JsValue, JsValue> {
        model_js(&inspect_scene(self.document.scene(), &SceneLimits::browser()).map_err(model_error)?)
    }
    pub fn diagnostics(&self) -> Result<JsValue, JsValue> {
        model_js(&self.diagnostics)
    }
    /// Required sidecars for standalone USDA/USDC: relative path -> Uint8Array.
    /// USDZ/GLB exports already embed their dependencies.
    pub fn assets(&self) -> Result<js_sys::Map, JsValue> {
        let assets = usd_toolbox_usd::scene::export_scene_assets(self.document.scene(), &SceneLimits::browser())
            .map_err(model_error)?;
        let map = js_sys::Map::new();
        for (path, bytes) in assets {
            map.set(&JsValue::from_str(&path), &js_sys::Uint8Array::from(bytes.as_slice()));
        }
        Ok(map)
    }
    pub fn validate(&self) -> Result<JsValue, JsValue> {
        model_js(&validate_scene(self.document.scene(), &SceneLimits::browser()))
    }
    /// Exports usdz/usda/usdc/glb/gltf as binary bytes plus structured diagnostics.
    pub fn export(&self, target: &str) -> Result<WasmExport, JsValue> {
        self.export_with_options(target, None)
    }
    pub fn export_with_options(&self, target: &str, options: Option<JsValue>) -> Result<WasmExport, JsValue> {
        let options: ModelExportOptions = options_from_js(options)?;
        let result = export_model_scene(self.document.scene(), target, &options, &SceneLimits::browser())
            .map_err(model_error)?;
        let mut diagnostics = self.diagnostics.clone();
        diagnostics.extend(result.diagnostics);
        Ok(WasmExport {
            bytes: result.bytes,
            losses: model_js(&diagnostics)?,
        })
    }
    /// Generate and export alternative LODs without mutating this document.
    pub fn export_lods(
        &self,
        lod_options: JsValue,
        target: &str,
        export_options: Option<JsValue>,
    ) -> Result<JsValue, JsValue> {
        if !matches!(target, "glb" | "usdz") {
            return Err(JsError::new("LOD target must be glb or usdz").into());
        }
        let mut options: SceneLodOptions = from_js(lod_options)?;
        let limits = SceneLimits::browser();
        options.max_levels = options.max_levels.min(8);
        options.max_output_bytes = options.max_output_bytes.min(limits.max_bytes);
        options.max_output_faces = options.max_output_faces.min(limits.max_faces);
        options.max_source_corners = options.max_source_corners.min(limits.max_corners);
        let export_options: ModelExportOptions = options_from_js(export_options)?;
        let levels = generate_scene_lods(self.document.scene(), &options, &limits)
            .map_err(model_error)?
            .export_with(|scene| export_model_scene(scene, target, &export_options, &limits))
            .map_err(model_error)?;
        let result = js_sys::Object::new();
        let mut manifest = serde_json::to_value(&levels.manifest).map_err(model_error)?;
        manifest["target"] = serde_json::json!(target);
        manifest["import_diagnostics"] = serde_json::to_value(&self.diagnostics).map_err(model_error)?;
        let mut artifacts = Vec::new();
        let files = js_sys::Array::new();
        for (index, level) in levels.levels.into_iter().enumerate() {
            artifacts.push(serde_json::json!({"name":format!("lod-{index}.{target}"),"level":level.level.name,
                "bytes":level.export.bytes.len(),"sha256":format!("{:x}",Sha256::digest(&level.export.bytes)),"diagnostics":level.export.diagnostics}));
            let file = js_sys::Object::new();
            js_sys::Reflect::set(&file, &"name".into(), &format!("lod-{index}.{target}").into())?;
            js_sys::Reflect::set(&file, &"level".into(), &level.level.name.into())?;
            js_sys::Reflect::set(
                &file,
                &"bytes".into(),
                &js_sys::Uint8Array::from(level.export.bytes.as_slice()),
            )?;
            let mut diagnostics = self.diagnostics.clone();
            diagnostics.extend(level.export.diagnostics);
            js_sys::Reflect::set(&file, &"diagnostics".into(), &model_js(&diagnostics)?)?;
            files.push(&file);
        }
        manifest["files"] = serde_json::Value::Array(artifacts);
        js_sys::Reflect::set(&result, &"manifest".into(), &model_js(&manifest)?)?;
        js_sys::Reflect::set(&result, &"files".into(), &files)?;
        Ok(result.into())
    }
    pub fn apply(&self, edits: JsValue) -> Result<WasmScene, JsValue> {
        let edits: Vec<SceneEdit> = from_js(edits)?;
        Ok(Self {
            document: SceneDocument::new(
                edit_scene(self.document.scene(), &edits, &SceneLimits::browser()).map_err(model_error)?,
                SceneLimits::browser(),
            )
            .map_err(model_error)?,
            diagnostics: self.diagnostics.clone(),
        })
    }
    pub fn extract(&self, node: &str) -> Result<WasmScene, JsValue> {
        Ok(Self {
            document: SceneDocument::new(
                extract_subtree(self.document.scene(), node, &SceneLimits::browser()).map_err(model_error)?,
                SceneLimits::browser(),
            )
            .map_err(model_error)?,
            diagnostics: self.diagnostics.clone(),
        })
    }
    pub fn diff(&self, after: &WasmScene) -> Result<JsValue, JsValue> {
        model_js(&diff_scenes(self.document.scene(), after.document.scene()))
    }
    /// Explicit full neutral document interchange; inspection avoids this copy.
    pub fn to_json(&self) -> Result<String, JsValue> {
        serde_json::to_string(self.document.scene()).map_err(model_error)
    }
}

fn export_model_scene(
    scene: &Scene,
    target: &str,
    options: &ModelExportOptions,
    limits: &SceneLimits,
) -> Result<SceneExport, SceneError> {
    if !matches!(target, "glb" | "gltf")
        && (options.texture_tier.is_some()
            || options.include_hidden.is_some()
            || options.optimize_vertex_cache.is_some())
    {
        return Err(SceneError::new(
            "export.options",
            target,
            "glTF preview options require glb/gltf target",
        ));
    }
    match target {
        "usdz" | "usda" | "usdc" => usd_toolbox_usd::scene::export_scene(
            scene,
            &SceneExportOptions {
                format: match target {
                    "usda" => usd_toolbox_usd::UsdFormat::Usda,
                    "usdc" => usd_toolbox_usd::UsdFormat::Usdc,
                    _ => usd_toolbox_usd::UsdFormat::Usdz,
                },
                limits: limits.clone(),
                ..Default::default()
            },
        ),
        "glb" | "gltf" => usd_toolbox_gltf::scene::export_scene(
            scene,
            &SceneGltfOptions {
                format: if target == "glb" {
                    usd_toolbox_gltf::GltfFormat::Glb
                } else {
                    usd_toolbox_gltf::GltfFormat::Gltf
                },
                texture_tier: options.texture_tier.unwrap_or(usd_toolbox_core::Tier::K2),
                include_hidden: options.include_hidden.unwrap_or(false),
                optimize_vertex_cache: options.optimize_vertex_cache.unwrap_or(false),
                limits: limits.clone(),
            },
        ),
        _ => Err(SceneError::new("export.target", target, "unknown model export target")),
    }
}

fn model_error(error: impl std::fmt::Display + 'static) -> JsValue {
    let object = js_sys::Error::new(&error.to_string());
    if let Some(scene) = (&error as &dyn std::any::Any).downcast_ref::<SceneError>() {
        let _ = js_sys::Reflect::set(&object, &"code".into(), &scene.code.clone().into());
        let details = js_sys::Object::new();
        let _ = js_sys::Reflect::set(&details, &"path".into(), &scene.path.clone().into());
        let _ = js_sys::Reflect::set(&details, &"detail".into(), &scene.detail.clone().into());
        let _ = js_sys::Reflect::set(&object, &"details".into(), &details);
    }
    object.into()
}

fn model_js<T: serde::Serialize + ?Sized>(value: &T) -> Result<JsValue, JsValue> {
    value
        .serialize(&serde_wasm_bindgen::Serializer::json_compatible())
        .map_err(model_error)
}

#[wasm_bindgen]
pub fn import_model(bytes: &[u8]) -> Result<WasmScene, JsValue> {
    let imported = if is_gltf("model.usd", bytes) {
        usd_toolbox_gltf::scene::import_scene(
            bytes,
            "model.glb",
            &usd_toolbox_gltf::scene::SceneGltfImportOptions {
                limits: SceneLimits::browser(),
                ..Default::default()
            },
        )
    } else {
        usd_toolbox_usd::scene::import_scene(
            Input::Bytes(bytes),
            &SceneImportOptions {
                limits: SceneLimits::browser(),
            },
        )
    }
    .map_err(model_error)?;
    Ok(WasmScene {
        document: SceneDocument::new(imported.scene, SceneLimits::browser()).map_err(model_error)?,
        diagnostics: imported.diagnostics,
    })
}

#[derive(serde::Deserialize)]
struct Asset {
    name: String,
    bytes: Vec<u8>,
}

#[wasm_bindgen]
pub fn import_model_bundle(root_name: &str, root_bytes: &[u8], assets: JsValue) -> Result<WasmScene, JsValue> {
    open_model_bundle(root_name, root_bytes, assets, None)
}

/// Open with selected metadata indexes and bounded query budgets. Browser scene
/// limits remain fixed; optional settings control document metadata/query work.
#[wasm_bindgen]
pub fn open_model_bundle(
    root_name: &str,
    root_bytes: &[u8],
    assets: JsValue,
    options: Option<JsValue>,
) -> Result<WasmScene, JsValue> {
    let mut options: DocumentOptions = options_from_js(options)?;
    options.scene_limits = SceneLimits::browser();
    let assets: Vec<Asset> = from_js(assets)?;
    if is_gltf(root_name, root_bytes) {
        let mut files = std::collections::BTreeMap::from([(root_name.to_owned(), root_bytes.to_vec())]);
        for asset in assets {
            if files.insert(asset.name, asset.bytes).is_some() {
                return Err(JsError::new("duplicate bundle asset").into());
            }
        }
        let imported = usd_toolbox_gltf::scene::import_scene_bundle(
            root_name,
            &files,
            &usd_toolbox_gltf::scene::SceneGltfImportOptions {
                limits: SceneLimits::browser(),
                ..Default::default()
            },
        )
        .map_err(model_error)?;
        return Ok(WasmScene {
            document: SceneDocument::with_options(imported.scene, options).map_err(model_error)?,
            diagnostics: imported.diagnostics,
        });
    }
    let mut files = vec![InputFile {
        name: root_name,
        bytes: root_bytes,
    }];
    files.extend(assets.iter().map(|a| InputFile {
        name: &a.name,
        bytes: &a.bytes,
    }));
    let imported = usd_toolbox_usd::scene::import_scene(
        Input::Bundle(&files),
        &SceneImportOptions {
            limits: SceneLimits::browser(),
        },
    )
    .map_err(model_error)?;
    Ok(WasmScene {
        document: SceneDocument::with_options(imported.scene, options).map_err(model_error)?,
        diagnostics: imported.diagnostics,
    })
}

fn is_gltf(name: &str, bytes: &[u8]) -> bool {
    bytes.starts_with(b"glTF")
        || name.to_ascii_lowercase().ends_with(".gltf")
        || name.to_ascii_lowercase().ends_with(".glb")
}

#[wasm_bindgen]
pub fn model_capabilities() -> Result<JsValue, JsValue> {
    let mut core = scene_capabilities();
    core.limits = SceneLimits::browser();
    model_js(
        &serde_json::json!({"schema":"usd-toolbox.model-capabilities.v1", "version":env!("CARGO_PKG_VERSION"), "core":core, "usd":usd_toolbox_usd::scene::usd_scene_capabilities(), "gltf":usd_toolbox_gltf::scene::gltf_scene_capabilities(), "geometry_lods":{"schema":"usd-toolbox.geometry-lods.v1","formats":["glb","usdz"],"error_policy":"original_vertex_displacement"}, "runtime":{"wasm":true,"threads":false,"host_io":false}}),
    )
}

#[wasm_bindgen]
pub fn preflight_model_bundle(
    root_name: &str,
    root_bytes: &[u8],
    assets: JsValue,
    options: Option<JsValue>,
) -> Result<JsValue, JsValue> {
    let assets: Vec<Asset> = from_js(assets)?;
    let mut options: usd_toolbox_usd::scene::PreflightOptions = options_from_js(options)?;
    options.limits = SceneLimits::browser();
    if is_gltf(root_name, root_bytes) {
        return model_js(&preflight_gltf_bundle(root_name, root_bytes, assets, &options).map_err(model_error)?);
    }
    let mut files = vec![InputFile {
        name: root_name,
        bytes: root_bytes,
    }];
    files.extend(assets.iter().map(|asset| InputFile {
        name: &asset.name,
        bytes: &asset.bytes,
    }));
    model_js(&usd_toolbox_usd::scene::preflight(Input::Bundle(&files), &options))
}

#[wasm_bindgen]
pub fn model_from_json(json: &str) -> Result<WasmScene, JsValue> {
    if json.len() > SceneLimits::browser().max_bytes {
        return Err(JsError::new("scene JSON exceeds browser limit").into());
    }
    let scene: Scene = serde_json::from_str(json).map_err(model_error)?;
    require_valid_scene(&scene, &SceneLimits::browser()).map_err(model_error)?;
    Ok(WasmScene {
        document: SceneDocument::new(scene, SceneLimits::browser()).map_err(model_error)?,
        diagnostics: Vec::new(),
    })
}

fn preflight_gltf_bundle(
    root_name: &str,
    root_bytes: &[u8],
    assets: Vec<Asset>,
    options: &usd_toolbox_usd::scene::PreflightOptions,
) -> Result<serde_json::Value, SceneError> {
    let total = assets
        .iter()
        .try_fold(root_bytes.len(), |total, asset| total.checked_add(asset.bytes.len()));
    if assets.len() > options.limits.max_dependencies || total.is_none_or(|total| total > options.limits.max_bytes) {
        return Err(SceneError::new(
            "scene.limit",
            root_name,
            "preflight bundle exceeds configured byte/dependency limits",
        ));
    }
    let mut names = std::collections::BTreeSet::from([root_name]);
    for asset in &assets {
        if !names.insert(&asset.name) {
            return Err(SceneError::new(
                "input.duplicate",
                &asset.name,
                "duplicate bundle asset",
            ));
        }
    }
    let mut report = usd_toolbox_gltf::scene::preflight_scene(root_bytes, root_name, &options.limits)?;
    let imported = || {
        let mut files = std::collections::BTreeMap::from([(root_name.to_owned(), root_bytes.to_vec())]);
        files.extend(assets.into_iter().map(|asset| (asset.name, asset.bytes)));
        usd_toolbox_gltf::scene::import_scene_bundle(
            root_name,
            &files,
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
mod preflight_tests {
    use super::*;

    #[test]
    fn gltf_profile_uses_supplied_assets_geometry_and_caps() {
        let scene = usd_toolbox_usd::scene::import_scene(
            Input::Bytes(include_bytes!("../../../fixtures/models/metadata-semantics.usda")),
            &Default::default(),
        )
        .unwrap()
        .scene;
        let exported = usd_toolbox_gltf::scene::export_scene(&scene, &Default::default()).unwrap();
        let json_size = u32::from_le_bytes(exported.bytes[12..16].try_into().unwrap()) as usize;
        let mut document: serde_json::Value = serde_json::from_slice(&exported.bytes[20..20 + json_size]).unwrap();
        document["buffers"][0]["uri"] = serde_json::json!("mesh.bin");
        let root_bytes = serde_json::to_vec(&document).unwrap();
        let bin_bytes = exported.bytes[28 + json_size..].to_vec();
        let mut options = usd_toolbox_usd::scene::PreflightOptions {
            limits: SceneLimits::browser(),
            check_geometry: true,
            target: usd_toolbox_usd::scene::PreflightTarget::Glb,
            ..Default::default()
        };
        let failed = preflight_gltf_bundle("model.gltf", &root_bytes, Vec::new(), &options).unwrap();
        assert_eq!(failed["complete"], false);
        assert_eq!(failed["validation"]["geometry"], "failed");
        let assets = vec![Asset {
            name: "mesh.bin".into(),
            bytes: bin_bytes,
        }];
        let report = preflight_gltf_bundle("model.gltf", &root_bytes, assets, &options).unwrap();
        assert_eq!(report["complete"], true);
        assert_eq!(report["validation"]["buffers"], "performed");
        assert_eq!(report["validation"]["geometry"], "performed");
        assert_eq!(report["validation"]["target"], "performed");
        options.check_geometry = false;
        options.max_issues = 1;
        let capped = preflight_gltf_bundle(
            "many.gltf",
            br#"{"asset":{"version":"2.0"},"cameras":[{}],"skins":[{}],"animations":[{}]}"#,
            Vec::new(),
            &options,
        )
        .unwrap();
        assert_eq!(capped["diagnostics"].as_array().unwrap().len(), 1);
        assert_eq!(capped["diagnostics_truncated"], true);
        assert_eq!(capped["complete"], false);
        options.limits.max_bytes = 16;
        assert!(preflight_gltf_bundle("model.gltf", &root_bytes, Vec::new(), &options).is_err());
    }
}
