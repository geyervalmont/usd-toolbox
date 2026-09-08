//! Filesystem boundary for server workers, scripts, and CI.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{self, Cursor, Write};
use std::path::{Component, Path, PathBuf};
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256 as Sha256Hasher};
use tempfile::NamedTempFile;
use thiserror::Error;
use time::OffsetDateTime;
use usd_toolbox_core::{
    AuxiliaryAsset, AuxiliaryRole, Channel, ColorSpace, Export, ExportError, Exporter, ImportError, Importer, Input,
    Loss, LossKind, MapRole, Material, MaterialId, NormalConvention, Operation, Repeat, Sha256, ShadingModel, Tier,
    Tiling,
};
use usd_toolbox_gltf::{GltfExportOptions, GltfExporter, GltfFormat};
use usd_toolbox_materials::{
    AttachmentProvenance, AuditProfile, EmbeddingOptions, EmbeddingRecord, MaterialWorkflowError, TextureAttachment,
    TierPolicy, apply_merge_patch, attach_texture, audit_library, cluster_embeddings, diff_materials, extract_texture,
    generate_missing_downscales, inspect, migrate_document_json, plan_tiers, prepare_embedding_input, remove_texture,
};
use usd_toolbox_materialx::{MaterialXExportOptions, MaterialXExporter, MaterialXImportOptions, MaterialXImporter};
use usd_toolbox_omniverse::{OmniverseExportOptions, OmniverseExporter};
use usd_toolbox_revit::{RevitExportOptions, RevitExporter};
use usd_toolbox_textures::{TextureImportOptions, TextureInput, TextureMetadata, TextureSetImporter, parse_role};
use usd_toolbox_usd::{UsdExportOptions, UsdExporter, UsdFormat, UsdImportOptions, UsdImporter};

#[derive(Debug, Parser)]
#[command(name = "usd-toolbox", version, about = "Build and convert portable USD assets")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Build a self-contained USDZ from a staged texture manifest.
    Build(BuildArgs),
    /// Inspect neutral materials without dumping texture byte arrays.
    Inspect(InputOutputArgs),
    /// Audit package contents against a configurable policy.
    Audit(AuditArgs),
    /// Audit and return a failing exit status when any material is invalid.
    Validate(AuditArgs),
    /// Compute a semantic diff between two material packages.
    Diff(DiffArgs),
    /// Apply a validated JSON merge patch and write a new USDZ revision.
    Apply(ApplyArgs),
    /// Extract one exact role/tier image.
    ExtractTexture(ExtractTextureArgs),
    /// Attach an AI-generated or manually authored image as a new revision.
    AttachTexture(Box<AttachTextureArgs>),
    /// Remove one role/tier image and write a new revision.
    RemoveTexture(RemoveTextureArgs),
    /// Describe missing texture-tier work without invoking a generator.
    PlanTiers(PolicyArgs),
    /// Generate safe missing downscales and report work that remains external.
    CompleteTiers(CompleteTiersArgs),
    /// Export to USD, MaterialX, glTF, Revit, or Omniverse.
    Export(ExportArgs),
    /// Produce stable visual, text, and structured embedding inputs.
    EmbeddingInput(EmbeddingInputArgs),
    /// Cluster caller-produced embedding records by cosine neighbourhood.
    Cluster(ClusterArgs),
    /// Upgrade legacy neutral JSON to the current versioned schema.
    Migrate(MigrateArgs),
}

#[derive(Debug, Args)]
struct InputOutputArgs {
    #[arg(long)]
    input: PathBuf,
    /// JSON output; stdout when omitted.
    #[arg(long)]
    output: Option<PathBuf>,
}

#[derive(Debug, Args)]
struct AuditArgs {
    #[arg(long)]
    input: PathBuf,
    /// Optional JSON AuditProfile; defaults to portable PBR.
    #[arg(long)]
    profile: Option<PathBuf>,
    #[arg(long)]
    report: PathBuf,
}

#[derive(Debug, Args)]
struct DiffArgs {
    #[arg(long)]
    before: PathBuf,
    #[arg(long)]
    after: PathBuf,
    #[arg(long)]
    output: Option<PathBuf>,
}

#[derive(Debug, Args)]
struct ApplyArgs {
    #[arg(long)]
    input: PathBuf,
    #[arg(long)]
    patch: PathBuf,
    /// Required when the package contains more than one material.
    #[arg(long)]
    material_id: Option<String>,
    #[arg(long)]
    output: PathBuf,
}

#[derive(Debug, Args)]
struct ExtractTextureArgs {
    #[arg(long)]
    input: PathBuf,
    #[arg(long)]
    material_id: Option<String>,
    #[arg(long)]
    role: String,
    #[arg(long)]
    tier: String,
    #[arg(long)]
    output: PathBuf,
}

#[derive(Debug, Args)]
struct AttachTextureArgs {
    #[arg(long)]
    input: PathBuf,
    #[arg(long)]
    material_id: Option<String>,
    #[arg(long)]
    role: String,
    #[arg(long)]
    tier: String,
    #[arg(long)]
    texture: PathBuf,
    #[arg(long)]
    color_space: String,
    #[arg(long)]
    channel: Option<String>,
    #[arg(long)]
    normal_convention: Option<String>,
    #[arg(long)]
    parent_hash: Option<String>,
    #[arg(long)]
    operation: String,
    #[arg(long)]
    tool: String,
    #[arg(long)]
    tool_version: String,
    #[arg(long)]
    model: Option<String>,
    /// Optional JSON object with provider/tool parameters.
    #[arg(long)]
    parameters: Option<PathBuf>,
    /// RFC3339 timestamp supplied by the ingest caller.
    #[arg(long)]
    created: String,
    #[arg(long)]
    output: PathBuf,
}

