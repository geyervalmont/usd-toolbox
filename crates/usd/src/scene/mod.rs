//! Strict static polygon scene adapter. Source bytes are supplied by the caller;
//! unsupported composition and shading semantics fail with path-addressed errors.
pub(crate) mod preflight;
mod read;
mod shading;
mod source_metadata;
mod transforms;
mod write;

pub use preflight::*;
pub use read::{asset_dependencies, import_scene};
pub use write::{export_scene, export_scene_assets};

use super::*;
use usd_toolbox_core::scene::*;

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct SceneImportOptions {
    pub limits: SceneLimits,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct SceneExportOptions {
    pub format: UsdFormat,
    pub texture_tier: Tier,
    pub limits: SceneLimits,
}

impl Default for SceneExportOptions {
    fn default() -> Self {
        Self {
            format: UsdFormat::Usdz,
            texture_tier: Tier::K2,
            limits: SceneLimits::default(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct Metadata {
    schema: String,
    id: String,
    name: String,
    nodes: BTreeMap<String, NodeMetadata>,
    dependencies: Vec<SceneDependency>,
    provenance: SceneProvenance,
    #[serde(default)]
    metadata: SceneMetadata,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct NodeMetadata {
    id: String,
    name: String,
    source_path: Option<String>,
    mesh: Option<String>,
}

fn err(code: &str, path: impl Into<String>, detail: impl std::fmt::Display) -> SceneError {
    SceneError::new(code, path, detail.to_string())
}

fn field(data: &dyn AbstractData, path: &Path, name: &str) -> Result<Option<UsdValue>, SceneError> {
    data.try_field(path, name)
        .map(|v| v.map(|v| v.into_owned()))
        .map_err(|e| err("usd.decode", path.as_str(), e))
}

fn value(data: &dyn AbstractData, prim: &Path, name: &str) -> Result<Option<UsdValue>, SceneError> {
    let path = prim
        .append_property(name)
        .map_err(|e| err("usd.path", prim.as_str(), e))?;
    field(data, &path, "default")
}

fn token(value: Option<UsdValue>) -> Option<String> {
    match value {
        Some(UsdValue::Token(v)) => Some(v.to_string()),
        Some(UsdValue::String(v)) => Some(v),
        _ => None,
    }
}

fn property_name(path: &Path) -> Option<&str> {
    path.as_str().rsplit_once('.').map(|(_, name)| name)
}

fn paths(value: Option<UsdValue>) -> Result<Vec<Path>, SceneError> {
    match value {
        None => Ok(Vec::new()),
        Some(UsdValue::PathListOp(v)) => Ok(v.compose_over(&[])),
        Some(UsdValue::PathVec(v)) => Ok(v),
        _ => Err(err("usd.relationship", "/", "expected path list")),
    }
}

fn ints(value: Option<UsdValue>, path: &str) -> Result<Vec<u32>, SceneError> {
    match value {
        Some(UsdValue::IntVec(v)) => v
            .into_iter()
            .map(|i| u32::try_from(i).map_err(|_| err("mesh.index", path, "negative index/count")))
            .collect(),
        _ => Err(err("usd.attribute", path, "expected int array")),
    }
}

fn vectors3(value: Option<UsdValue>, path: &str) -> Result<Vec<[f64; 3]>, SceneError> {
    match value {
        Some(UsdValue::Vec3fVec(v)) => Ok(v.into_iter().map(|v| <[f32; 3]>::from(v).map(f64::from)).collect()),
        Some(UsdValue::Vec3dVec(v)) => Ok(v.into_iter().map(<[f64; 3]>::from).collect()),
        _ => Err(err("usd.attribute", path, "expected 3-component vector array")),
    }
}

fn interpolation(
    data: &dyn AbstractData,
    prim: &Path,
    name: &str,
    fallback: Interpolation,
) -> Result<Interpolation, SceneError> {
    let path = prim
        .append_property(name)
        .map_err(|e| err("usd.path", prim.as_str(), e))?;
    match token(field(data, &path, "interpolation")?).as_deref() {
        None => Ok(fallback),
        Some("constant") => Ok(Interpolation::Constant),
        Some("uniform") => Ok(Interpolation::Uniform),
        Some("vertex") => Ok(Interpolation::Vertex),
        Some("varying") => Ok(Interpolation::Varying),
        Some("faceVarying") => Ok(Interpolation::FaceVarying),
        Some(v) => Err(err("usd.interpolation", path.as_str(), v)),
    }
}

fn interpolation_name(i: Interpolation) -> &'static str {
    match i {
        Interpolation::Constant => "constant",
        Interpolation::Uniform => "uniform",
        Interpolation::Vertex => "vertex",
        Interpolation::Varying => "varying",
        Interpolation::FaceVarying => "faceVarying",
    }
}

fn attribute<T>(
    data: &dyn AbstractData,
    prim: &Path,
    name: &str,
    fallback: Interpolation,
    values: Vec<T>,
) -> Result<Attribute<T>, SceneError> {
    let property = prim
        .append_property(name)
        .map_err(|e| err("usd.path", prim.as_str(), e))?;
    if field(data, &property, "elementSize")?.is_some_and(|v| v != UsdValue::Int(1))
        || field(data, &property, "unauthoredValuesIndex")?.is_some_and(|v| v != UsdValue::Int(-1))
    {
        return Err(err(
            "usd.primvar_unsupported",
            property.as_str(),
            "grouped elements and unauthored-value indices are not supported",
        ));
    }
    let indices = value(data, prim, &format!("{name}:indices"))?
        .map(|v| ints(Some(v), prim.as_str()))
        .transpose()?;
    Ok(Attribute {
        interpolation: interpolation(data, prim, name, fallback)?,
        values,
        indices,
    })
}

fn crate_limits(limits: &SceneLimits) -> openusd::usdc::ReadLimits {
    openusd::usdc::ReadLimits {
        max_allocation_bytes: limits.max_bytes.min(128 * 1024 * 1024),
        max_total_decoded_bytes: limits.max_bytes.saturating_mul(2),
        max_value_depth: limits.max_depth.min(32),
        max_value_calls: limits.max_nodes.saturating_mul(128).max(1024),
        max_path_bytes: limits.max_depth.saturating_mul(256).min(16 * 1024),
    }
}
