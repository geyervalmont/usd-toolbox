use super::*;
use usd_toolbox_core::{InputFile, scene::*};
use usd_toolbox_gltf::scene::SceneGltfOptions;
use usd_toolbox_usd::scene::{SceneExportOptions, SceneImportOptions, asset_dependencies, import_scene};

#[derive(Debug, Args)]
pub(super) struct ModelArgs {
    #[command(subcommand)]
    command: ModelCommand,
}

#[derive(Debug, Subcommand)]
enum ModelCommand {
    /// Query projected node metadata with bounded pagination and structured filters.
    Query(QueryArgs),
    /// Read typed scene, node, mesh or material metadata.
    Metadata(MetadataArgs),
    /// Scan authored USD without requiring successful model import.
    Preflight(PreflightArgs),
    /// Discover supported model operations and portable format profiles.
    Capabilities {
        #[arg(long)]
        output: Option<PathBuf>,
    },
    /// Inspect counts, scale, bounds, materials and dependencies without raw buffers.
    Inspect(ReadArgs),
    /// Validate a static scene and write diagnostics; failure has nonzero status.
    Validate(ReadArgs),
    /// Create a self-contained USDZ model and integrity report.
    Pack(WriteArgs),
    /// Export a scene as usdz, usda, usdc, glb, gltf or scene-json.
    Export(ExportModelArgs),
    /// Generate bounded independent geometry levels into a new package directory.
    Lod(LodArgs),
    /// Apply an atomic array of scene edits to a new USDZ revision.
    Apply(EditArgs),
    /// Extract a subtree into a new USDZ while preserving world placement.
    Extract(ExtractArgs),
    /// Compare scene entities and fields without dumping geometry arrays.
    Diff(DiffArgs),
}

#[derive(Debug, Args)]
struct ReadArgs {
    #[arg(long)]
    input: PathBuf,
    #[arg(long, alias = "report")]
    output: Option<PathBuf>,
}
#[derive(Debug, Args)]
struct QueryArgs {
    #[command(flatten)]
    files: ReadArgs,
    /// QueryRequest JSON file; defaults to the first page of node headers.
    #[arg(long)]
    request: Option<PathBuf>,
    /// Return only the matching count.
    #[arg(long)]
    count: bool,
}
#[derive(Debug, Args)]
struct MetadataArgs {
    #[command(flatten)]
    files: ReadArgs,
    #[arg(long, conflicts_with_all = ["mesh", "material"])]
    node: Option<String>,
    #[arg(long, conflicts_with_all = ["node", "material"])]
    mesh: Option<String>,
    #[arg(long, conflicts_with_all = ["node", "mesh"])]
    material: Option<String>,
    /// Read one property instead of the entire metadata record.
    #[arg(long)]
    key: Option<String>,
    #[arg(long, default_value = "normalized", value_parser = ["normalized", "original"])]
    namespace: String,
}
#[derive(Debug, Args)]
struct PreflightArgs {
    #[command(flatten)]
    files: ReadArgs,
    #[arg(long, default_value = "usd", value_parser = ["usd", "usdz", "glb", "browser_preview"])]
    target: String,
    /// Include expensive geometry/transform checks when projection is supported.
    #[arg(long)]
    geometry: bool,
    #[arg(long, default_value_t = 256)]
    max_issues: usize,
}
#[derive(Debug, Args)]
struct WriteArgs {
    #[arg(long)]
    input: PathBuf,
    #[arg(long)]
    output: PathBuf,
    #[arg(long)]
    report: Option<PathBuf>,
    /// Stable catalogue identity for a newly packaged model.
    #[arg(long)]
    id: Option<String>,
    #[arg(long)]
    name: Option<String>,
    /// JSON array of source_material, package, sha256 and optional material_id.
    #[arg(long)]
    material_bindings: Option<PathBuf>,
}
#[derive(Debug, Args)]
struct ExportModelArgs {
    #[command(flatten)]
    files: WriteArgs,
    #[arg(long)]
    target: String,
    #[arg(long)]
    tier: Option<String>,
    /// Optimize opaque GLB/glTF vertex-cache order when simulated cache misses improve.
    #[arg(long)]
    optimize_vertex_cache: bool,
}
#[derive(Debug, Args)]
struct LodArgs {
    #[arg(long)]
    input: PathBuf,
    /// SceneLodOptions JSON with explicit ratios and displacement limits.
    #[arg(long)]
    options: PathBuf,
    /// Must be a new path; existing directories/files are never overwritten.
    #[arg(long)]
    output_dir: PathBuf,
    #[arg(long, default_value = "glb", value_parser = ["glb", "usdz"])]
    target: String,
}
#[derive(Debug, Args)]
struct EditArgs {
    #[command(flatten)]
    files: WriteArgs,
    #[arg(long)]
    edits: PathBuf,
}
#[derive(Debug, Args)]
struct ExtractArgs {
    #[command(flatten)]
    files: WriteArgs,
    #[arg(long)]
    node: String,
}