#[derive(Debug, Args)]
struct RemoveTextureArgs {
    #[arg(long)]
    input: PathBuf,
    #[arg(long)]
    material_id: Option<String>,
    #[arg(long)]
    role: String,
    #[arg(long)]
    tier: String,
    #[arg(long)]
    output: PathBuf,
}

#[derive(Debug, Args)]
struct PolicyArgs {
    #[arg(long)]
    input: PathBuf,
    #[arg(long)]
    policy: Option<PathBuf>,
    #[arg(long)]
    report: PathBuf,
}

#[derive(Debug, Args)]
struct CompleteTiersArgs {
    #[arg(long)]
    input: PathBuf,
    #[arg(long)]
    material_id: Option<String>,
    #[arg(long)]
    policy: Option<PathBuf>,
    #[arg(long)]
    created: String,
    #[arg(long)]
    output: PathBuf,
    #[arg(long)]
    report: PathBuf,
}

#[derive(Debug, Args)]
struct ExportArgs {
    #[arg(long)]
    input: PathBuf,
    /// usd, usda, usdc, usdz, materialx, gltf, glb, revit, or omniverse.
    #[arg(long)]
    target: String,
    #[arg(long)]
    tier: Option<String>,
    #[arg(long)]
    output: PathBuf,
    #[arg(long)]
    report: Option<PathBuf>,
}

#[derive(Debug, Args)]
struct EmbeddingInputArgs {
    #[arg(long)]
    input: PathBuf,
    #[arg(long)]
    material_id: Option<String>,
    #[arg(long)]
    tier: Option<String>,
    #[arg(long)]
    output: PathBuf,
}

#[derive(Debug, Args)]
struct ClusterArgs {
    /// JSON array of model-versioned EmbeddingRecord objects.
    #[arg(long)]
    input: PathBuf,
    #[arg(long, default_value_t = 0.85)]
    similarity_threshold: f32,
    #[arg(long, default_value_t = 3)]
    min_cluster_size: usize,
    #[arg(long)]
    output: PathBuf,
}

#[derive(Debug, Args)]
struct MigrateArgs {
    #[arg(long)]
    input: PathBuf,
    #[arg(long)]
    output: PathBuf,
}

#[derive(Debug, Args)]
struct BuildArgs {
    /// JSON build manifest. Texture paths are relative to this file.
    #[arg(long)]
    manifest: PathBuf,
    /// Destination USDZ package.
    #[arg(long)]
    output: PathBuf,
    /// Destination JSON loss and build report.
    #[arg(long)]
    report: PathBuf,
}

