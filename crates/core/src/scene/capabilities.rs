//! Core capability discovery intentionally separates source-format compatibility.
use super::*;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SceneCapabilities {
    pub schema: String,
    pub scene_schema: String,
    pub metadata_schema: String,
    pub query_schema: String,
    pub query_features: Vec<String>,
    pub edit_operations: Vec<String>,
    pub metadata_value_types: Vec<String>,
    pub limits: SceneLimits,
    pub metadata_limits: MetadataLimits,
    pub query_limits: QueryLimits,
    pub notes: Vec<String>,
}

pub fn scene_capabilities() -> SceneCapabilities {
    SceneCapabilities {
        schema:"usd-toolbox.scene-capabilities.v1".into(),scene_schema:SCENE_SCHEMA.into(),metadata_schema:METADATA_SCHEMA.into(),query_schema:"usd-toolbox.query-page.v1".into(),
        query_features:["stable_id","source_path","hierarchy","metadata","structured_filters","selective_metadata_indexes","projections","stable_sort","generation_cursors","count","summary","effective_bindings","cached_world_transforms","cached_physical_bounds"].map(str::to_owned).to_vec(),
        edit_operations:["triangulate_mesh","simplify_mesh","bake_transform","generate_normals","weld_vertices","remove_degenerate_faces","rename","reparent","add_node","remove_subtree","duplicate_subtree","merge_scene","set_pivot","convert_axis","set_metadata","set_transform","convert_units","replace_material","rebind_material","assign_material","set_visibility"].map(str::to_owned).to_vec(),
        metadata_value_types:["null","bool","string","i64","u64","f64","array","dictionary","typed","typed_array"].map(str::to_owned).to_vec(),
        limits:SceneLimits::default(),metadata_limits:MetadataLimits::default(),query_limits:QueryLimits::default(),
        notes:vec![
            "Core operations are portable and require no filesystem, network, browser globals, or native USD runtime.".into(),
            "Metadata integer values are canonical decimal strings. Missing keys, null values, and unresolved properties are distinct.".into(),
            "Metadata numeric filters compare i64/u64 exactly across integer types; f64 comparisons require both values to be f64. Equals is type-strict. Text case folding uses Unicode lowercase, without locale or Unicode normalization.".into(),
            "Source path lookup can return multiple stable identities after duplication or merge. Cursors bind exact scene-content revision, generation, and query; reopening unchanged content permits cross-process pagination.".into(),
            "Document metadata and hierarchy edits preserve geometry and texture allocations. Other edits and the copy-returning edit_scene API can copy payloads.".into(),
            "Import/export support and per-property source-format losses are reported separately by format adapters; typed storage does not imply universal round-trip fidelity.".into(),
        ],
    }
}
