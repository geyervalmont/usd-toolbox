use super::*;
use std::collections::BTreeSet;

#[derive(Clone, Debug, Serialize)]
pub struct SceneInspection {
    pub schema: &'static str,
    pub id: String,
    pub name: String,
    pub meters_per_unit: f64,
    pub up_axis: UpAxis,
    pub nodes: usize,
    pub unique_meshes: usize,
    pub mesh_occurrences: usize,
    pub unique_points: usize,
    pub unique_faces: usize,
    pub placed_faces: usize,
    pub materials: Vec<MaterialId>,
    pub dependencies: Vec<SceneDependency>,
    pub bounds: Option<Bounds>,
    pub mesh_bounds: BTreeMap<String, Bounds>,
    pub diagnostics: Vec<SceneDiagnostic>,
}

pub fn inspect_scene(scene: &Scene, limits: &SceneLimits) -> Result<SceneInspection, SceneError> {
    require_valid_scene(scene, limits)?;
    Ok(SceneInspection {
        schema: "usd-toolbox.scene-inspection.v1",
        id: scene.id.clone(),
        name: scene.name.clone(),
        meters_per_unit: scene.meters_per_unit,
        up_axis: scene.up_axis,
        nodes: scene.nodes.len(),
        unique_meshes: scene.meshes.len(),
        mesh_occurrences: scene.nodes.values().filter(|n| n.mesh.is_some()).count(),
        unique_points: scene.meshes.values().map(|m| m.positions.len()).sum(),
        unique_faces: scene.meshes.values().map(|m| m.face_counts.len()).sum(),
        placed_faces: scene
            .nodes
            .values()
            .filter_map(|n| n.mesh.as_ref())
            .map(|m| scene.meshes[m].face_counts.len())
            .sum(),
        materials: scene.materials.iter().map(|m| m.id.clone()).collect(),
        dependencies: scene.dependencies.clone(),
        bounds: scene_bounds(scene)?,
        mesh_bounds: scene
            .meshes
            .iter()
            .filter_map(|(id, m)| mesh_bounds(m).map(|b| (id.clone(), b)))
            .collect(),
        diagnostics: validate_scene(scene, limits),
    })
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum SceneEdit {
    TriangulateMesh {
        mesh: String,
    },
    SimplifyMesh {
        mesh: String,
        target_faces: usize,
        max_vertex_displacement: f64,
    },
    BakeTransform {
        node: String,
    },
    GenerateNormals {
        mesh: String,
        crease_angle_degrees: f64,
        #[serde(default = "default_true")]
        area_weighted: bool,
    },
    WeldVertices {
        mesh: String,
        tolerance: f64,
    },
    RemoveDegenerateFaces {
        mesh: String,
        #[serde(default)]
        area_tolerance: f64,
    },
    Rename {
        node: String,
        name: String,
    },
    Reparent {
        node: String,
        parent: Option<String>,
        #[serde(default = "default_true")]
        preserve_world: bool,
        #[serde(default)]
        preserve_inherited: bool,
    },
    AddNode {
        id: String,
        node: Box<SceneNode>,
    },
    RemoveSubtree {
        node: String,
    },
    DuplicateSubtree {
        node: String,
        prefix: String,
        parent: Option<String>,
        #[serde(default = "default_true")]
        preserve_world: bool,
    },
    MergeScene {
        scene: Box<Scene>,
        prefix: String,
        parent: Option<String>,
    },
    /// Move the local origin while retaining this mesh and its children's placement.
    SetPivot {
        node: String,
        pivot: [f64; 3],
    },
    ConvertAxis {
        up_axis: UpAxis,
    },
    SetMetadata {
        target: MetadataTarget,
        #[serde(default)]
        namespace: MetadataNamespace,
        key: String,
        value: Option<MetadataProperty>,
    },
    SetTransform {
        node: String,
        transform: Matrix4,
    },
    /// Change coordinate units while preserving physical dimensions.
    ConvertUnits {
        meters_per_unit: f64,
    },
    ReplaceMaterial {
        material: Box<Material>,
    },
    /// Replace a source material with an independently identified library material.
    RebindMaterial {
        source: MaterialId,
        material: Box<Material>,
    },
    AssignMaterial {
        node: String,
        material: MaterialId,
    },
    SetVisibility {
        node: String,
        visible: bool,
    },
}

/// Applies edits to a copy, returning no partial result on failure.
pub fn edit_scene(scene: &Scene, edits: &[SceneEdit], limits: &SceneLimits) -> Result<Scene, SceneError> {
    require_valid_scene(scene, limits)?;
    let mut output = scene.clone();
    apply_scene_edits(&mut output, edits, limits)?;
    require_valid_scene(&output, limits)?;
    scene_bounds(&output)?;
    Ok(output)
}

pub(crate) fn apply_scene_edits(
    output: &mut Scene,
    edits: &[SceneEdit],
    limits: &SceneLimits,
) -> Result<(), SceneError> {
    for edit in edits {
        let detail = match edit {
            SceneEdit::TriangulateMesh { mesh } => geometry_detail(geometry::triangulate_mesh(output, mesh, limits)?)?,
            SceneEdit::SimplifyMesh {
                mesh,
                target_faces,
                max_vertex_displacement,
            } => geometry_detail(geometry::simplify(
                output,
                mesh,
                *target_faces,
                *max_vertex_displacement,
                limits,
            )?)?,
            SceneEdit::BakeTransform { node } => geometry_detail(geometry::bake_transform(output, node)?)?,
            SceneEdit::GenerateNormals {
                mesh,
                crease_angle_degrees,
                area_weighted,
            } => geometry_detail(geometry::generate_normals(
                output,
                mesh,
                *crease_angle_degrees,
                *area_weighted,
                limits,
            )?)?,
            SceneEdit::WeldVertices { mesh, tolerance } => {
                geometry_detail(geometry::weld(output, mesh, *tolerance, limits)?)?
            }
            SceneEdit::RemoveDegenerateFaces { mesh, area_tolerance } => {
                geometry_detail(geometry::remove_degenerate(output, mesh, *area_tolerance, limits)?)?
            }
            SceneEdit::Rename { node, name } => {
                node_mut(output, node)?.name = name.clone();
                format!("renamed {node}; stable identity and source path retained")
            }
            SceneEdit::AddNode { id, node } => {
                if id.is_empty() || output.nodes.contains_key(id) {
                    return Err(SceneError::new(
                        "scene.identity_collision",
                        id,
                        "new node ID is empty or already exists",
                    ));
                }
                output.nodes.insert(id.clone(), (**node).clone());
                format!("added node {id}")
            }
            SceneEdit::RemoveSubtree { node } => {
                let ids = subtree_ids(output, node)?;
                output.nodes.retain(|id, _| !ids.contains(id));
                output.metadata.nodes.retain(|id, _| !ids.contains(id));
                format!(
                    "removed {} nodes below {node}; reusable mesh/material assets retained",
                    ids.len()
                )
            }
            SceneEdit::Reparent {
                node,
                parent,
                preserve_world,
                preserve_inherited,
            } => {
                reparent(output, node, parent.as_deref(), *preserve_world, *preserve_inherited)?;
                format!(
                    "reparented {node}; world preservation={preserve_world}; inheritance preservation={preserve_inherited}"
                )
            }
            SceneEdit::DuplicateSubtree {
                node,
                prefix,
                parent,
                preserve_world,
            } => {
                duplicate(output, node, prefix, parent.as_deref(), *preserve_world)?;
                format!("duplicated {node} with prefix {prefix}; geometry/material storage shared")
            }
            SceneEdit::MergeScene { scene, prefix, parent } => {
                merge(output, scene, prefix, parent.as_deref(), limits)?;
                format!("merged scene {} using identity prefix {prefix}", scene.id)
            }
            SceneEdit::SetPivot { node, pivot } => {
                set_pivot(output, node, *pivot)?;
                format!("adjusted local origin of {node}; placement retained")
            }
            SceneEdit::ConvertAxis { up_axis } => {
                convert_axis(output, *up_axis);
                format!("converted up axis to {up_axis:?}")
            }
            SceneEdit::SetMetadata {
                target,
                namespace,
                key,
                value,
            } => {
                if !metadata_target_exists(output, target) {
                    return Err(SceneError::new(
                        "metadata.target",
                        key,
                        "metadata target does not exist",
                    ));
                }
                if key.is_empty() {
                    return Err(SceneError::new("metadata.key", key, "metadata key must not be empty"));
                }
                let metadata = output.metadata.get_or_insert(target);
                let properties = match namespace {
                    MetadataNamespace::Original => &mut metadata.original,
                    MetadataNamespace::Normalized => &mut metadata.normalized,
                };
                if let Some(value) = value {
                    properties.insert(key.clone(), value.clone());
                } else {
                    properties.remove(key);
                }
                format!("updated {namespace:?} metadata {key} on {target:?}")
            }
            SceneEdit::SetTransform { node, transform } => {
                output
                    .nodes
                    .get_mut(node)
                    .ok_or_else(|| SceneError::new("scene.missing_node", node, "edit target not found"))?
                    .transform = *transform;
                format!("set transform of {node}")
            }
            SceneEdit::ConvertUnits { meters_per_unit } => {
                if !meters_per_unit.is_finite() || *meters_per_unit <= 0. {
                    return Err(SceneError::new("scene.units", "/", "units must be finite and positive"));
                }
                let scale = output.meters_per_unit / meters_per_unit;
                for mesh in output.meshes.values_mut() {
                    for p in &mut mesh.positions {
                        for v in p {
                            *v *= scale;
                        }
                    }
                }
                for node in output.nodes.values_mut() {
                    for r in 0..3 {
                        node.transform[r][3] *= scale;
                    }
                }
                let detail = format!(
                    "converted units from {} to {meters_per_unit} meters",
                    output.meters_per_unit
                );
                output.meters_per_unit = *meters_per_unit;
                detail
            }
            SceneEdit::ReplaceMaterial { material } => {
                let target = output
                    .materials
                    .iter_mut()
                    .find(|m| m.id == material.id)
                    .ok_or_else(|| {
                        SceneError::new(
                            "scene.missing_material",
                            material.id.0.clone(),
                            "replacement must retain an existing material ID",
                        )
                    })?;
                *target = (**material).clone();
                format!("replaced material {}", material.id)
            }
            SceneEdit::RebindMaterial { source, material } => {
                if !output.materials.iter().any(|m| &m.id == source) {
                    return Err(SceneError::new(
                        "scene.missing_material",
                        &source.0,
                        "source material not found",
                    ));
                }
                if &material.id != source
                    && output
                        .materials
                        .iter()
                        .any(|m| m.id == material.id && m != material.as_ref())
                {
                    return Err(SceneError::new(
                        "scene.material_conflict",
                        &material.id.0,
                        "existing identity has different material content",
                    ));
                }
                for node in output.nodes.values_mut() {
                    if node.material.as_ref() == Some(source) {
                        node.material = Some(material.id.clone());
                    }
                    for subset in &mut node.subsets {
                        if &subset.material == source {
                            subset.material = material.id.clone();
                        }
                    }
                }
                if source != &material.id
                    && let Some(metadata) = output.metadata.materials.remove(&source.0)
                {
                    let target = output.metadata.materials.entry(material.id.0.clone()).or_default();
                    for (key, value) in metadata.original {
                        target.original.entry(key).or_insert(value);
                    }
                    for (key, value) in metadata.normalized {
                        target.normalized.entry(key).or_insert(value);
                    }
                }
                output.materials.retain(|m| &m.id != source);
                if !output.materials.iter().any(|m| m.id == material.id) {
                    output.materials.push((**material).clone());
                }
                format!("rebound material {source} to {}", material.id)
            }
            SceneEdit::AssignMaterial { node, material } => {
                output
                    .nodes
                    .get_mut(node)
                    .ok_or_else(|| SceneError::new("scene.missing_node", node, "edit target not found"))?
                    .material = Some(material.clone());
                format!("assigned material {material} to {node}; face subsets retained")
            }
            SceneEdit::SetVisibility { node, visible } => {
                output
                    .nodes
                    .get_mut(node)
                    .ok_or_else(|| SceneError::new("scene.missing_node", node, "edit target not found"))?
                    .visible = *visible;
                format!("set visibility of {node} to {visible}")
            }
        };
        output.provenance.operations.push(SceneOperation {
            operation: "edit".into(),
            detail,
        });
    }
    Ok(())
}

pub fn extract_subtree(scene: &Scene, root: &str, limits: &SceneLimits) -> Result<Scene, SceneError> {
    require_valid_scene(scene, limits)?;
    let world = world_transform(scene, root)?;
    let keep = subtree_ids(scene, root)?;
    let mut output = scene.clone();
    output.nodes.retain(|id, _| keep.contains(id));
    output.metadata.nodes.retain(|id, _| keep.contains(id));
    // Preserve inherited visibility and purpose when detaching the subtree.
    let (visible, purpose) = effective_visibility(scene, root)?;
    let node = output.nodes.get_mut(root).expect("world_transform checked root");
    node.parent = None;
    node.transform = world;
    node.reset_transform = false;
    node.visible = visible;
    node.purpose = purpose;
    node.material = effective_material(scene, root)?;
    let meshes: BTreeSet<_> = output.nodes.values().filter_map(|n| n.mesh.clone()).collect();
    output.meshes.retain(|id, _| meshes.contains(id));
    output.metadata.meshes.retain(|id, _| meshes.contains(id));
    let materials: BTreeSet<_> = output
        .nodes
        .values()
        .flat_map(|n| n.material.iter().chain(n.subsets.iter().map(|s| &s.material)))
        .cloned()
        .collect();
    output.materials.retain(|m| materials.contains(&m.id));
    output
        .metadata
        .materials
        .retain(|id, _| materials.contains(&MaterialId::new(id)));
    output.id = format!("{}:{root}", scene.id);
    output.name = node_name(scene, root);
    output.provenance.operations.push(SceneOperation {
        operation: "extract".into(),
        detail: format!("subtree {root} from {}; world placement retained", scene.id),
    });
    require_valid_scene(&output, limits)?;
    Ok(output)
}

fn node_name(scene: &Scene, root: &str) -> String {
    scene.nodes[root].name.clone()
}

pub fn effective_visibility(scene: &Scene, id: &str) -> Result<(bool, Purpose), SceneError> {
    let mut current = Some(id);
    let mut seen = BTreeSet::new();
    let mut visible = true;
    let mut purpose = Purpose::Inherited;
    while let Some(id) = current {
        if !seen.insert(id) {
            return Err(SceneError::new("scene.cycle", id, "parent cycle"));
        }
        let node = scene
            .nodes
            .get(id)
            .ok_or_else(|| SceneError::new("scene.missing_node", id, "node not found"))?;
        visible &= node.visible;
        if purpose == Purpose::Inherited {
            purpose = node.purpose;
        }
        current = node.parent.as_deref();
    }
    Ok((
        visible,
        if purpose == Purpose::Inherited {
            Purpose::Default
        } else {
            purpose
        },
    ))
}

pub fn effective_material(scene: &Scene, id: &str) -> Result<Option<MaterialId>, SceneError> {
    let mut current = Some(id);
    let mut seen = BTreeSet::new();
    while let Some(id) = current {
        if !seen.insert(id) {
            return Err(SceneError::new("scene.cycle", id, "parent cycle"));
        }
        let node = scene
            .nodes
            .get(id)
            .ok_or_else(|| SceneError::new("scene.missing_node", id, "node not found"))?;
        if let Some(material) = &node.material {
            return Ok(Some(material.clone()));
        }
        current = node.parent.as_deref();
    }
    Ok(None)
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SceneChange {
    pub entity: String,
    pub change: String,
    pub fields: Vec<String>,
}

/// Semantic entity/field changes without embedding geometry or texture payloads.
pub fn diff_scenes(before: &Scene, after: &Scene) -> Vec<SceneChange> {
    let mut changes = diff_scene_headers(before, after);
    for id in before.meshes.keys().chain(after.meshes.keys()).collect::<BTreeSet<_>>() {
        let fields = match (before.meshes.get(id), after.meshes.get(id)) {
            (Some(a), Some(b)) => [
                ("positions", a.positions != b.positions),
                ("face_counts", a.face_counts != b.face_counts),
                ("face_indices", a.face_indices != b.face_indices),
                ("orientation", a.orientation != b.orientation),
                ("double_sided", a.double_sided != b.double_sided),
                ("normals", a.normals != b.normals),
                ("tangents", a.tangents != b.tangents),
                ("uv_sets", a.uv_sets != b.uv_sets),
                ("colors", a.colors != b.colors),
            ]
            .into_iter()
            .filter(|(_, v)| *v)
            .map(|(k, _)| k.to_owned())
            .collect(),
            _ => Vec::new(),
        };
        append_change(
            &mut changes,
            "mesh",
            id,
            before.meshes.contains_key(id),
            after.meshes.contains_key(id),
            fields,
        );
    }
    let a: BTreeMap<_, _> = before.materials.iter().map(|m| (&m.id.0, m)).collect();
    let b: BTreeMap<_, _> = after.materials.iter().map(|m| (&m.id.0, m)).collect();
    for id in a.keys().chain(b.keys()).collect::<BTreeSet<_>>() {
        let fields = match (a.get(id), b.get(id)) {
            (Some(a), Some(b)) => [
                ("name", a.name != b.name),
                ("model", a.model != b.model),
                ("surface", a.surface != b.surface),
                ("geometry", a.geometry != b.geometry),
                ("tiling", a.tiling != b.tiling),
                ("auxiliary", a.auxiliary != b.auxiliary),
                ("provenance", a.provenance != b.provenance),
                ("materialx", a.materialx != b.materialx),
                ("variants", a.variants != b.variants),
            ]
            .into_iter()
            .filter(|(_, v)| *v)
            .map(|(k, _)| k.to_owned())
            .collect(),
            _ => Vec::new(),
        };
        append_change(
            &mut changes,
            "material",
            id,
            a.contains_key(id),
            b.contains_key(id),
            fields,
        );
    }
    changes
}
fn append_change(changes: &mut Vec<SceneChange>, kind: &str, id: &str, before: bool, after: bool, fields: Vec<String>) {
    let change = match (before, after) {
        (false, true) => "added",
        (true, false) => "removed",
        (true, true) if !fields.is_empty() => "modified",
        _ => return,
    };
    changes.push(SceneChange {
        entity: format!("{kind}:{id}"),
        change: change.into(),
        fields,
    });
}

fn diff_entities<T: PartialEq + Serialize>(
    kind: &str,
    before: &BTreeMap<String, T>,
    after: &BTreeMap<String, T>,
    changes: &mut Vec<SceneChange>,
) {
    for id in before.keys().chain(after.keys()).collect::<BTreeSet<_>>() {
        let (change, fields) = match (before.get(id), after.get(id)) {
            (Some(a), Some(b)) if a != b => {
                let a = serde_json::to_value(a).unwrap_or_default();
                let b = serde_json::to_value(b).unwrap_or_default();
                let fields = a
                    .as_object()
                    .into_iter()
                    .flat_map(|o| o.keys())
                    .filter(|key| a[*key] != b[*key])
                    .cloned()
                    .collect();
                ("modified", fields)
            }
            (None, Some(_)) => ("added", Vec::new()),
            (Some(_), None) => ("removed", Vec::new()),
            _ => continue,
        };
        changes.push(SceneChange {
            entity: format!("{kind}:{id}"),
            change: change.into(),
            fields,
        });
    }
}

impl SceneEdit {
    /// This category never mutates meshes, materials, textures or dependency bytes.
    pub fn is_lightweight(&self) -> bool {
        matches!(
            self,
            Self::Rename { .. }
                | Self::Reparent { .. }
                | Self::AddNode { .. }
                | Self::RemoveSubtree { .. }
                | Self::DuplicateSubtree { .. }
                | Self::SetMetadata { .. }
                | Self::SetTransform { .. }
                | Self::SetVisibility { .. }
                | Self::AssignMaterial { .. }
                | Self::ConvertAxis { .. }
        )
    }
}
fn node_mut<'a>(scene: &'a mut Scene, id: &str) -> Result<&'a mut SceneNode, SceneError> {
    scene
        .nodes
        .get_mut(id)
        .ok_or_else(|| SceneError::new("scene.missing_node", id, "edit target not found"))
}
fn metadata_target_exists(scene: &Scene, target: &MetadataTarget) -> bool {
    match target {
        MetadataTarget::Scene => true,
        MetadataTarget::Node(id) => scene.nodes.contains_key(id),
        MetadataTarget::Mesh(id) => scene.meshes.contains_key(id),
        MetadataTarget::Material(id) => scene.materials.iter().any(|m| m.id.0 == *id),
    }
}
fn subtree_ids(scene: &Scene, root: &str) -> Result<BTreeSet<String>, SceneError> {
    if !scene.nodes.contains_key(root) {
        return Err(SceneError::new("scene.missing_node", root, "subtree root not found"));
    }
    let mut children: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (id, node) in &scene.nodes {
        if let Some(parent) = node.parent.as_deref() {
            children.entry(parent).or_default().push(id);
        }
    }
    let mut keep = BTreeSet::new();
    let mut pending = vec![root];
    while let Some(id) = pending.pop() {
        if !keep.insert(id.to_owned()) {
            return Err(SceneError::new("scene.cycle", id, "subtree contains cycle"));
        }
        pending.extend(children.get(id).into_iter().flatten().copied());
    }
    Ok(keep)
}
fn reparent(
    scene: &mut Scene,
    id: &str,
    parent: Option<&str>,
    preserve_world: bool,
    preserve_inherited: bool,
) -> Result<(), SceneError> {
    let ids = subtree_ids(scene, id)?;
    if parent.is_some_and(|p| ids.contains(p)) {
        return Err(SceneError::new(
            "scene.cycle",
            id,
            "cannot parent a node beneath itself",
        ));
    }
    let world = if preserve_world {
        Some(world_transform(scene, id)?)
    } else {
        None
    };
    let parent_world = parent
        .map(|p| world_transform(scene, p))
        .transpose()?
        .unwrap_or(IDENTITY);
    let local = world
        .map(|world| {
            inverse(parent_world)
                .map(|parent| multiply(parent, world))
                .ok_or_else(|| SceneError::new("scene.singular_parent", id, "new parent has no inverse"))
        })
        .transpose()?;
    let inherited = if preserve_inherited {
        Some((effective_visibility(scene, id)?, effective_material(scene, id)?))
    } else {
        None
    };
    if let Some(((visible, _), material)) = &inherited {
        if *visible
            && parent
                .map(|p| effective_visibility(scene, p))
                .transpose()?
                .is_some_and(|(v, _)| !v)
        {
            return Err(SceneError::new(
                "scene.inheritance",
                id,
                "cannot preserve visibility beneath an invisible parent",
            ));
        }
        if material.is_none()
            && parent
                .map(|p| effective_material(scene, p))
                .transpose()?
                .flatten()
                .is_some()
        {
            return Err(SceneError::new(
                "scene.inheritance",
                id,
                "cannot represent an unbound override beneath a bound parent",
            ));
        }
    }
    let node = node_mut(scene, id)?;
    node.parent = parent.map(str::to_owned);
    if let Some(local) = local {
        node.transform = local;
        node.reset_transform = false;
    }
    if let Some(((visible, purpose), material)) = inherited {
        node.visible = visible;
        node.purpose = purpose;
        node.material = material;
    }
    Ok(())
}
fn duplicate(
    scene: &mut Scene,
    root: &str,
    prefix: &str,
    parent: Option<&str>,
    preserve_world: bool,
) -> Result<(), SceneError> {
    if prefix.is_empty() {
        return Err(SceneError::new(
            "scene.identity_collision",
            root,
            "duplicate needs a nonempty prefix",
        ));
    }
    let ids = subtree_ids(scene, root)?;
    if let Some(parent) = parent
        && !scene.nodes.contains_key(parent)
    {
        return Err(SceneError::new("scene.missing_node", parent, "new parent not found"));
    }
    let mut copied = Vec::new();
    for id in &ids {
        let next = format!("{prefix}{id}");
        if scene.nodes.contains_key(&next) {
            return Err(SceneError::new(
                "scene.identity_collision",
                &next,
                "duplicated node ID already exists",
            ));
        }
        let mut node = scene.nodes[id].clone();
        if id != root {
            node.parent = node.parent.map(|id| format!("{prefix}{id}"));
        }
        copied.push((next, node));
    }
    for (id, node) in copied {
        scene.nodes.insert(id, node);
    }
    for id in &ids {
        if let Some(metadata) = scene.metadata.nodes.get(id).cloned() {
            scene.metadata.nodes.insert(format!("{prefix}{id}"), metadata);
        }
    }
    reparent(scene, &format!("{prefix}{root}"), parent, preserve_world, true)
}
fn convert_axis(scene: &mut Scene, axis: UpAxis) {
    if scene.up_axis == axis {
        return;
    }
    let mut rotation = IDENTITY;
    rotation[1][1] = 0.;
    rotation[2][2] = 0.;
    match axis {
        UpAxis::Z => {
            rotation[1][2] = -1.;
            rotation[2][1] = 1.;
        }
        UpAxis::Y => {
            rotation[1][2] = 1.;
            rotation[2][1] = -1.;
        }
    }
    for node in scene.nodes.values_mut() {
        if node.parent.is_none() || node.reset_transform {
            node.transform = multiply(rotation, node.transform);
        }
    }
    scene.up_axis = axis;
}
fn set_pivot(scene: &mut Scene, id: &str, pivot: [f64; 3]) -> Result<(), SceneError> {
    if pivot.iter().any(|v| !v.is_finite()) {
        return Err(SceneError::new("scene.pivot", id, "pivot must be finite"));
    }
    let node = scene
        .nodes
        .get(id)
        .ok_or_else(|| SceneError::new("scene.missing_node", id, "pivot target not found"))?;
    let old_mesh = node.mesh.clone();
    let mut translation = IDENTITY;
    for r in 0..3 {
        translation[r][3] = pivot[r];
    }
    let mut inverse_translation = IDENTITY;
    for r in 0..3 {
        inverse_translation[r][3] = -pivot[r];
    }
    if let Some(mesh_id) = old_mesh {
        if !scene.meshes.contains_key(&mesh_id) {
            return Err(SceneError::new(
                "scene.missing_mesh",
                &mesh_id,
                "pivot mesh is unresolved",
            ));
        }
        let shared = scene
            .nodes
            .iter()
            .any(|(other, n)| other != id && n.mesh.as_ref() == Some(&mesh_id));
        let target = if shared {
            let mut suffix = 1;
            let mut candidate = format!("{mesh_id}:pivot:{id}");
            while scene.meshes.contains_key(&candidate) {
                candidate = format!("{mesh_id}:pivot:{id}:{suffix}");
                suffix += 1;
            }
            let mesh = scene.meshes[&mesh_id].clone();
            scene.meshes.insert(candidate.clone(), mesh);
            if let Some(metadata) = scene.metadata.meshes.get(&mesh_id).cloned() {
                scene.metadata.meshes.insert(candidate.clone(), metadata);
            }
            candidate
        } else {
            mesh_id
        };
        for p in &mut scene.meshes.get_mut(&target).expect("existing mesh").positions {
            for r in 0..3 {
                p[r] -= pivot[r];
            }
        }
        node_mut(scene, id)?.mesh = Some(target);
    }
    let node = node_mut(scene, id)?;
    node.transform = multiply(node.transform, translation);
    for node in scene.nodes.values_mut() {
        if node.parent.as_deref() == Some(id) && !node.reset_transform {
            node.transform = multiply(inverse_translation, node.transform);
        }
    }
    Ok(())
}
fn merge(
    scene: &mut Scene,
    incoming: &Scene,
    prefix: &str,
    parent: Option<&str>,
    limits: &SceneLimits,
) -> Result<(), SceneError> {
    require_valid_scene(incoming, limits)?;
    if prefix.is_empty() {
        return Err(SceneError::new(
            "scene.identity_collision",
            "/",
            "merge requires a nonempty identity prefix",
        ));
    }
    if let Some(parent) = parent
        && !scene.nodes.contains_key(parent)
    {
        return Err(SceneError::new("scene.missing_node", parent, "merge parent not found"));
    }
    if let Some(parent) = parent {
        let parent_material = effective_material(scene, parent)?;
        let parent_visibility = effective_visibility(scene, parent)?.0;
        for (id, node) in &incoming.nodes {
            if node.parent.is_none() {
                if node.material.is_none() && parent_material.is_some() {
                    return Err(SceneError::new(
                        "scene.inheritance",
                        id,
                        "cannot preserve an unbound merged root beneath a bound parent",
                    ));
                }
                if node.visible && !parent_visibility {
                    return Err(SceneError::new(
                        "scene.inheritance",
                        id,
                        "cannot preserve visible merged roots beneath an invisible parent",
                    ));
                }
            }
        }
    }
    for id in incoming.nodes.keys() {
        if scene.nodes.contains_key(&format!("{prefix}{id}")) {
            return Err(SceneError::new(
                "scene.identity_collision",
                id,
                "merged node ID already exists",
            ));
        }
    }
    for id in incoming.meshes.keys() {
        if scene.meshes.contains_key(&format!("{prefix}{id}")) {
            return Err(SceneError::new(
                "scene.identity_collision",
                id,
                "merged mesh ID already exists",
            ));
        }
    }
    for material in &incoming.materials {
        if scene
            .materials
            .iter()
            .any(|m| m.id.0 == format!("{prefix}{}", material.id.0))
        {
            return Err(SceneError::new(
                "scene.identity_collision",
                &material.id.0,
                "merged material ID already exists",
            ));
        }
    }
    for dependency in &incoming.dependencies {
        if scene
            .dependencies
            .iter()
            .any(|d| d.resolved_path == dependency.resolved_path && d != dependency)
        {
            return Err(SceneError::new(
                "scene.dependency_collision",
                &dependency.resolved_path,
                "dependencies with identical paths have different content",
            ));
        }
    }
    let mut incoming = incoming.clone();
    apply_scene_edits(
        &mut incoming,
        &[
            SceneEdit::ConvertUnits {
                meters_per_unit: scene.meters_per_unit,
            },
            SceneEdit::ConvertAxis { up_axis: scene.up_axis },
        ],
        limits,
    )?;
    let parent_inverse = parent
        .map(|p| world_transform(scene, p))
        .transpose()?
        .map(inverse)
        .transpose_option()
        .ok_or_else(|| SceneError::new("scene.singular_parent", "/", "merge parent has no inverse"))?
        .unwrap_or(IDENTITY);
    for (id, mesh) in incoming.meshes {
        scene.meshes.insert(format!("{prefix}{id}"), mesh);
    }
    for mut material in incoming.materials {
        material.id = MaterialId::new(format!("{prefix}{}", material.id.0));
        scene.materials.push(material);
    }
    for (id, mut node) in incoming.nodes {
        if node.parent.is_none() {
            if node.purpose == Purpose::Inherited {
                node.purpose = Purpose::Default;
            }
            node.parent = parent.map(str::to_owned);
            if !node.reset_transform {
                node.transform = multiply(parent_inverse, node.transform);
            }
        } else {
            node.parent = node.parent.map(|id| format!("{prefix}{id}"));
        }
        node.mesh = node.mesh.map(|id| format!("{prefix}{id}"));
        node.material = node.material.map(|id| MaterialId::new(format!("{prefix}{}", id.0)));
        for subset in &mut node.subsets {
            subset.material = MaterialId::new(format!("{prefix}{}", subset.material.0));
        }
        scene.nodes.insert(format!("{prefix}{id}"), node);
    }
    for (id, metadata) in incoming.metadata.nodes {
        scene.metadata.nodes.insert(format!("{prefix}{id}"), metadata);
    }
    for (id, metadata) in incoming.metadata.meshes {
        scene.metadata.meshes.insert(format!("{prefix}{id}"), metadata);
    }
    for (id, metadata) in incoming.metadata.materials {
        scene.metadata.materials.insert(format!("{prefix}{id}"), metadata);
    }
    // Source scene metadata is retained as an explicitly namespaced nested record.
    scene.metadata.scene.original.insert(format!("merged_scene:{prefix}"),MetadataProperty::authored(MetadataValue::from_json(serde_json::json!({"id":incoming.id,"name":incoming.name,"metadata":incoming.metadata.scene,"provenance":incoming.provenance}))));
    for dependency in incoming.dependencies {
        if !scene.dependencies.contains(&dependency) {
            scene.dependencies.push(dependency);
        }
    }
    Ok(())
}
trait TransposeOption<T> {
    fn transpose_option(self) -> Option<Option<T>>;
}
impl<T> TransposeOption<T> for Option<Option<T>> {
    fn transpose_option(self) -> Option<Option<T>> {
        match self {
            Some(Some(v)) => Some(Some(v)),
            Some(None) => None,
            None => Some(None),
        }
    }
}