#[derive(Debug, Error)]
enum BuildError {
    #[error("cannot {action} `{path}`: {source}")]
    Io {
        action: &'static str,
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("invalid manifest `{path}`: {source}")]
    Manifest {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("invalid build manifest: {0}")]
    InvalidManifest(String),
    #[error("texture import failed: {0}")]
    Import(#[from] ImportError),
    #[error("USDZ export failed: {0}")]
    Export(#[from] ExportError),
    #[error("material workflow failed: {0}")]
    Workflow(#[from] MaterialWorkflowError),
    #[error("unsupported input or option: {0}")]
    Unsupported(String),
    #[error("validation failed for {0} material(s); see the report for diagnostics")]
    ValidationFailed(usize),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct BuildManifest {
    schema: u32,
    variant: ManifestVariant,
    #[serde(default)]
    shading_model: Option<String>,
    required_tiers: Vec<Tier>,
    #[serde(default)]
    tiling: Value,
    channels: BTreeMap<String, BTreeMap<String, ChannelSource>>,
    #[serde(default)]
    provenance: Value,
    #[serde(with = "time::serde::rfc3339")]
    ingested_at: OffsetDateTime,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestVariant {
    code: String,
    name: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ChannelSource {
    sha256: String,
    object_key: String,
    #[serde(rename = "bytes")]
    expected_bytes: u64,
    colour_space: Option<ColorSpace>,
    width_px: Option<u32>,
    height_px: Option<u32>,
    normal_convention: Option<ManifestNormalConvention>,
    path: PathBuf,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
enum ManifestNormalConvention {
    #[serde(alias = "open_gl", alias = "gl")]
    Opengl,
    #[serde(alias = "direct_x", alias = "dx")]
    Directx,
}

impl From<ManifestNormalConvention> for NormalConvention {
    fn from(value: ManifestNormalConvention) -> Self {
        match value {
            ManifestNormalConvention::Opengl => Self::OpenGl,
            ManifestNormalConvention::Directx => Self::DirectX,
        }
    }
}

#[derive(Debug, Serialize)]
struct BuildReport {
    schema: u32,
    version: &'static str,
    tiers: Vec<String>,
    losses: Vec<Loss>,
    output: OutputReport,
}

#[derive(Debug, Serialize)]
struct OutputReport {
    sha256: String,
    bytes: usize,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Build(args) => build(&args),
        Command::Inspect(args) => inspect_command(&args),
        Command::Audit(args) => audit_command(&args),
        Command::Validate(args) => validate_command(&args),
        Command::Diff(args) => diff_command(&args),
        Command::Apply(args) => apply_command(&args),
        Command::ExtractTexture(args) => extract_texture_command(&args),
        Command::AttachTexture(args) => attach_texture_command(&args),
        Command::RemoveTexture(args) => remove_texture_command(&args),
        Command::PlanTiers(args) => plan_tiers_command(&args),
        Command::CompleteTiers(args) => complete_tiers_command(&args),
        Command::Export(args) => export_command(&args),
        Command::EmbeddingInput(args) => embedding_input_command(&args),
        Command::Cluster(args) => cluster_command(&args),
        Command::Migrate(args) => migrate_command(&args),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("usd-toolbox: {error}");
            ExitCode::FAILURE
        }
    }
}

fn build(args: &BuildArgs) -> Result<(), BuildError> {
    let manifest_bytes = read(&args.manifest, "read manifest")?;
    let manifest: BuildManifest = serde_json::from_slice(&manifest_bytes).map_err(|source| BuildError::Manifest {
        path: args.manifest.clone(),
        source,
    })?;
    if manifest.schema != 1 {
        return Err(BuildError::InvalidManifest(format!(
            "unsupported schema {}; expected 1",
            manifest.schema
        )));
    }
    if manifest.variant.code.trim().is_empty() || manifest.variant.name.trim().is_empty() {
        return Err(BuildError::InvalidManifest(
            "variant.code and variant.name must not be empty".into(),
        ));
    }
    if manifest.channels.is_empty() {
        return Err(BuildError::InvalidManifest("channels must not be empty".into()));
    }

    let manifest_dir = args.manifest.parent().unwrap_or_else(|| Path::new("."));
    let source_root = canonicalize(manifest_dir, "resolve manifest directory")?;
    let loaded = load_inputs(&manifest, &source_root)?;
    let options = TextureImportOptions {
        material_id: manifest.variant.code.clone(),
        material_name: manifest.variant.name.clone(),
        required_tiers: manifest.required_tiers.clone(),
        normal_target: Some(NormalConvention::OpenGl),
        infer_normal_convention_from_filename: false,
        created: manifest.ingested_at,
        files: BTreeMap::new(),
    };
    let (mut material, mut losses) = if loaded.textures.is_empty() {
        let mut material = Material::new(&options.material_id, &options.material_name);
        material.provenance.builder = Some("usd-toolbox".into());
        material.provenance.builder_version = Some(env!("CARGO_PKG_VERSION").into());
        material.provenance.built_at = Some(options.created);
        (material, Vec::new())
    } else {
        let imported = TextureSetImporter.import_set(&loaded.textures, &options)?;
        (imported.material, imported.losses)
    };
    material.auxiliary = loaded.auxiliary;
    material.model = parse_shading_model(manifest.shading_model.as_deref())?;
    let (tiling, tiling_loss) = parse_tiling(&manifest.tiling, &material.id)?;
    material.tiling = tiling;
    material.provenance.metadata = provenance_map(&manifest.provenance)?;
    material.provenance.source_identifier = metadata_string(&material.provenance.metadata, "source_identifier");
    material.provenance.supplier_source = metadata_string(&material.provenance.metadata, "supplier_source");
    material.provenance.version = metadata_string(&material.provenance.metadata, "version");

    let tiers = material_tiers(&material);
    let exported = UsdExporter.export(std::slice::from_ref(&material), &UsdExportOptions::default())?;
    if let Some(loss) = tiling_loss {
        losses.push(loss);
    }
    losses.extend(exported.losses);
    losses.sort_by(|left, right| {
        left.material
            .cmp(&right.material)
            .then_with(|| left.parameter.cmp(right.parameter))
            .then_with(|| left.detail.cmp(&right.detail))
    });
    losses.dedup();

    let report = BuildReport {
        schema: 1,
        version: env!("CARGO_PKG_VERSION"),
        tiers,
        losses,
        output: OutputReport {
            sha256: format!("{:x}", Sha256Hasher::digest(&exported.bytes)),
            bytes: exported.bytes.len(),
        },
    };
    let mut report_bytes = serde_json::to_vec_pretty(&report).map_err(|source| BuildError::Manifest {
        path: args.report.clone(),
        source,
    })?;
    report_bytes.push(b'\n');

    atomic_write(&args.output, &exported.bytes, "write output")?;
    atomic_write(&args.report, &report_bytes, "write report")?;
    Ok(())
}

struct LoadedInputs {
    textures: Vec<TextureInput>,
    auxiliary: Vec<AuxiliaryAsset>,
}

#[derive(Clone, Copy)]
enum InputRole {
    Texture(MapRole),
    Auxiliary(AuxiliaryRole),
}

fn load_inputs(manifest: &BuildManifest, root: &Path) -> Result<LoadedInputs, BuildError> {
    let mut textures = Vec::new();
    let mut auxiliary = Vec::new();
    for (role_name, tiers) in &manifest.channels {
        let role = match parse_role(role_name) {
            Ok(role) => InputRole::Texture(role),
            Err(error) => parse_auxiliary_role(role_name)
                .map_or_else(|| Err(BuildError::Import(error)), |role| Ok(InputRole::Auxiliary(role)))?,
        };
        if tiers.is_empty() {
            return Err(BuildError::InvalidManifest(format!(
                "channel `{role_name}` must contain at least one tier"
            )));
        }
        for (tier_name, source) in tiers {
            let tier = parse_tier(tier_name)?;
            let path = resolve_source(root, &source.path)?;
            let bytes = read(&path, "read texture")?;
            validate_source(source, &path, &bytes)?;
            if let InputRole::Texture(role) = role {
                let colour_space = source.colour_space.ok_or_else(|| {
                    BuildError::InvalidManifest(format!(
                        "channels.{role_name}.{tier_name}.colour_space must be explicit"
                    ))
                })?;
                let normal_convention = source.normal_convention.map(Into::into);
                if role == MapRole::Normal && normal_convention.is_none() {
                    return Err(BuildError::InvalidManifest(format!(
                        "channels.{role_name}.{tier_name}.normal_convention must be explicit"
                    )));
                }
                textures.push(TextureInput {
                    name: source_name(source),
                    bytes,
                    metadata: TextureMetadata {
                        role: Some(role),
                        tier: Some(tier),
                        color_space: Some(colour_space),
                        channel: None,
                        normal_convention,
                    },
                });
            } else if let InputRole::Auxiliary(role) = role {
                auxiliary.push(AuxiliaryAsset {
                    role,
                    name: source_name(source),
                    hash: Sha256(source.sha256.clone()),
                    media_type: media_type(&source.path).into(),
                    bytes,
                });
            }
        }
    }
    Ok(LoadedInputs { textures, auxiliary })
}

fn validate_source(source: &ChannelSource, path: &Path, bytes: &[u8]) -> Result<(), BuildError> {
    let actual_bytes = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
    if actual_bytes != source.expected_bytes {
        return Err(BuildError::InvalidManifest(format!(
            "texture `{}` has {actual_bytes} bytes; manifest declares {}",
            path.display(),
            source.expected_bytes
        )));
    }
    let actual_sha256 = format!("{:x}", Sha256Hasher::digest(bytes));
    if actual_sha256 != source.sha256 {
        return Err(BuildError::InvalidManifest(format!(
            "texture `{}` has SHA-256 {actual_sha256}; manifest declares {}",
            path.display(),
            source.sha256
        )));
    }
    if source.width_px.is_some() || source.height_px.is_some() {
        let format = image::guess_format(bytes).map_err(|error| {
            BuildError::InvalidManifest(format!(
                "texture `{}` has no recognised image format: {error}",
                path.display()
            ))
        })?;
        let (width, height) = image::ImageReader::with_format(Cursor::new(bytes), format)
            .into_dimensions()
            .map_err(|error| {
                BuildError::InvalidManifest(format!(
                    "cannot read dimensions for texture `{}`: {error}",
                    path.display()
                ))
            })?;
        if source.width_px.is_some_and(|expected| expected != width)
            || source.height_px.is_some_and(|expected| expected != height)
        {
            return Err(BuildError::InvalidManifest(format!(
                "texture `{}` is {width}x{height}; manifest declares {}x{}",
                path.display(),
                display_dimension(source.width_px),
                display_dimension(source.height_px)
            )));
        }
    }
    Ok(())
}

fn resolve_source(root: &Path, relative: &Path) -> Result<PathBuf, BuildError> {
    if relative.as_os_str().is_empty()
        || relative.is_absolute()
        || relative.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(BuildError::InvalidManifest(format!(
            "texture path `{}` must stay relative to the manifest directory",
            relative.display()
        )));
    }
    let resolved = canonicalize(&root.join(relative), "resolve texture")?;
    if !resolved.starts_with(root) {
        return Err(BuildError::InvalidManifest(format!(
            "texture path `{}` resolves outside the manifest directory",
            relative.display()
        )));
    }
    Ok(resolved)
}

fn parse_tier(value: &str) -> Result<Tier, BuildError> {
    match value.to_ascii_lowercase().as_str() {
        "preview" => Ok(Tier::Preview),
        "1k" => Ok(Tier::K1),
        "2k" => Ok(Tier::K2),
        "4k" => Ok(Tier::K4),
        "8k" => Ok(Tier::K8),
        _ => Err(BuildError::InvalidManifest(format!(
            "unknown resolution tier `{value}`"
        ))),
    }
}

fn parse_auxiliary_role(value: &str) -> Option<AuxiliaryRole> {
    match value.to_ascii_lowercase().as_str() {
        "ref_image" => Some(AuxiliaryRole::RefImage),
        "render" => Some(AuxiliaryRole::Render),
        "thumbnail" => Some(AuxiliaryRole::Thumbnail),
        "mdl" => Some(AuxiliaryRole::Mdl),
        _ => None,
    }
}

fn source_name(source: &ChannelSource) -> String {
    if source.object_key.is_empty() {
        source.path.to_string_lossy().into_owned()
    } else {
        source.object_key.clone()
    }
}

fn media_type(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
    {
        Some(extension) if extension == "png" => "image/png",
        Some(extension) if matches!(extension.as_str(), "jpg" | "jpeg" | "jpe") => "image/jpeg",
        Some(extension) if extension == "exr" => "image/x-exr",
        Some(extension) if extension == "mdl" => "application/vnd.nvidia.mdl",
        _ => "application/octet-stream",
    }
}

fn parse_shading_model(value: Option<&str>) -> Result<ShadingModel, BuildError> {
    match value
        .unwrap_or("openpbr")
        .to_ascii_lowercase()
        .replace('-', "_")
        .as_str()
    {
        "openpbr" | "open_pbr" => Ok(ShadingModel::OpenPbr),
        "standard_surface" => Ok(ShadingModel::StandardSurface),
        "gltf_pbr" | "gltfpbr" => Ok(ShadingModel::GltfPbr),
        value => Err(BuildError::InvalidManifest(format!("unknown shading_model `{value}`"))),
    }
}

fn parse_tiling(value: &Value, material: &MaterialId) -> Result<(Option<Tiling>, Option<Loss>), BuildError> {
    let object = match value {
        Value::Null => return Ok((None, None)),
        Value::Array(values) if values.is_empty() => return Ok((None, None)),
        Value::Object(object) if object.is_empty() => return Ok((None, None)),
        Value::Object(object) => object,
        _ => {
            return Err(BuildError::InvalidManifest(
                "tiling must be an object (or an empty array)".into(),
            ));
        }
    };
    for key in object.keys() {
        if !["width_mm", "height_mm", "repeat", "install_pattern"].contains(&key.as_str()) {
            return Err(BuildError::InvalidManifest(format!("unknown tiling field `{key}`")));
        }
    }
    let width_mm = optional_number(object, "width_mm")?;
    let height_mm = optional_number(object, "height_mm")?;
    let install_pattern = optional_string(object, "install_pattern")?;
    let repeat_name = optional_string(object, "repeat")?;
    let (repeat, approximation) = match repeat_name.as_deref().map(str::to_ascii_lowercase).as_deref() {
        None | Some("straight") => (Repeat::Straight, None),
        Some("halfdrop" | "half_drop") => (Repeat::Halfdrop, None),
        Some("brick") => (Repeat::Brick, None),
        Some("random") => (Repeat::Random, None),
        Some("none") => (Repeat::None, None),
        Some("tile" | "pattern" | "surface_crop") => (
            Repeat::Straight,
            Some(format!(
                "application repeat `{}` was represented as straight texture repetition",
                repeat_name.as_deref().unwrap_or_default()
            )),
        ),
        Some(value) => return Err(BuildError::InvalidManifest(format!("unknown tiling.repeat `{value}`"))),
    };
    let loss = approximation.map(|detail| Loss {
        material: material.clone(),
        parameter: "tiling.repeat",
        kind: LossKind::Approximated {
            as_parameter: "tiling.repeat",
        },
        detail,
    });
    Ok((
        Some(Tiling {
            width_mm,
            height_mm,
            repeat,
            install_pattern,
        }),
        loss,
    ))
}

fn optional_number(object: &Map<String, Value>, key: &str) -> Result<Option<f64>, BuildError> {
    let Some(value) = object.get(key) else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    let number = match value {
        Value::Number(value) => value.as_f64(),
        Value::String(value) => value.parse().ok(),
        _ => None,
    }
    .filter(|value| value.is_finite() && *value > 0.0)
    .ok_or_else(|| BuildError::InvalidManifest(format!("tiling.{key} must be a positive number")))?;
    Ok(Some(number))
}

fn optional_string(object: &Map<String, Value>, key: &str) -> Result<Option<String>, BuildError> {
    match object.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) if !value.trim().is_empty() => Ok(Some(value.clone())),
        Some(Value::String(_)) => Ok(None),
        Some(_) => Err(BuildError::InvalidManifest(format!("tiling.{key} must be a string"))),
    }
}

fn provenance_map(value: &Value) -> Result<BTreeMap<String, Value>, BuildError> {
    match value {
        Value::Null => Ok(BTreeMap::new()),
        Value::Array(values) if values.is_empty() => Ok(BTreeMap::new()),
        Value::Object(values) => Ok(values.iter().map(|(key, value)| (key.clone(), value.clone())).collect()),
        _ => Err(BuildError::InvalidManifest(
            "provenance must be an object (or an empty array)".into(),
        )),
    }
}

fn metadata_string(metadata: &BTreeMap<String, Value>, key: &str) -> Option<String> {
    metadata.get(key).and_then(Value::as_str).map(str::to_owned)
}

fn material_tiers(material: &usd_toolbox_core::Material) -> Vec<String> {
    let mut tiers = BTreeSet::new();
    material.visit_textures(|_, texture| tiers.extend(texture.tiers.keys().copied()));
    tiers.into_iter().map(|tier| tier.as_str().to_owned()).collect()
}

fn display_dimension(value: Option<u32>) -> String {
    value.map_or_else(|| "?".into(), |value| value.to_string())
}

fn read(path: &Path, action: &'static str) -> Result<Vec<u8>, BuildError> {
    fs::read(path).map_err(|source| BuildError::Io {
        action,
        path: path.to_owned(),
        source,
    })
}

fn canonicalize(path: &Path, action: &'static str) -> Result<PathBuf, BuildError> {
    fs::canonicalize(path).map_err(|source| BuildError::Io {
        action,
        path: path.to_owned(),
        source,
    })
}

fn atomic_write(path: &Path, bytes: &[u8], action: &'static str) -> Result<(), BuildError> {
    let parent = path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).map_err(|source| BuildError::Io {
        action: "create output directory",
        path: parent.to_owned(),
        source,
    })?;
    let mut temporary = NamedTempFile::new_in(parent).map_err(|source| BuildError::Io {
        action,
        path: path.to_owned(),
        source,
    })?;
    temporary.write_all(bytes).map_err(|source| BuildError::Io {
        action,
        path: path.to_owned(),
        source,
    })?;
    temporary.flush().map_err(|source| BuildError::Io {
        action,
        path: path.to_owned(),
        source,
    })?;
    temporary.persist(path).map_err(|error| BuildError::Io {
        action,
        path: path.to_owned(),
        source: error.error,
    })?;
    Ok(())
}

fn inspect_command(args: &InputOutputArgs) -> Result<(), BuildError> {
    let materials = load_materials(&args.input)?;
    write_json(args.output.as_deref(), &inspect(&materials), "write inspection")
}

fn audit_command(args: &AuditArgs) -> Result<(), BuildError> {
    let materials = load_materials(&args.input)?;
    let profile = read_json_or_default::<AuditProfile>(args.profile.as_deref())?;
    write_json(
        Some(&args.report),
        &audit_library(&materials, &profile),
        "write audit report",
    )
}

fn validate_command(args: &AuditArgs) -> Result<(), BuildError> {
    let materials = load_materials(&args.input)?;
    let profile = read_json_or_default::<AuditProfile>(args.profile.as_deref())?;
    let report = audit_library(&materials, &profile);
    let invalid = report.material_count.saturating_sub(report.valid_materials);
    write_json(Some(&args.report), &report, "write validation report")?;
    if invalid == 0 {
        Ok(())
    } else {
        Err(BuildError::ValidationFailed(invalid))
    }
}

fn diff_command(args: &DiffArgs) -> Result<(), BuildError> {
    let before = load_materials(&args.before)?;
    let after = load_materials(&args.after)?;
    let before_by_id = before
        .iter()
        .map(|material| (&material.id.0, material))
        .collect::<BTreeMap<_, _>>();
    let after_by_id = after
        .iter()
        .map(|material| (&material.id.0, material))
        .collect::<BTreeMap<_, _>>();
    let ids = before_by_id
        .keys()
        .chain(after_by_id.keys())
        .copied()
        .collect::<BTreeSet<_>>();
    let mut report = Vec::new();
    for id in ids {
        let changes = match (before_by_id.get(id), after_by_id.get(id)) {
            (Some(before), Some(after)) => serde_json::json!(diff_materials(before, after)?),
            (Some(_), None) => serde_json::json!([{ "path": "", "before": "material", "after": null }]),
            (None, Some(_)) => serde_json::json!([{ "path": "", "before": null, "after": "material" }]),
            (None, None) => unreachable!(),
        };
        report.push(serde_json::json!({ "material_id": id, "changes": changes }));
    }
    write_json(args.output.as_deref(), &report, "write diff")
}

fn apply_command(args: &ApplyArgs) -> Result<(), BuildError> {
    let mut materials = load_materials(&args.input)?;
    let patch: Value = read_json(&args.patch)?;
    let index = selected_material_index(&materials, args.material_id.as_deref())?;
    materials[index] = apply_merge_patch(&materials[index], &patch)?;
    write_usdz(&materials, &args.output)
}

fn extract_texture_command(args: &ExtractTextureArgs) -> Result<(), BuildError> {
    let materials = load_materials(&args.input)?;
    let index = selected_material_index(&materials, args.material_id.as_deref())?;
    let role = parse_role(&args.role)?;
    let tier = parse_tier(&args.tier)?;
    let bytes = extract_texture(&materials[index], role, tier)
        .ok_or_else(|| MaterialWorkflowError::TextureNotFound(format!("{} {}", role, tier.as_str())))?;
    atomic_write(&args.output, &bytes, "write extracted texture")
}

fn attach_texture_command(args: &AttachTextureArgs) -> Result<(), BuildError> {
    let mut materials = load_materials(&args.input)?;
    let index = selected_material_index(&materials, args.material_id.as_deref())?;
    let role = parse_role(&args.role)?;
    let parameters = match &args.parameters {
        Some(path) => read_json(path)?,
        None => serde_json::json!({}),
    };
    let attachment = TextureAttachment {
        role,
        tier: parse_tier(&args.tier)?,
        bytes: read(&args.texture, "read attachment")?,
        color_space: parse_color_space(&args.color_space)?,
        channel: args
            .channel
            .as_deref()
            .map(parse_channel)
            .transpose()?
            .unwrap_or_else(|| default_channel(role)),
        normal_convention: args
            .normal_convention
            .as_deref()
            .map(parse_normal_convention)
            .transpose()?,
        parent: args.parent_hash.clone().map(Sha256),
        provenance: AttachmentProvenance {
            operation: parse_operation(&args.operation)?,
            tool: args.tool.clone(),
            tool_version: args.tool_version.clone(),
            model: args.model.clone(),
            parameters,
            created: parse_timestamp(&args.created)?,
        },
    };
    attach_texture(&mut materials[index], attachment)?;
    write_usdz(&materials, &args.output)
}

fn remove_texture_command(args: &RemoveTextureArgs) -> Result<(), BuildError> {
    let mut materials = load_materials(&args.input)?;
    let index = selected_material_index(&materials, args.material_id.as_deref())?;
    remove_texture(&mut materials[index], parse_role(&args.role)?, parse_tier(&args.tier)?)?;
    write_usdz(&materials, &args.output)
}

fn plan_tiers_command(args: &PolicyArgs) -> Result<(), BuildError> {
    let materials = load_materials(&args.input)?;
    let policy = read_json_or_default::<TierPolicy>(args.policy.as_deref())?;
    let jobs = materials
        .iter()
        .flat_map(|material| plan_tiers(material, &policy))
        .collect::<Vec<_>>();
    write_json(Some(&args.report), &jobs, "write tier plan")
}

fn complete_tiers_command(args: &CompleteTiersArgs) -> Result<(), BuildError> {
    let mut materials = load_materials(&args.input)?;
    let index = selected_material_index(&materials, args.material_id.as_deref())?;
    let policy = read_json_or_default::<TierPolicy>(args.policy.as_deref())?;
    let attached = generate_missing_downscales(&mut materials[index], &policy, parse_timestamp(&args.created)?)?;
    let remaining = plan_tiers(&materials[index], &policy);
    write_usdz(&materials, &args.output)?;
    write_json(
        Some(&args.report),
        &serde_json::json!({
            "schema": "usd-toolbox.tier-completion.v1",
            "attached_hashes": attached,
            "remaining_jobs": remaining,
        }),
        "write tier completion report",
    )
}

fn export_command(args: &ExportArgs) -> Result<(), BuildError> {
    let materials = load_materials(&args.input)?;
    let tier = args.tier.as_deref().map(parse_tier).transpose()?.unwrap_or(Tier::K2);
    let target = args.target.to_ascii_lowercase();
    let export = match target.as_str() {
        "usd" | "usdz" => UsdExporter.export(
            &materials,
            &UsdExportOptions {
                format: UsdFormat::Usdz,
                graph_tier: tier,
                include_provenance_mirror: true,
            },
        )?,
        "usda" => UsdExporter.export(
            &materials,
            &UsdExportOptions {
                format: UsdFormat::Usda,
                graph_tier: tier,
                include_provenance_mirror: false,
            },
        )?,
        "usdc" => UsdExporter.export(
            &materials,
            &UsdExportOptions {
                format: UsdFormat::Usdc,
                graph_tier: tier,
                include_provenance_mirror: false,
            },
        )?,
        "materialx" | "mtlx" => MaterialXExporter.export(
            &materials,
            &MaterialXExportOptions {
                graph_tier: tier,
                embed_payloads: true,
                include_neutral_manifest: true,
            },
        )?,
        "gltf" | "glb" => GltfExporter.export(
            &materials,
            &GltfExportOptions {
                format: if target == "gltf" {
                    GltfFormat::Gltf
                } else {
                    GltfFormat::Glb
                },
                texture_tier: tier,
                pretty: target == "gltf",
                include_olsyn_extras: true,
            },
        )?,
        "revit" => RevitExporter.export(
            &materials,
            &RevitExportOptions {
                texture_tier: tier,
                include_manifest: true,
            },
        )?,
        "omniverse" => OmniverseExporter.export(
            &materials,
            &OmniverseExportOptions {
                texture_tier: tier,
                include_provenance_mirror: true,
            },
        )?,
        _ => {
            return Err(BuildError::Unsupported(format!(
                "unknown target `{}`; expected usd, usda, usdc, materialx, gltf, glb, revit, or omniverse",
                args.target
            )));
        }
    };
    write_export(args, &target, export)
}

fn embedding_input_command(args: &EmbeddingInputArgs) -> Result<(), BuildError> {
    let materials = load_materials(&args.input)?;
    let index = selected_material_index(&materials, args.material_id.as_deref())?;
    let options = EmbeddingOptions {
        visual_tier: args
            .tier
            .as_deref()
            .map(parse_tier)
            .transpose()?
            .unwrap_or(Tier::Preview),
        ..EmbeddingOptions::default()
    };
    write_json(
        Some(&args.output),
        &prepare_embedding_input(&materials[index], &options),
        "write embedding input",
    )
}

fn cluster_command(args: &ClusterArgs) -> Result<(), BuildError> {
    let records: Vec<EmbeddingRecord> = read_json(&args.input)?;
    let result = cluster_embeddings(&records, args.similarity_threshold, args.min_cluster_size)?;
    write_json(Some(&args.output), &result, "write clusters")
}

fn migrate_command(args: &MigrateArgs) -> Result<(), BuildError> {
    let bytes = read(&args.input, "read neutral JSON")?;
    let document = migrate_document_json(&bytes)?;
    write_json(Some(&args.output), &document, "write migrated neutral JSON")
}

fn load_materials(path: &Path) -> Result<Vec<Material>, BuildError> {
    let bytes = read(path, "read material input")?;
    match path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("usd" | "usda" | "usdc" | "usdz") => Ok(UsdImporter.import(
            Input::Named {
                name: path.to_str().unwrap_or("material.usd"),
                bytes: &bytes,
            },
            &UsdImportOptions {
                allow_partial_graph: true,
                ..UsdImportOptions::default()
            },
        )?),
        Some("mtlx") => Ok(MaterialXImporter.import(
            Input::Named {
                name: path.to_str().unwrap_or("material.mtlx"),
                bytes: &bytes,
            },
            &MaterialXImportOptions {
                allow_partial_graph: true,
            },
        )?),
        Some("json") => Ok(migrate_document_json(&bytes)?.materials),
        extension => Err(BuildError::Unsupported(format!(
            "cannot infer material format from extension {:?}",
            extension.unwrap_or("none")
        ))),
    }
}

