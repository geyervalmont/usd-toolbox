//! Bounded authored-layer inventory. This deliberately does not require successful
//! scene projection, dependency resolution, shader evaluation, or image decoding.
use super::*;

pub const PREFLIGHT_SCHEMA: &str = "usd-toolbox.preflight.v1";

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PreflightTarget {
    #[default]
    Usd,
    Usdz,
    Glb,
    BrowserPreview,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct PreflightOptions {
    pub limits: SceneLimits,
    pub max_specs: usize,
    pub max_issues: usize,
    pub max_metadata_bytes: usize,
    pub target: PreflightTarget,
    /// Opt in to topology and transform checks. False avoids decoding mesh
    /// attributes from binary crates and never materializes a neutral scene.
    pub check_geometry: bool,
}

impl Default for PreflightOptions {
    fn default() -> Self {
        Self {
            limits: SceneLimits::default(),
            max_specs: 1_000_000,
            max_issues: 256,
            max_metadata_bytes: 4 * 1024 * 1024,
            target: PreflightTarget::Usd,
            check_geometry: false,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PreflightIssue {
    pub code: String,
    pub path: String,
    pub severity: String,
    pub detail: String,
    pub remedy: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PreflightDependency {
    pub authored_path: String,
    pub source: String,
    pub usage: String,
    /// supplied, missing, internal, or unsafe. Supplied bytes are not evaluated
    /// as composed layers. No network or filesystem resolution is attempted.
    pub status: String,
    pub bytes: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PreflightMetadata {
    pub path: String,
    pub field: String,
    pub property: usd_toolbox_core::scene::MetadataProperty,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct PreflightEstimates {
    pub encoded_bytes: usize,
    pub supplied_dependency_bytes: usize,
    pub meshes: usize,
    pub points: usize,
    pub faces: usize,
    pub corners: usize,
    /// Neutral position/index/count storage, excluding parser, metadata,
    /// attributes, images and scratch allocations. Not a peak memory promise.
    pub minimum_geometry_bytes: usize,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PreflightReport {
    pub schema: String,
    pub target: PreflightTarget,
    /// All authored specs and requested checks were inspected. False includes
    /// parse failure, limits, and deferred target checks after failed projection.
    pub complete: bool,
    pub scanned_specs: usize,
    pub prim_types: BTreeMap<String, usize>,
    pub applied_schemas: BTreeMap<String, usize>,
    pub shader_ids: BTreeMap<String, usize>,
    pub features: BTreeSet<String>,
    pub dependencies: Vec<PreflightDependency>,
    pub metadata: Vec<PreflightMetadata>,
    pub issues: Vec<PreflightIssue>,
    pub performed: Vec<String>,
    pub deferred: Vec<String>,
    pub estimates: PreflightEstimates,
}

impl PreflightReport {
    fn new(options: &PreflightOptions) -> Self {
        Self {
            schema: PREFLIGHT_SCHEMA.into(),
            target: options.target,
            complete: true,
            scanned_specs: 0,
            prim_types: BTreeMap::new(),
            applied_schemas: BTreeMap::new(),
            shader_ids: BTreeMap::new(),
            features: BTreeSet::new(),
            dependencies: Vec::new(),
            metadata: Vec::new(),
            issues: Vec::new(),
            performed: Vec::new(),
            deferred: vec![
                "image decoding and render equivalence".into(),
                "external layer composition".into(),
            ],
            estimates: PreflightEstimates::default(),
        }
    }

    fn issue(
        &mut self,
        options: &PreflightOptions,
        code: &str,
        path: &str,
        severity: &str,
        detail: impl Into<String>,
        remedy: &str,
    ) {
        if self.issues.len() >= options.max_issues.max(1) {
            self.complete = false;
            if !self.deferred.iter().any(|s| s == "diagnostic limit reached") {
                self.deferred.push("diagnostic limit reached".into());
            }
            return;
        }
        self.issues.push(PreflightIssue {
            code: code.into(),
            path: path.into(),
            severity: severity.into(),
            detail: detail.into(),
            remedy: remedy.into(),
        });
    }

    fn failure(&mut self, options: &PreflightOptions, error: SceneError) {
        self.complete = false;
        self.issue(
            options,
            &error.code,
            &error.path,
            "error",
            error.detail,
            "Correct the source or raise the stated limit, then scan again.",
        );
    }
}

/// Inventory an authored layer even if it cannot be imported. Parse errors and
/// limit failures are returned in the report rather than discarding findings.
/// USDA parsing necessarily materializes values; USDC opens lazily. The encoded
/// source limit and nesting guard apply before text parsing; `max_specs` applies
/// after the parser returns its spec index.
pub fn preflight(input: Input<'_>, options: &PreflightOptions) -> PreflightReport {
    let mut report = PreflightReport::new(options);
    let result = (|| {
        let (mut name, mut bytes, mut entries) = read::input_entries(input, &options.limits)?;
        report.estimates.encoded_bytes = bytes.len();
        if bytes.starts_with(b"PK\x03\x04") {
            if !entries.is_empty() {
                return Err(err("usd.bundle", "/", "USDZ must be self-contained"));
            }
            (name, bytes, entries) = read::unpack(&bytes, &options.limits)?;
            report
                .performed
                .push("USDZ archive layout and declared uncompressed byte limits".into());
        }
        entries = read::anchor_entries(&name, entries)?;
        report.estimates.supplied_dependency_bytes = entries.values().map(Vec::len).sum();
        let data: Box<dyn AbstractData> = if bytes.starts_with(USDC_MAGIC) {
            Box::new(
                CrateData::open_with_limits(Cursor::new(&bytes), true, crate_limits(&options.limits))
                    .map_err(|e| err("usd.decode", &name, e))?,
            )
        } else {
            let source = std::str::from_utf8(&bytes).map_err(|e| err("usd.decode", &name, e))?;
            guard_text_nesting(source, options.limits.max_depth)?;
            Box::new(openusd::usda::parse(source).map_err(|e| err("usd.decode", &name, e))?)
        };
        scan(data.as_ref(), &entries, options, &mut report)?;
        report
            .performed
            .push("authored layer, schema, shader, dependency and metadata inventory".into());
        if options.check_geometry {
            target_checks(&name, &bytes, &entries, options, &mut report);
        } else {
            report
                .deferred
                .push("geometry, hierarchy, evaluated shading and target exportability (check_geometry=false)".into());
        }
        Ok(())
    })();
    if let Err(error) = result {
        report.failure(options, error);
    }
    report
}

fn scan(
    data: &dyn AbstractData,
    entries: &BTreeMap<String, Vec<u8>>,
    options: &PreflightOptions,
    report: &mut PreflightReport,
) -> Result<(), SceneError> {
    let mut specs = data.spec_paths();
    specs.sort_by(|a, b| a.as_str().cmp(b.as_str()));
    let mut metadata_bytes = 0usize;
    let mut dependency_keys = BTreeSet::new();
    let mut fields_seen = 0usize;
    let max_fields = options.max_specs.saturating_mul(64);
    let mut metadata_exhausted = false;
    let mut inactive_roots = BTreeSet::new();
    for p in &specs {
        if report.scanned_specs >= options.max_specs {
            report.failure(
                options,
                err("preflight.spec_limit", p.as_str(), "authored spec scan limit reached"),
            );
            break;
        }
        report.scanned_specs += 1;
        if field(data, p, "active")? == Some(UsdValue::Bool(false)) {
            inactive_roots.insert(p.as_str().to_owned());
        }
        let mut ancestor = Some(p.prim_path());
        let mut inactive = false;
        while let Some(path) = ancestor {
            if inactive_roots.contains(path.as_str()) {
                inactive = true;
                break;
            }
            ancestor = path.parent();
        }
        let unsupported_severity = if inactive { "info" } else { "error" };
        if data.spec_type(p) == Some(SpecType::Prim) {
            let kind = token(field(data, p, "typeName")?).unwrap_or_default();
            *report.prim_types.entry(kind.clone()).or_default() += 1;
            if !matches!(
                kind.as_str(),
                "" | "Xform" | "Scope" | "Mesh" | "GeomSubset" | "Material" | "Shader" | "NodeGraph"
            ) {
                report.issue(
                    options,
                    "usd.prim_unsupported",
                    p.as_str(),
                    unsupported_severity,
                    format!("{kind} is not projected by the static polygon profile"),
                    "Convert to a supported mesh or preserve this object in the original source.",
                );
            }
            if kind == "Mesh" {
                report.estimates.meshes += 1;
                let scheme = token(value(data, p, "subdivisionScheme")?).unwrap_or_else(|| "catmullClark".into());
                if scheme != "none" {
                    report.features.insert("subdivision".into());
                    report.issue(options, "usd.subdivision_unsupported", p.as_str(), unsupported_severity, format!("subdivisionScheme={scheme}"), "Tessellate with a declared tolerance; only author none when the source really is polygon geometry.");
                }
                for feature in ["holeIndices", "creaseIndices", "cornerIndices"] {
                    if value(data, p, feature)?.is_some_and(|v| !matches!(v, UsdValue::IntVec(ref v) if v.is_empty())) {
                        report.features.insert(feature.into());
                        report.issue(
                            options,
                            "usd.mesh_feature_unsupported",
                            p.as_str(),
                            unsupported_severity,
                            feature,
                            "Tessellate the authored surface with an explicit policy before import.",
                        );
                    }
                }
            }
            if kind == "Shader" {
                let id = token(value(data, p, "info:id")?).unwrap_or_else(|| "<unresolved>".into());
                *report.shader_ids.entry(id.clone()).or_default() += 1;
                if !matches!(
                    id.as_str(),
                    "UsdPreviewSurface" | "UsdUVTexture" | "UsdPrimvarReader_float2" | "UsdTransform2d"
                ) {
                    report.issue(
                        options,
                        "usd.shader_unsupported",
                        p.as_str(),
                        "warning",
                        format!("shader {id} cannot be evaluated by the portable preview profile"),
                        "Supply a PreviewSurface conversion and retain the source graph for fidelity.",
                    );
                }
            }
        }
        let mut fields = data.list_fields(p).unwrap_or_default();
        fields.sort_by(|a, b| a.as_str().cmp(b.as_str()));
        for f in fields {
            fields_seen += 1;
            if fields_seen > max_fields {
                report.failure(
                    options,
                    err("preflight.work_limit", p.as_str(), "field scan work limit reached"),
                );
                return Ok(());
            }
            let feature = match f.as_str() {
                "references" => Some("references"),
                "payload" => Some("payloads"),
                "subLayers" => Some("sublayers"),
                "inheritPaths" => Some("inherits"),
                "specializes" => Some("specializes"),
                "relocates" => Some("relocates"),
                "variantSetNames" | "variantSelection" => Some("variants"),
                "timeSamples" => Some("animation"),
                _ => None,
            };
            if let Some(feature) = feature {
                report.features.insert(feature.into());
                report.issue(options, if feature == "animation" { "usd.animation_unsupported" } else { "usd.unsupported_composition" }, p.as_str(), unsupported_severity, format!("authored {feature} ({f})"), "Flatten and evaluate an explicitly selected static stage with an optional host OpenUSD adapter; portable import does not compose opinions.");
            }
            if f.as_str() == "instanceable" && field(data, p, "instanceable")? == Some(UsdValue::Bool(true)) {
                report.features.insert("native_instances".into());
                report.issue(options, "usd.instancing_unsupported", p.as_str(), unsupported_severity, "instanceable prim", "Resolve prototypes with a composition-capable host; retain shared geometry in the resulting scene.");
            }
            if f.as_str() == "active" && field(data, p, "active")? == Some(UsdValue::Bool(false)) {
                report.features.insert("inactive_prims".into());
                report.issue(
                    options,
                    "usd.inactive_omitted",
                    p.as_str(),
                    "warning",
                    "inactive subtree is omitted from static projection",
                    "Retain the original layer if inactive authored opinions must remain editable.",
                );
            }
            if f.as_str() == "apiSchemas" {
                let schemas = match field(data, p, f.as_str())? {
                    Some(UsdValue::TokenListOp(v)) => v.compose_over(&[]),
                    Some(UsdValue::TokenVec(v)) => v,
                    _ => Vec::new(),
                };
                for schema in schemas {
                    *report.applied_schemas.entry(schema.to_string()).or_default() += 1;
                    if schema.as_str() != "MaterialBindingAPI" {
                        report.issue(
                            options,
                            "usd.schema_unsupported",
                            p.as_str(),
                            unsupported_severity,
                            schema.to_string(),
                            "Remove or project this schema only with an explicit conversion policy.",
                        );
                    }
                }
            }
            // Avoid loading mesh buffers in metadata mode. Asset-valued defaults
            // are identified from authored typeName before asking a lazy crate.
            let type_name = token(field(data, p, "typeName")?).unwrap_or_default();
            let scan_assets = matches!(
                f.as_str(),
                "references" | "payload" | "subLayers" | "assetInfo" | "customData" | "customLayerData"
            ) || (f.as_str() == "default" && matches!(type_name.as_str(), "asset" | "asset[]"));
            if scan_assets && let Some(v) = field(data, p, f.as_str())? {
                let mut assets = Vec::new();
                collect_assets(&v, f.as_str(), &mut assets, options.limits.max_dependencies, 0)?;
                for name in assets {
                    if !dependency_keys.insert((p.as_str().to_owned(), name.clone())) {
                        continue;
                    }
                    if report.dependencies.len() >= options.limits.max_dependencies {
                        report.failure(
                            options,
                            err("preflight.dependency_limit", p.as_str(), "dependency count exceeded"),
                        );
                        return Ok(());
                    }
                    let status = if name.is_empty() {
                        "internal"
                    } else if validate_entry_name(&name).is_err() {
                        "unsafe"
                    } else if entries.contains_key(&name) {
                        "supplied"
                    } else {
                        "missing"
                    };
                    let usage = if matches!(f.as_str(), "assetInfo" | "customData" | "customLayerData") {
                        "metadata"
                    } else if matches!(f.as_str(), "references" | "payload" | "subLayers") {
                        "composition"
                    } else {
                        "asset_attribute"
                    };
                    if matches!(status, "missing" | "unsafe") && usage != "metadata" {
                        report.issue(options, if status == "unsafe" { "usd.dependency_path" } else { "usd.missing_dependency" }, p.as_str(), unsupported_severity, format!("{status} dependency {name}"), "Supply confined root-relative dependency bytes explicitly; network and parent-directory resolution are disabled.");
                    }
                    report.dependencies.push(PreflightDependency {
                        bytes: entries.get(&name).map(|b| b.len() as u64),
                        authored_path: name,
                        source: p.as_str().into(),
                        usage: usage.into(),
                        status: status.into(),
                    });
                }
            }
            if !metadata_exhausted
                && source_metadata::is_metadata_field(data, p, f.as_str())?
                && let Some(v) = field(data, p, f.as_str())?
            {
                match source_metadata::property(
                    p,
                    f.as_str(),
                    &type_name,
                    &v,
                    options.max_metadata_bytes.saturating_sub(metadata_bytes),
                ) {
                    Ok(property) => {
                        let size = serde_json::to_vec(&property)
                            .map_err(|e| err("usd.metadata", p.as_str(), e))?
                            .len();
                        if size <= options.max_metadata_bytes.saturating_sub(metadata_bytes) {
                            metadata_bytes += size;
                            report.metadata.push(PreflightMetadata {
                                path: p.as_str().into(),
                                field: f.to_string(),
                                property,
                            });
                        } else {
                            metadata_exhausted = true;
                            report.failure(
                                options,
                                err(
                                    "preflight.metadata_limit",
                                    p.as_str(),
                                    "metadata inventory byte limit reached",
                                ),
                            );
                        }
                    }
                    Err(error) if error.code == "usd.metadata_limit" => {
                        metadata_exhausted = true;
                        report.failure(options, error);
                    }
                    Err(error) => report.issue(
                        options,
                        &error.code,
                        &error.path,
                        "warning",
                        error.detail,
                        "Keep original bytes; this value type cannot be preserved in portable typed metadata.",
                    ),
                }
            }
        }
        if let Some(name) = property_name(p)
            && (name.starts_with("material:binding:") || name.starts_with("collection:"))
        {
            report.issue(
                options,
                "usd.binding_unsupported",
                p.as_str(),
                unsupported_severity,
                "collection or purpose-specific material binding",
                "Resolve the requested render-purpose binding with a host USD stage before import.",
            );
        }
    }
    Ok(())
}

fn collect_assets(
    value: &UsdValue,
    field_name: &str,
    out: &mut Vec<String>,
    max: usize,
    depth: usize,
) -> Result<(), SceneError> {
    if depth > 64 {
        return Err(err(
            "preflight.metadata_limit",
            "/",
            "dependency dictionary depth exceeds 64",
        ));
    }
    match value {
        UsdValue::AssetPath(v) => out.push(v.authored_path.clone()),
        UsdValue::AssetPathVec(v) => out.extend(v.iter().take(max.saturating_add(1)).map(|v| v.authored_path.clone())),
        UsdValue::StringVec(v) if field_name == "subLayers" => {
            out.extend(v.iter().take(max.saturating_add(1)).cloned())
        }
        UsdValue::ReferenceListOp(v) => out.extend(
            v.compose_over(&[])
                .iter()
                .take(max.saturating_add(1))
                .map(|r| r.asset_path.clone()),
        ),
        UsdValue::PayloadListOp(v) => out.extend(
            v.compose_over(&[])
                .iter()
                .take(max.saturating_add(1))
                .map(|r| r.asset_path.clone()),
        ),
        UsdValue::Payload(v) => out.push(v.asset_path.clone()),
        UsdValue::Dictionary(v) => {
            for v in v.values() {
                collect_assets(v, field_name, out, max, depth + 1)?;
            }
        }
        _ => {}
    }
    if out.len() > max {
        return Err(err(
            "preflight.dependency_limit",
            "/",
            "too many dependency occurrences",
        ));
    }
    Ok(())
}

fn target_checks(
    name: &str,
    bytes: &[u8],
    entries: &BTreeMap<String, Vec<u8>>,
    options: &PreflightOptions,
    report: &mut PreflightReport,
) {
    let mut files = vec![usd_toolbox_core::InputFile {
        name: name.rsplit('/').next().unwrap_or(name),
        bytes,
    }];
    files.extend(
        entries
            .iter()
            .map(|(name, bytes)| usd_toolbox_core::InputFile { name, bytes }),
    );
    let result = import_scene(
        Input::Bundle(&files),
        &SceneImportOptions {
            limits: options.limits.clone(),
        },
    );
    let scene = match result {
        Ok(result) => {
            for d in result.diagnostics {
                report.issue(
                    options,
                    &d.code,
                    &d.path,
                    "warning",
                    d.detail,
                    "Review this documented projection before publishing.",
                );
            }
            result.scene
        }
        Err(error) => {
            if !report
                .issues
                .iter()
                .any(|i| i.code == error.code && i.path == error.path)
            {
                report.failure(options, error);
            }
            report.complete = false;
            report
                .deferred
                .push("evaluated target checks: strict static projection failed".into());
            return;
        }
    };
    report
        .performed
        .push("static scene projection, topology, hierarchy, units and dependency integrity".into());
    for mesh in scene.meshes.values() {
        report.estimates.points = report.estimates.points.saturating_add(mesh.positions.len());
        report.estimates.faces = report.estimates.faces.saturating_add(mesh.face_counts.len());
        report.estimates.corners = report.estimates.corners.saturating_add(mesh.face_indices.len());
    }
    report.estimates.minimum_geometry_bytes = report.estimates.points.saturating_mul(24).saturating_add(
        report
            .estimates
            .faces
            .saturating_add(report.estimates.corners)
            .saturating_mul(4),
    );
    if matches!(options.target, PreflightTarget::Glb | PreflightTarget::BrowserPreview) {
        for (id, mesh) in &scene.meshes {
            let mut offset = 0;
            for (face, &count) in mesh.face_counts.iter().enumerate() {
                let end = offset + count as usize;
                let points: Vec<_> = mesh.face_indices[offset..end]
                    .iter()
                    .map(|&i| mesh.positions[i as usize])
                    .collect();
                if let Err(error) = triangulate(&points) {
                    report.issue(
                        options,
                        &error.code,
                        &format!("{id}/faces/{face}"),
                        "error",
                        error.detail,
                        "Explicitly repair or tessellate this polygon before preview export.",
                    );
                }
                offset = end;
            }
        }
        for (id, node) in &scene.nodes {
            let m = node.transform;
            let cols: [[f64; 3]; 3] = std::array::from_fn(|c| std::array::from_fn(|r| m[r][c]));
            let dot = |a: [f64; 3], b: [f64; 3]| a.iter().zip(b).map(|(a, b)| a * b).sum::<f64>();
            if (0..3).any(|a| {
                (a + 1..3).any(|b| {
                    dot(cols[a], cols[b]).abs() > 1e-7 * dot(cols[a], cols[a]).sqrt() * dot(cols[b], cols[b]).sqrt()
                })
            }) {
                report.issue(
                    options,
                    "gltf.shear",
                    id,
                    "error",
                    "local transform contains shear",
                    "Bake the transform into geometry explicitly before GLB export.",
                );
            }
        }
        report
            .performed
            .push("polygon triangulation and local shear checks".into());
        report
            .deferred
            .push("actual GLB encoding and decoded image limits (run export to establish exportability)".into());
    }
}

/// Cheap text guard before recursive parsing. Delimiters inside quoted strings,
/// asset paths, path references and comments do not contribute to nesting.
pub(crate) fn guard_text_nesting(text: &str, maximum: usize) -> Result<(), SceneError> {
    let bytes = text.as_bytes();
    let mut i = 0usize;
    let mut depth = 0usize;
    while i < bytes.len() {
        let ch = bytes[i];
        if bytes.get(i..i + 2) == Some(b"/*") {
            i += 2;
            let mut branches = 0usize;
            while i < bytes.len() && bytes.get(i..i + 2) != Some(b"*/") {
                if bytes[i] == b'*' {
                    branches += 1;
                }
                if branches > 512 {
                    return Err(err(
                        "usd.text_token_limit",
                        "/",
                        "block comment exceeds safe lexer complexity",
                    ));
                }
                i += 1;
            }
            i = i.saturating_add(2);
            continue;
        }
        if ch == b'#' {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if matches!(ch, b'"' | b'\'' | b'@' | b'<') {
            let close = if ch == b'<' { b'>' } else { ch };
            let triple = matches!(ch, b'"' | b'\'' | b'@') && bytes.get(i..i + 3).is_some_and(|v| v == [ch, ch, ch]);
            let count = if triple { 3 } else { 1 };
            i += count;
            let mut lexical_branches = 0usize;
            while i < bytes.len() {
                if matches!(bytes[i], b'\\' | b'"' | b'\'') {
                    lexical_branches += 1;
                    if lexical_branches > 512 {
                        return Err(err(
                            "usd.text_token_limit",
                            "/",
                            "string exceeds safe escaped/quoted token complexity; use USDC or split the authored value",
                        ));
                    }
                }
                if bytes[i] == b'\\' {
                    i = i.saturating_add(2);
                    continue;
                }
                if bytes[i] == close && (!triple || bytes.get(i..i + 3).is_some_and(|v| v == [close, close, close])) {
                    i += count;
                    break;
                }
                i += 1;
            }
            continue;
        }
        match ch {
            b'{' | b'[' | b'(' => {
                depth += 1;
                if depth > maximum {
                    return Err(err(
                        "usd.nesting_limit",
                        "/",
                        format!("text nesting exceeds {maximum} before parsing"),
                    ));
                }
            }
            b'}' | b']' | b')' => depth = depth.saturating_sub(1),
            _ => {}
        }
        i += 1;
    }
    Ok(())
}

/// Portable authored-layer capabilities. Versioned independently from package
/// versions; host composition/preprocessing remains an explicit external adapter.
pub fn usd_scene_capabilities() -> serde_json::Value {
    serde_json::json!({
        "schema":"usd-toolbox.usd-capabilities.v1",
        "profile":"static-polygon-v2",
        "portable":true,
        "host_io":false,
        "preflight":{"authored_layer":true,"accumulated_issues":true,"metadata_without_scene":true,"lazy_crate_geometry":true,"text_parser_materializes_values":true,"targets":["usd","usdz","glb","browser_preview"]},
        "input":["usda","usdc","usdz","caller_bundle"],
        "supported":["mesh","hierarchy","transform_stacks","inactive_subtree_pruning","constant_inherited_primvars","direct_binding_strength","preview_surface","shader_interface_forwarding","typed_source_metadata","archived_source_graphs","tangent_primvars","cutout_opacity","texture_wrap_repeat_clamp_mirror","uv_sets_st_through_st31","single_usd_transform2d"],
        "unsupported":["general_composition","native_instances","point_instancers","animation","subdivision_evaluation","collection_bindings","purpose_bindings","arbitrary_shader_evaluation","exr_decode","udim"],
        "composition_adapter":{
            "required":false,
            "execution":"caller supplied, explicit preprocessing outside portable core",
            "contract":["select variants and render purpose explicitly","declare payload load policy and resolver context","flatten a composed stage using USD opinion strength","evaluate a declared time","record adapter version and source hashes","report removed or approximated schemas","feed self-contained static layer and dependency bytes back to portable preflight"],
            "implementation":{"kind":"optional_native_host_adapter","entrypoint":"scripts/normalize_usd.py","runtime":"externally installed OpenUSD Python bindings","portable_dependency":false}
        }
    })
}