pub(crate) fn diff_scene_headers(before: &Scene, after: &Scene) -> Vec<SceneChange> {
    let mut changes = Vec::new();
    diff_entities("node", &before.nodes, &after.nodes, &mut changes);
    diff_entities(
        "metadata:node",
        &before.metadata.nodes,
        &after.metadata.nodes,
        &mut changes,
    );
    diff_entities(
        "metadata:mesh",
        &before.metadata.meshes,
        &after.metadata.meshes,
        &mut changes,
    );
    diff_entities(
        "metadata:material",
        &before.metadata.materials,
        &after.metadata.materials,
        &mut changes,
    );
    let fields = [
        ("id", before.id != after.id),
        ("name", before.name != after.name),
        ("meters_per_unit", before.meters_per_unit != after.meters_per_unit),
        ("up_axis", before.up_axis != after.up_axis),
        ("dependencies", before.dependencies != after.dependencies),
        ("provenance", before.provenance != after.provenance),
        (
            "metadata",
            before.metadata.scene != after.metadata.scene || before.metadata.schema != after.metadata.schema,
        ),
    ]
    .into_iter()
    .filter(|(_, changed)| *changed)
    .map(|(name, _)| name.to_owned())
    .collect::<Vec<_>>();
    if !fields.is_empty() {
        changes.push(SceneChange {
            entity: "scene".into(),
            change: "modified".into(),
            fields,
        });
    }
    changes
}

fn geometry_detail(report: GeometryReport) -> Result<String, SceneError> {
    serde_json::to_string(&report).map_err(|error| SceneError::new("geometry.report", "/", error.to_string()))
}