fn selected_material_index(materials: &[Material], requested: Option<&str>) -> Result<usize, BuildError> {
    if let Some(id) = requested {
        return materials
            .iter()
            .position(|material| material.id.0 == id)
            .ok_or_else(|| BuildError::Unsupported(format!("material `{id}` was not found")));
    }
    if materials.len() == 1 {
        Ok(0)
    } else {
        Err(BuildError::Unsupported(
            "--material-id is required when the input does not contain exactly one material".into(),
        ))
    }
}

fn write_usdz(materials: &[Material], path: &Path) -> Result<(), BuildError> {
    let export = UsdExporter.export(materials, &UsdExportOptions::default())?;
    atomic_write(path, &export.bytes, "write USDZ revision")
}

fn write_export(args: &ExportArgs, target: &str, export: Export) -> Result<(), BuildError> {
    let sha256 = format!("{:x}", Sha256Hasher::digest(&export.bytes));
    atomic_write(&args.output, &export.bytes, "write export")?;
    if let Some(report) = &args.report {
        write_json(
            Some(report),
            &serde_json::json!({
                "schema": "usd-toolbox.export.v1",
                "target": target,
                "losses": export.losses,
                "output": { "sha256": sha256, "bytes": export.bytes.len() },
            }),
            "write export report",
        )?;
    }
    Ok(())
}