type ModelResult<T> = Result<T, Box<dyn std::error::Error>>;

pub(super) fn run(args: ModelArgs) -> ModelResult<()> {
    let limits = SceneLimits::default();
    match args.command {
        ModelCommand::Query(args) => {
            let input = load(&args.files.input)?;
            let document = SceneDocument::new(input.scene, limits)?;
            let request: QueryRequest = args.request.as_deref().map(read_json).transpose()?.unwrap_or_default();
            if args.count {
                write_json(
                    args.files.output.as_deref(),
                    &serde_json::json!({"schema":"usd-toolbox.query-count.v1","count":document.count(&request.filter)?}),
                    "write query count",
                )?;
            } else {
                write_json(args.files.output.as_deref(), &document.query(&request)?, "write query")?;
            }
        }
        ModelCommand::Metadata(args) => {
            let input = load(&args.files.input)?;
            let document = SceneDocument::new(input.scene, limits)?;
            let target = if let Some(id) = args.node {
                MetadataTarget::Node(id)
            } else if let Some(id) = args.mesh {
                MetadataTarget::Mesh(id)
            } else if let Some(id) = args.material {
                MetadataTarget::Material(id)
            } else {
                MetadataTarget::Scene
            };
            if let Some(key) = args.key {
                let namespace = if args.namespace == "original" {
                    MetadataNamespace::Original
                } else {
                    MetadataNamespace::Normalized
                };
                write_json(
                    args.files.output.as_deref(),
                    &document.get_property(&target, namespace, &key)?,
                    "write property",
                )?;
            } else {
                write_json(
                    args.files.output.as_deref(),
                    &document.get_metadata(&target)?,
                    "write metadata",
                )?;
            }
        }
        ModelCommand::Preflight(args) => {
            let bytes = read_bounded(&args.files.input, limits.max_bytes)?;
            let name = args
                .files
                .input
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("model.usd");
            if bytes.starts_with(b"glTF")
                || name.to_ascii_lowercase().ends_with(".gltf")
                || name.to_ascii_lowercase().ends_with(".glb")
            {
                let options = usd_toolbox_usd::scene::PreflightOptions {
                    limits: limits.clone(),
                    target: serde_json::from_value(serde_json::Value::String(args.target))?,
                    max_issues: args.max_issues,
                    check_geometry: args.geometry,
                    ..Default::default()
                };
                let mut report = usd_toolbox_gltf::scene::preflight_scene(&bytes, name, &limits)?;
                let imported = || load(&args.files.input);

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
                                report["validation"]["integrity"] =
                                    serde_json::json!("performed_source_and_dependency_validation");
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
                                            report["target_diagnostics"] = serde_json::to_value(exported.diagnostics)
                                                .expect("diagnostics are serializable");
                                            report["validation"]["target"] = serde_json::json!("performed");
                                            report["validation"]["images"] =
                                                serde_json::json!("performed_preview_decode");
                                        }
                                        Err(error) => {
                                            report["compatible"] = serde_json::json!(false);
                                            report["complete"] = serde_json::json!(false);
                                            report["validation"]["target"] = serde_json::json!("failed");
                                            report["target_error"] = serde_json::json!({"code":error.code,"path":error.path,"detail":error.detail});
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
                write_json(args.files.output.as_deref(), &report, "write glTF preflight")?;
                if report["compatible"] != true || report["complete"] != true {
                    return Err("model preflight reported blockers or incomplete checks".into());
                }
                return Ok(());
            }
            let target = serde_json::from_value(serde_json::Value::String(args.target))?;
            let options = usd_toolbox_usd::scene::PreflightOptions {
                limits,
                target,
                max_issues: args.max_issues,
                check_geometry: args.geometry,
                ..Default::default()
            };
            let report = preflight_usd_file(&args.files.input, name, &bytes, &options)?;
            write_json(args.files.output.as_deref(), &report, "write preflight")?;
            if !report.complete || report.issues.iter().any(|issue| issue.severity == "error") {
                return Err("model preflight reported blockers or incomplete checks".into());
            }
        }
        ModelCommand::Capabilities { output } => {
            write_json(
                output.as_deref(),
                &serde_json::json!({"schema":"usd-toolbox.model-capabilities.v1", "version":env!("CARGO_PKG_VERSION"), "geometry_lods":{"schema":"usd-toolbox.geometry-lods.v1","formats":["glb","usdz"],"error_policy":"original_vertex_displacement"}, "core":scene_capabilities(), "usd":usd_toolbox_usd::scene::usd_scene_capabilities(), "gltf":usd_toolbox_gltf::scene::gltf_scene_capabilities()}),
                "write model capabilities",
            )?;
        }
        ModelCommand::Inspect(args) => {
            let input = load(&args.input)?;
            let mut report = inspect_scene(&input.scene, &limits)?;
            report.diagnostics.extend(input.diagnostics);
            write_json(args.output.as_deref(), &report, "write model inspection")?;
        }
        ModelCommand::Validate(args) => match load(&args.input) {
            Ok(input) => {
                let mut issues = validate_scene(&input.scene, &limits);
                issues.extend(input.diagnostics);
                let valid = !issues.iter().any(|i| i.severity == SceneSeverity::Error);
                write_json(
                    args.output.as_deref(),
                    &serde_json::json!({"schema":"usd-toolbox.scene-validation.v1","valid":valid,"diagnostics":issues}),
                    "write validation",
                )?;
                if !valid {
                    return Err("model validation failed".into());
                }
            }
            Err(error) => {
                let diagnostic = match error.downcast_ref::<SceneError>() {
                    Some(issue) => {
                        serde_json::json!({"code":issue.code,"severity":"error","path":issue.path,"detail":issue.detail})
                    }
                    None => {
                        serde_json::json!({"code":"model.input","severity":"error","path":args.input,"detail":error.to_string()})
                    }
                };
                write_json(
                    args.output.as_deref(),
                    &serde_json::json!({"schema":"usd-toolbox.scene-validation.v1","valid":false,"diagnostics":[diagnostic]}),
                    "write validation",
                )?;
                return Err(error);
            }
        },
        ModelCommand::Pack(args) => {
            let input = load(&args.input)?;
            write_model(input, &args, "usdz", Tier::K2, false)?;
        }
        ModelCommand::Export(args) => {
            let input = load(&args.files.input)?;
            let tier = args.tier.as_deref().map(parse_tier).transpose()?.unwrap_or(Tier::K2);
            if args.optimize_vertex_cache && !matches!(args.target.as_str(), "glb" | "gltf") {
                return Err("--optimize-vertex-cache applies to glTF exports".into());
            }
            write_model(input, &args.files, &args.target, tier, args.optimize_vertex_cache)?;
        }
        ModelCommand::Lod(args) => write_lods(&args, &limits)?,
        ModelCommand::Apply(args) => {
            let mut input = load(&args.files.input)?;
            let edits: Vec<SceneEdit> = read_json(&args.edits)?;
            input.scene = edit_scene(&input.scene, &edits, &limits)?;
            write_model(input, &args.files, "usdz", Tier::K2, false)?;
        }
        ModelCommand::Extract(args) => {
            let mut input = load(&args.files.input)?;
            input.scene = extract_subtree(&input.scene, &args.node, &limits)?;
            write_model(input, &args.files, "usdz", Tier::K2, false)?;
        }
        ModelCommand::Diff(args) => {
            let before = load(&args.before)?;
            let after = load(&args.after)?;
            write_json(
                args.output.as_deref(),
                &serde_json::json!({"schema":"usd-toolbox.scene-diff.v1","changes":diff_scenes(&before.scene,&after.scene)}),
                "write model diff",
            )?;
        }
    }
    Ok(())
}

