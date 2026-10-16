//! Static polygon scenes. Matrices are row-major, acting on column vectors;
//! world = parent * local. Coordinates are right-handed with explicit units.

mod capabilities;
mod geometry;
mod math;
mod metadata;
mod operations;
mod query;
mod triangulate;
mod validation;

pub use capabilities::*;
pub use geometry::{
    GeometryReport, MeshLodReport, MeshRepairOptions, MeshRepairResult, SceneLodExport, SceneLodExports,
    SceneLodLevelReport, SceneLodManifest, SceneLodOptions, SceneLodRequest, SceneLodSet, generate_scene_lods,
    repair_mesh,
};
pub use math::*;
pub use metadata::*;
pub use operations::*;
pub use query::*;
pub use triangulate::triangulate;
pub use validation::{require_valid_scene, validate_scene};

use crate::{Material, MaterialId};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const SCENE_SCHEMA: &str = "usd-toolbox.scene.v1";
pub type Matrix4 = [[f64; 4]; 4];
pub const IDENTITY: Matrix4 = [[1., 0., 0., 0.], [0., 1., 0., 0.], [0., 0., 1., 0.], [0., 0., 0., 1.]];

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
pub enum UpAxis {
    #[default]
    Y,
    Z,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Scene {
    pub schema: String,
    pub id: String,
    pub name: String,
    pub meters_per_unit: f64,
    pub up_axis: UpAxis,
    pub nodes: BTreeMap<String, SceneNode>,
    pub meshes: BTreeMap<String, Mesh>,
    pub materials: Vec<Material>,
    #[serde(default)]
    pub dependencies: Vec<SceneDependency>,
    #[serde(default)]
    pub provenance: SceneProvenance,
    #[serde(default)]
    pub metadata: SceneMetadata,
}

impl Scene {
    pub fn new(id: impl Into<String>) -> Self {
        let id = id.into();
        Self {
            schema: SCENE_SCHEMA.into(),
            name: id.clone(),
            id,
            meters_per_unit: 1.,
            up_axis: UpAxis::Y,
            nodes: BTreeMap::new(),
            meshes: BTreeMap::new(),
            materials: Vec::new(),
            dependencies: Vec::new(),
            provenance: SceneProvenance::default(),
            metadata: SceneMetadata::default(),
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SceneProvenance {
    pub source_name: Option<String>,
    pub source_sha256: Option<String>,
    #[serde(default)]
    pub metadata: BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    pub operations: Vec<SceneOperation>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct SceneOperation {
    pub operation: String,
    pub detail: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SceneDependency {
    pub authored_path: String,
    pub resolved_path: String,
    pub sha256: String,
    pub bytes: u64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SceneNode {
    pub name: String,
    pub source_path: Option<String>,
    pub parent: Option<String>,
    pub transform: Matrix4,
    #[serde(default)]
    pub reset_transform: bool,
    pub mesh: Option<String>,
    #[serde(default = "default_true")]
    pub visible: bool,
    #[serde(default)]
    pub purpose: Purpose,
    /// Direct binding; None inherits the nearest ancestor's binding.
    pub material: Option<MaterialId>,
    #[serde(default)]
    pub subsets: Vec<FaceBinding>,
}

const fn default_true() -> bool {
    true
}

impl SceneNode {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            source_path: None,
            parent: None,
            transform: IDENTITY,
            reset_transform: false,
            mesh: None,
            visible: true,
            purpose: Purpose::Inherited,
            material: None,
            subsets: Vec::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Purpose {
    #[default]
    Inherited,
    Default,
    Render,
    Proxy,
    Guide,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FaceBinding {
    pub name: String,
    pub faces: Vec<u32>,
    pub material: MaterialId,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Orientation {
    #[default]
    RightHanded,
    LeftHanded,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Interpolation {
    Constant,
    Uniform,
    Vertex,
    Varying,
    FaceVarying,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Attribute<T> {
    pub interpolation: Interpolation,
    pub values: Vec<T>,
    #[serde(default)]
    pub indices: Option<Vec<u32>>,
}

impl<T> Attribute<T> {
    pub fn sample(&self, face: usize, corner: usize, vertex: usize) -> Option<&T> {
        let index = match self.interpolation {
            Interpolation::Constant => 0,
            Interpolation::Uniform => face,
            Interpolation::Vertex | Interpolation::Varying => vertex,
            Interpolation::FaceVarying => corner,
        };
        let index = if let Some(indices) = &self.indices {
            *indices.get(index)? as usize
        } else {
            index
        };
        self.values.get(index)
    }
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Mesh {
    pub positions: Vec<[f64; 3]>,
    pub face_counts: Vec<u32>,
    pub face_indices: Vec<u32>,
    #[serde(default)]
    pub orientation: Orientation,
    #[serde(default)]
    pub double_sided: bool,
    pub normals: Option<Attribute<[f64; 3]>>,
    /// Tangent xyz and bitangent handedness w, relative to the primary UV set.
    #[serde(default)]
    pub tangents: Option<Attribute<[f32; 4]>>,
    #[serde(default)]
    pub uv_sets: BTreeMap<String, Attribute<[f32; 2]>>,
    pub colors: Option<Attribute<[f32; 4]>>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct SceneLimits {
    pub max_nodes: usize,
    pub max_depth: usize,
    pub max_points: usize,
    pub max_faces: usize,
    pub max_corners: usize,
    pub max_face_corners: usize,
    pub max_bytes: usize,
    pub max_dependencies: usize,
    pub max_operations: usize,
}

impl Default for SceneLimits {
    fn default() -> Self {
        Self {
            max_nodes: 100_000,
            max_depth: 256,
            max_points: 5_000_000,
            max_faces: 5_000_000,
            max_corners: 15_000_000,
            max_face_corners: 1024,
            max_bytes: 512 * 1024 * 1024,
            max_dependencies: 10_000,
            max_operations: 10_000,
        }
    }
}

impl SceneLimits {
    pub fn browser() -> Self {
        Self {
            max_nodes: 20_000,
            max_points: 1_000_000,
            max_faces: 1_000_000,
            max_corners: 3_000_000,
            max_bytes: 128 * 1024 * 1024,
            ..Self::default()
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SceneSeverity {
    Error,
    Warning,
    Info,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct SceneDiagnostic {
    pub code: String,
    pub severity: SceneSeverity,
    pub path: String,
    pub detail: String,
}

impl SceneDiagnostic {
    pub fn warning(code: &str, path: &str, detail: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            severity: SceneSeverity::Warning,
            path: path.into(),
            detail: detail.into(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SceneImport {
    pub scene: Scene,
    pub diagnostics: Vec<SceneDiagnostic>,
}

#[derive(Clone, Debug)]
pub struct SceneExport {
    pub bytes: Vec<u8>,
    pub diagnostics: Vec<SceneDiagnostic>,
}

#[derive(Clone, Debug, thiserror::Error)]
#[error("{code} at {path}: {detail}")]
pub struct SceneError {
    pub code: String,
    pub path: String,
    pub detail: String,
}

impl SceneError {
    pub fn new(code: &str, path: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            path: path.into(),
            detail: detail.into(),
        }
    }
}