fn write_json(path: Option<&Path>, value: &impl Serialize, action: &'static str) -> Result<(), BuildError> {
    let mut bytes = serde_json::to_vec_pretty(value).map_err(|source| BuildError::Manifest {
        path: path.unwrap_or_else(|| Path::new("<stdout>")).to_owned(),
        source,
    })?;
    bytes.push(b'\n');
    match path {
        Some(path) => atomic_write(path, &bytes, action),
        None => io::stdout().write_all(&bytes).map_err(|source| BuildError::Io {
            action,
            path: PathBuf::from("<stdout>"),
            source,
        }),
    }
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, BuildError> {
    let bytes = read(path, "read JSON")?;
    serde_json::from_slice(&bytes).map_err(|source| BuildError::Manifest {
        path: path.to_owned(),
        source,
    })
}

fn read_json_or_default<T: serde::de::DeserializeOwned + Default>(path: Option<&Path>) -> Result<T, BuildError> {
    path.map_or_else(|| Ok(T::default()), read_json)
}

fn parse_color_space(value: &str) -> Result<ColorSpace, BuildError> {
    match value.to_ascii_lowercase().as_str() {
        "srgb" => Ok(ColorSpace::Srgb),
        "raw" => Ok(ColorSpace::Raw),
        "linear" => Ok(ColorSpace::Linear),
        _ => Err(BuildError::Unsupported(format!("unknown color space `{value}`"))),
    }
}

fn parse_channel(value: &str) -> Result<Channel, BuildError> {
    match value.to_ascii_lowercase().as_str() {
        "rgb" => Ok(Channel::Rgb),
        "r" => Ok(Channel::R),
        "g" => Ok(Channel::G),
        "b" => Ok(Channel::B),
        "a" => Ok(Channel::A),
        _ => Err(BuildError::Unsupported(format!("unknown channel `{value}`"))),
    }
}

const fn default_channel(role: MapRole) -> Channel {
    if matches!(role, MapRole::BaseColor | MapRole::Normal | MapRole::Emissive) {
        Channel::Rgb
    } else {
        Channel::R
    }
}

fn parse_normal_convention(value: &str) -> Result<NormalConvention, BuildError> {
    match value.to_ascii_lowercase().replace(['-', '_'], "").as_str() {
        "opengl" | "gl" => Ok(NormalConvention::OpenGl),
        "directx" | "dx" => Ok(NormalConvention::DirectX),
        _ => Err(BuildError::Unsupported(format!("unknown normal convention `{value}`"))),
    }
}

fn parse_operation(value: &str) -> Result<Operation, BuildError> {
    match value.to_ascii_lowercase().as_str() {
        "authored" => Ok(Operation::Authored),
        "scanned" => Ok(Operation::Scanned),
        "downscale" => Ok(Operation::Downscale),
        "upscale" => Ok(Operation::Upscale),
        "generated" => Ok(Operation::Generated),
        "copied" => Ok(Operation::Copied),
        "converted" => Ok(Operation::Converted),
        "retouched" => Ok(Operation::Retouched),
        _ => Err(BuildError::Unsupported(format!(
            "unknown provenance operation `{value}`"
        ))),
    }
}

fn parse_timestamp(value: &str) -> Result<OffsetDateTime, BuildError> {
    OffsetDateTime::parse(value, &time::format_description::well_known::Rfc3339)
        .map_err(|error| BuildError::Unsupported(format!("invalid RFC3339 timestamp `{value}`: {error}")))
}