fn preflight_usd_file(
    path: &Path,
    name: &str,
    bytes: &[u8],
    options: &usd_toolbox_usd::scene::PreflightOptions,
) -> ModelResult<usd_toolbox_usd::scene::PreflightReport> {
    use usd_toolbox_usd::scene::{PreflightIssue, preflight};
    // Authored inventory remains available even when projection fails. Only
    // explicitly referenced, confined local files are supplied to the scanner.
    let mut scan_options = options.clone();
    scan_options.check_geometry = false;
    let first = preflight(Input::Named { name, bytes }, &scan_options);
    let root = fs::canonicalize(
        path.parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new(".")),
    )?;
    let mut assets = BTreeMap::new();
    let mut errors = Vec::new();
    let mut total = bytes.len();
    for dependency in &first.dependencies {
        if dependency.status != "missing"
            || dependency.usage == "metadata"
            || assets.contains_key(&dependency.authored_path)
        {
            continue;
        }
        let candidate = match fs::canonicalize(root.join(&dependency.authored_path)) {
            Ok(candidate) => candidate,
            Err(_) => continue, // Preserve the scanner's missing-dependency issue.
        };
        let data = if !candidate.starts_with(&root) {
            Err("dependency escapes source directory".into())
        } else {
            read_bounded(&candidate, options.limits.max_bytes.saturating_sub(total))
        };
        match data {
            Ok(data) => {
                total += data.len();
                assets.insert(dependency.authored_path.clone(), data);
            }
            Err(error) => errors.push(PreflightIssue {
                code: "preflight.dependency_read".into(),
                path: dependency.source.clone(),
                severity: "error".into(),
                detail: error.to_string(),
                remedy: "Supply a readable dependency within the source directory and input budget.".into(),
            }),
        }
    }
    let mut report = if assets.is_empty() && !options.check_geometry {
        first
    } else {
        let mut files = vec![InputFile { name, bytes }];
        files.extend(assets.iter().map(|(name, bytes)| InputFile { name, bytes }));
        preflight(Input::Bundle(&files), options)
    };
    for error in errors {
        report.complete = false;
        if report.issues.len() < options.max_issues.max(1) {
            report.issues.push(error);
        }
    }
    Ok(report)
}

fn load(path: &Path) -> ModelResult<SceneImport> {
    let limits = SceneLimits::default();
    let bytes = read_bounded(path, limits.max_bytes)?;
    let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("model.usd");
    let gltf = bytes.starts_with(b"glTF")
        || path
            .extension()
            .and_then(|s| s.to_str())
            .is_some_and(|s| s.eq_ignore_ascii_case("gltf") || s.eq_ignore_ascii_case("glb"));
    if gltf {
        let dependencies = usd_toolbox_gltf::scene::discover_dependencies(&bytes, name, &limits)?;
        let options = usd_toolbox_gltf::scene::SceneGltfImportOptions {
            limits,
            ..Default::default()
        };
        if dependencies.is_empty() {
            return Ok(usd_toolbox_gltf::scene::import_scene(&bytes, name, &options)?);
        }
        let root = fs::canonicalize(
            path.parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(Path::new(".")),
        )?;
        let mut total = bytes.len();
        let mut files = BTreeMap::from([(name.to_owned(), bytes)]);
        for dependency in dependencies {
            let candidate = fs::canonicalize(root.join(&dependency))?;
            if !candidate.starts_with(&root) {
                return Err(format!("dependency escapes source directory: {dependency}").into());
            }
            let bytes = read_bounded(&candidate, options.limits.max_bytes.saturating_sub(total))?;
            total += bytes.len();
            files.insert(dependency, bytes);
        }
        return Ok(usd_toolbox_gltf::scene::import_scene_bundle(name, &files, &options)?);
    }
    if bytes.iter().copied().find(|c| !c.is_ascii_whitespace()) == Some(b'{') {
        let scene: Scene = serde_json::from_slice(&bytes)?;
        require_valid_scene(&scene, &limits)?;
        return Ok(SceneImport {
            scene,
            diagnostics: Vec::new(),
        });
    }
    let dependencies = asset_dependencies(&bytes, &limits)?;
    if dependencies.is_empty() {
        return Ok(import_scene(
            Input::Named {
                name: path.file_name().and_then(|s| s.to_str()).unwrap_or("model.usd"),
                bytes: &bytes,
            },
            &SceneImportOptions { limits },
        )?);
    }
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let root = fs::canonicalize(parent)?;
    let mut owned = vec![(
        path.file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("model.usd")
            .to_owned(),
        bytes,
    )];
    let mut total = owned[0].1.len();
    for name in dependencies {
        let candidate = fs::canonicalize(root.join(&name))?;
        if !candidate.starts_with(&root) {
            return Err(format!("dependency escapes source directory: {name}").into());
        }
        let bytes = read_bounded(&candidate, limits.max_bytes.saturating_sub(total))?;
        total += bytes.len();
        owned.push((name, bytes));
    }
    let files: Vec<_> = owned.iter().map(|(name, bytes)| InputFile { name, bytes }).collect();
    Ok(import_scene(Input::Bundle(&files), &SceneImportOptions { limits })?)
}

fn read_bounded(path: &Path, maximum: usize) -> ModelResult<Vec<u8>> {
    use std::io::Read;
    let file = fs::File::open(path)?;
    if file.metadata()?.len() > maximum as u64 {
        return Err(format!("input exceeds byte limit: {}", path.display()).into());
    }
    let mut bytes = Vec::new();
    file.take(maximum as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > maximum {
        return Err("input grew beyond byte limit".into());
    }
    Ok(bytes)
}

fn write_model(
    mut input: SceneImport,
    args: &WriteArgs,
    target: &str,
    tier: Tier,
    optimize_vertex_cache: bool,
) -> ModelResult<()> {
    if let Some(id) = &args.id {
        input.scene.id = id.clone();
    }
    if let Some(name) = &args.name {
        input.scene.name = name.clone();
    }
    if let Some(path) = &args.material_bindings {
        input.scene = pin_materials(&input.scene, path)?;
    }
    let source = fs::canonicalize(&args.input)?;
    if args.output.exists() && fs::canonicalize(&args.output)? == source {
        return Err("model commands require a new output path; source files are immutable".into());
    }
    if let Some(report) = &args.report
        && (report == &args.output
            || (report.exists()
                && (fs::canonicalize(report)? == source
                    || (args.output.exists() && fs::canonicalize(report)? == fs::canonicalize(&args.output)?))))
    {
        return Err("report must have a separate path from input and output".into());
    }
    let exported = match target {
        "usdz" | "usda" | "usdc" => usd_toolbox_usd::scene::export_scene(
            &input.scene,
            &SceneExportOptions {
                format: match target {
                    "usda" => UsdFormat::Usda,
                    "usdc" => UsdFormat::Usdc,
                    _ => UsdFormat::Usdz,
                },
                texture_tier: tier,
                ..Default::default()
            },
        )?,
        "glb" | "gltf" => usd_toolbox_gltf::scene::export_scene(
            &input.scene,
            &SceneGltfOptions {
                optimize_vertex_cache,
                format: if target == "glb" {
                    GltfFormat::Glb
                } else {
                    GltfFormat::Gltf
                },
                texture_tier: tier,
                ..Default::default()
            },
        )?,
        "scene-json" => SceneExport {
            bytes: serde_json::to_vec(&input.scene)?,
            diagnostics: Vec::new(),
        },
        _ => return Err(format!("unsupported model target {target}").into()),
    };
    let mut diagnostics = input.diagnostics;
    diagnostics.extend(exported.diagnostics);
    // Describe the canonical bytes after USD float32 projection, so downstream
    // previews compare against the package rather than unrounded source points.
    let inspection = if matches!(target, "usdz" | "usda" | "usdc") {
        let canonical = import_scene(Input::Bytes(&exported.bytes), &SceneImportOptions::default())?;
        inspect_scene(&canonical.scene, &SceneLimits::default())?
    } else {
        inspect_scene(&input.scene, &SceneLimits::default())?
    };
    let mut assets_report = Vec::new();
    if matches!(target, "usda" | "usdc") {
        let assets = usd_toolbox_usd::scene::export_scene_assets(&input.scene, &SceneLimits::default())?;
        let parent = args
            .output
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        fs::create_dir_all(parent)?;
        let root = fs::canonicalize(parent)?;
        for (name, bytes) in &assets {
            let destination = root.join(name);
            let mut directory = root.clone();
            for component in Path::new(name).parent().into_iter().flat_map(Path::components) {
                if !matches!(component, Component::Normal(_)) {
                    return Err("invalid sidecar path".into());
                }
                directory.push(component);
                if directory.is_symlink() {
                    return Err("sidecar directories must not be symlinks".into());
                }
                fs::create_dir_all(&directory)?;
            }
            if destination.is_symlink() {
                return Err("sidecar must not be a symlink".into());
            }
            if destination.exists() {
                if read_bounded(&destination, SceneLimits::default().max_bytes)? != *bytes {
                    return Err(format!("sidecar would overwrite different content: {name}").into());
                }
            } else {
                atomic_write(&destination, bytes, "write model dependency")?;
            }
            assets_report.push(serde_json::json!({"path":name,"sha256":format!("{:x}",Sha256Hasher::digest(bytes)),"bytes":bytes.len()}));
        }
    }
    let report = serde_json::json!({"schema":"usd-toolbox.model-build.v1","version":env!("CARGO_PKG_VERSION"),"target":target,"inspection":inspection,
        "diagnostics":diagnostics,"assets":assets_report,"output":{"sha256":format!("{:x}",Sha256Hasher::digest(&exported.bytes)),"bytes":exported.bytes.len()}});
    atomic_write(&args.output, &exported.bytes, "write model")?;
    write_json(args.report.as_deref(), &report, "write model report")?;
    Ok(())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MaterialPin {
    source_material: String,
    package: PathBuf,
    sha256: String,
    material_id: Option<String>,
}

fn pin_materials(scene: &Scene, path: &Path) -> ModelResult<Scene> {
    let pins: Vec<MaterialPin> = read_json(path)?;
    let root = fs::canonicalize(
        path.parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new(".")),
    )?;
    let mut result = scene.clone();
    let mut seen = BTreeSet::new();
    for pin in pins {
        if !seen.insert(pin.source_material.clone()) {
            return Err("duplicate source material pin".into());
        }
        if pin.package.is_absolute()
            || pin
                .package
                .components()
                .any(|c| matches!(c, Component::ParentDir | Component::Prefix(_)))
        {
            return Err("material package path must stay within manifest directory".into());
        }
        let package = fs::canonicalize(root.join(&pin.package))?;
        if !package.starts_with(&root) {
            return Err("material package symlink escapes manifest directory".into());
        }
        let bytes = read_bounded(&package, SceneLimits::default().max_bytes)?;
        if format!("{:x}", Sha256Hasher::digest(&bytes)) != pin.sha256 {
            return Err("material package SHA-256 mismatch".into());
        }
        let materials = UsdImporter.import(Input::Bytes(&bytes), &UsdImportOptions::default())?;
        let mut material = if let Some(id) = pin.material_id {
            materials
                .into_iter()
                .find(|m| m.id.0 == id)
                .ok_or("material identity missing from package")?
        } else {
            if materials.len() != 1 {
                return Err("multi-material package requires material_id".into());
            }
            materials.into_iter().next().unwrap()
        };
        material
            .provenance
            .metadata
            .insert("opal_package_sha256".into(), serde_json::json!(pin.sha256));
        result = edit_scene(
            &result,
            &[SceneEdit::RebindMaterial {
                source: usd_toolbox_core::MaterialId(pin.source_material),
                material: Box::new(material),
            }],
            &SceneLimits::default(),
        )?;
    }
    Ok(result)
}

fn write_lods(args: &LodArgs, limits: &SceneLimits) -> ModelResult<()> {
    // symlink_metadata also rejects dangling symlinks, unlike Path::exists.
    if args.output_dir.symlink_metadata().is_ok() {
        return Err("LOD output directory must be a new path".into());
    }
    let mut options: SceneLodOptions = serde_json::from_slice(&read_bounded(&args.options, 64 * 1024)?)?;
    options.max_levels = options.max_levels.min(8);
    options.max_output_bytes = options.max_output_bytes.min(limits.max_bytes);
    options.max_output_faces = options.max_output_faces.min(limits.max_faces);
    options.max_source_corners = options.max_source_corners.min(limits.max_corners);
    let input = load(&args.input)?;
    let generated = generate_scene_lods(&input.scene, &options, limits)?.export_with(|scene| {
        if args.target == "glb" {
            usd_toolbox_gltf::scene::export_scene(
                scene,
                &SceneGltfOptions {
                    limits: limits.clone(),
                    ..Default::default()
                },
            )
        } else {
            usd_toolbox_usd::scene::export_scene(
                scene,
                &SceneExportOptions {
                    limits: limits.clone(),
                    ..Default::default()
                },
            )
        }
    })?;
    let files = generated
        .levels
        .iter()
        .enumerate()
        .map(|(i, item)| {
            serde_json::json!({
                "name":format!("lod-{i}.{}",args.target),"level":item.level.name,"bytes":item.export.bytes.len(),
                "sha256":format!("{:x}",Sha256Hasher::digest(&item.export.bytes)),"diagnostics":item.export.diagnostics,
            })
        })
        .collect::<Vec<_>>();
    let mut manifest = serde_json::to_value(&generated.manifest)?;
    manifest["target"] = serde_json::json!(args.target);
    manifest["files"] = serde_json::json!(files);
    manifest["import_diagnostics"] = serde_json::json!(input.diagnostics);
    let manifest_bytes = serde_json::to_vec_pretty(&manifest)?;
    let total = generated
        .levels
        .iter()
        .try_fold(manifest_bytes.len(), |size, item| {
            size.checked_add(item.export.bytes.len())
        })
        .ok_or("LOD output byte count overflow")?;
    if total > options.max_output_bytes {
        return Err("LOD files and manifest exceed max_output_bytes".into());
    }
    let parent = args
        .output_dir
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent)?;
    let staging = tempfile::Builder::new()
        .prefix(".usd-toolbox-lods-")
        .tempdir_in(parent)?;
    for (i, item) in generated.levels.into_iter().enumerate() {
        let mut file = fs::File::create(staging.path().join(format!("lod-{i}.{}", args.target)))?;
        file.write_all(&item.export.bytes)?;
        file.sync_all()?;
    }
    let mut file = fs::File::create(staging.path().join("manifest.json"))?;
    file.write_all(&manifest_bytes)?;
    file.sync_all()?;
    drop(file);
    // Reserve only a new path. On Unix the complete directory replaces this
    // owned empty reservation atomically; no existing user directory is replaced.
    fs::create_dir(&args.output_dir)?;
    #[cfg(windows)]
    fs::remove_dir(&args.output_dir)?; // Windows rename refuses any existing destination.
    if let Err(error) = fs::rename(staging.path(), &args.output_dir) {
        let _ = fs::remove_dir(&args.output_dir); // Only removes an empty reservation.
        return Err(error.into());
    }
    Ok(())
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
