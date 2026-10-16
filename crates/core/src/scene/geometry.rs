//! Explicit, deterministic geometry conversion. Operations preserve corner
//! attributes and face bindings; potentially lossy changes are opt-in.
use super::*;
use std::collections::{BTreeSet, HashMap};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct GeometryReport {
    pub operation: String,
    pub mesh: Option<String>,
    pub vertices_before: usize,
    pub vertices_after: usize,
    pub faces_before: usize,
    pub faces_after: usize,
    pub max_vertex_displacement: f64,
    pub notes: Vec<String>,
}
impl GeometryReport {
    fn mesh(operation: &str, id: &str, before: &Mesh, after: &Mesh) -> Self {
        Self {
            operation: operation.into(),
            mesh: Some(id.into()),
            vertices_before: before.positions.len(),
            vertices_after: after.positions.len(),
            faces_before: before.face_counts.len(),
            faces_after: after.face_counts.len(),
            max_vertex_displacement: 0.,
            notes: Vec::new(),
        }
    }
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct MeshRepairOptions {
    pub area_tolerance: f64,
    pub remove_invalid_index_faces: bool,
    pub max_faces: usize,
    pub max_corners: usize,
    pub max_face_corners: usize,
}
impl Default for MeshRepairOptions {
    fn default() -> Self {
        Self {
            area_tolerance: 0.,
            remove_invalid_index_faces: false,
            max_faces: 5_000_000,
            max_corners: 15_000_000,
            max_face_corners: 1024,
        }
    }
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct MeshRepairResult {
    pub mesh: Mesh,
    pub old_to_new_faces: Vec<Option<u32>>,
    pub removed_faces: Vec<u32>,
}

/// Standalone topology repair accepts bad point indices only when requested.
/// Ambiguous face-count streams, malformed attributes, non-finite coordinates,
/// self-intersections and nonplanarity are errors, never guessed repairs.
pub fn repair_mesh(mesh: &Mesh, options: &MeshRepairOptions) -> Result<MeshRepairResult, SceneError> {
    if !options.area_tolerance.is_finite() || options.area_tolerance < 0. {
        return Err(SceneError::new(
            "geometry.tolerance",
            "/",
            "area tolerance must be finite and nonnegative",
        ));
    }
    if mesh.face_counts.len() > options.max_faces
        || mesh.face_indices.len() > options.max_corners
        || mesh.face_counts.iter().any(|n| *n as usize > options.max_face_corners)
    {
        return Err(SceneError::new(
            "geometry.limit",
            "/",
            "repair topology budget exceeded",
        ));
    }
    let counts = mesh
        .face_counts
        .iter()
        .try_fold(0usize, |sum, n| sum.checked_add(*n as usize));
    if counts != Some(mesh.face_indices.len()) {
        return Err(SceneError::new(
            "mesh.topology",
            "/",
            "cannot infer faces from inconsistent corner counts",
        ));
    }
    if mesh.positions.iter().flatten().any(|v| !v.is_finite()) {
        return Err(SceneError::new(
            "mesh.non_finite",
            "/",
            "repair requires finite coordinates",
        ));
    }
    let attributes_valid = mesh
        .normals
        .as_ref()
        .is_none_or(|a| validation::attribute_valid(a, mesh) && a.values.iter().all(|v| normalize(*v).is_some()))
        && mesh.tangents.as_ref().is_none_or(|a| {
            validation::attribute_valid(a, mesh)
                && a.values.iter().all(|v| {
                    normalize([f64::from(v[0]), f64::from(v[1]), f64::from(v[2])]).is_some()
                        && (v[3] == 1. || v[3] == -1.)
                })
        })
        && mesh.colors.as_ref().is_none_or(|a| {
            validation::attribute_valid(a, mesh)
                && a.values
                    .iter()
                    .flatten()
                    .all(|v| v.is_finite() && (0. ..=1.).contains(v))
        })
        && mesh.uv_sets.iter().all(|(name, a)| {
            !name.is_empty() && validation::attribute_valid(a, mesh) && a.values.iter().flatten().all(|v| v.is_finite())
        });
    if !attributes_valid {
        return Err(SceneError::new(
            "mesh.attribute",
            "/",
            "repair requires valid source attribute counts and values",
        ));
    }
    let mut keep = Vec::new();
    let mut removed_faces = Vec::new();
    let mut old_to_new_faces = vec![None; mesh.face_counts.len()];
    let mut corner = 0;
    for (face, count) in mesh.face_counts.iter().enumerate() {
        let ids = &mesh.face_indices[corner..corner + *count as usize];
        let invalid = ids.iter().any(|id| *id as usize >= mesh.positions.len());
        if invalid && !options.remove_invalid_index_faces {
            return Err(SceneError::new(
                "mesh.index",
                face.to_string(),
                "out-of-range face index; explicit invalid-index removal is disabled",
            ));
        }
        let repeated = ids.iter().collect::<BTreeSet<_>>().len() != ids.len();
        let area = if invalid || repeated || ids.len() < 3 {
            0.
        } else {
            polygon_area(mesh, ids)?
        };
        if invalid || repeated || ids.len() < 3 || area <= options.area_tolerance {
            removed_faces.push(face as u32);
        } else {
            old_to_new_faces[face] = Some(keep.len() as u32);
            keep.push(face);
        }
        corner += *count as usize;
    }
    if keep.is_empty() {
        return Err(SceneError::new("geometry.empty", "/", "repair would remove every face"));
    }
    let mesh = select_faces(mesh, &keep)?;
    Ok(MeshRepairResult {
        mesh,
        old_to_new_faces,
        removed_faces,
    })
}
fn polygon_area(mesh: &Mesh, ids: &[u32]) -> Result<f64, SceneError> {
    let points: Vec<_> = ids.iter().map(|id| mesh.positions[*id as usize]).collect();
    // Collinear polygons are degenerate; reportable repair rather than parse failure.
    let origin = points[0];
    let mut sum = [0.; 3];
    for i in 1..points.len() - 1 {
        let cross = cross(sub(points[i], origin), sub(points[i + 1], origin));
        for j in 0..3 {
            sum[j] += cross[j];
        }
    }
    let area = length(sum) * 0.5;
    if area == 0.
        && (1..points.len() - 1).all(|i| length(cross(sub(points[i], origin), sub(points[i + 1], origin))) == 0.)
    {
        return Ok(0.);
    }
    triangulate(&points)?;
    Ok(area)
}
fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    std::array::from_fn(|i| a[i] - b[i])
}
fn length(a: [f64; 3]) -> f64 {
    a.iter().map(|v| v * v).sum::<f64>().sqrt()
}
fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    (0..3).map(|i| a[i] * b[i]).sum()
}
fn vector(matrix: Matrix4, v: [f64; 3]) -> [f64; 3] {
    std::array::from_fn(|r| (0..3).map(|c| matrix[r][c] * v[c]).sum())
}

pub(crate) fn bake_transform(scene: &mut Scene, id: &str) -> Result<GeometryReport, SceneError> {
    let node = scene
        .nodes
        .get(id)
        .ok_or_else(|| SceneError::new("scene.missing_node", id, "bake target not found"))?
        .clone();
    let matrix = node.transform;
    let mut report = GeometryReport {
        operation: "bake_transform".into(),
        mesh: node.mesh.clone(),
        vertices_before: 0,
        vertices_after: 0,
        faces_before: 0,
        faces_after: 0,
        max_vertex_displacement: 0.,
        notes: vec![
            "Local affine transform is baked into geometry; child placement and reset stacks are preserved.".into(),
        ],
    };
    if let Some(mesh_id) = &node.mesh {
        let inverse = inverse(matrix)
            .ok_or_else(|| SceneError::new("geometry.singular", id, "cannot bake singular geometry transform"))?;
        let old = scene
            .meshes
            .get(mesh_id)
            .ok_or_else(|| SceneError::new("scene.missing_mesh", mesh_id, "bake mesh unresolved"))?;
        let mut mesh = old.clone();
        for p in &mut mesh.positions {
            *p = transform_point(matrix, *p);
        }
        if let Some(normals) = &mut mesh.normals {
            for normal in &mut normals.values {
                *normal = normalize(std::array::from_fn(|r| (0..3).map(|c| inverse[c][r] * normal[c]).sum()))
                    .ok_or_else(|| SceneError::new("geometry.normal", id, "normal transform became degenerate"))?;
            }
        }
        let mirrored = determinant(matrix) < 0.;
        if let Some(tangents) = &mut mesh.tangents {
            for tangent in &mut tangents.values {
                let t = normalize(vector(
                    matrix,
                    [f64::from(tangent[0]), f64::from(tangent[1]), f64::from(tangent[2])],
                ))
                .ok_or_else(|| SceneError::new("geometry.tangent", id, "tangent transform became degenerate"))?;
                for i in 0..3 {
                    tangent[i] = t[i] as f32;
                }
                if mirrored {
                    tangent[3] = -tangent[3];
                }
            }
        }
        if mirrored {
            mesh.orientation = match mesh.orientation {
                Orientation::RightHanded => Orientation::LeftHanded,
                Orientation::LeftHanded => Orientation::RightHanded,
            };
        }
        report = GeometryReport::mesh("bake_transform", mesh_id, old, &mesh);
        report.notes.push("Affine baking transforms normals by inverse transpose, tangent direction by the linear transform, and flips orientation/handedness for mirrors.".into());
        let shared = scene
            .nodes
            .iter()
            .any(|(other, node)| other != id && node.mesh.as_ref() == Some(mesh_id));
        let target = if shared {
            let target = unique_mesh_id(scene, &format!("{mesh_id}:baked:{id}"));
            if let Some(metadata) = scene.metadata.meshes.get(mesh_id).cloned() {
                scene.metadata.meshes.insert(target.clone(), metadata);
            }
            report
                .notes
                .push(format!("Shared source mesh retained; this occurrence uses {target}."));
            target
        } else {
            mesh_id.clone()
        };
        scene.meshes.insert(target.clone(), mesh);
        scene.nodes.get_mut(id).expect("target exists").mesh = Some(target.clone());
        report.mesh = Some(target);
    }
    scene.nodes.get_mut(id).expect("target exists").transform = IDENTITY;
    for node in scene.nodes.values_mut() {
        if node.parent.as_deref() == Some(id) && !node.reset_transform {
            node.transform = multiply(matrix, node.transform);
        }
    }
    Ok(report)
}
fn unique_mesh_id(scene: &Scene, base: &str) -> String {
    let mut candidate = base.to_owned();
    let mut suffix = 1;
    while scene.meshes.contains_key(&candidate) {
        candidate = format!("{base}:{suffix}");
        suffix += 1;
    }
    candidate
}

pub(crate) fn generate_normals(
    scene: &mut Scene,
    id: &str,
    crease_degrees: f64,
    area_weighted: bool,
    limits: &SceneLimits,
) -> Result<GeometryReport, SceneError> {
    if !crease_degrees.is_finite() || !(0. ..=180.).contains(&crease_degrees) {
        return Err(SceneError::new(
            "geometry.angle",
            id,
            "crease angle must be between 0 and 180 degrees",
        ));
    }
    let mesh = scene
        .meshes
        .get(id)
        .ok_or_else(|| SceneError::new("scene.missing_mesh", id, "normal generation mesh not found"))?;
    let before_points = mesh.positions.len();
    let before_faces = mesh.face_counts.len();
    let regions = face_regions(scene, id, limits)?;
    let mut face_normals = Vec::new();
    let mut adjacency = vec![Vec::new(); mesh.positions.len()];
    let mut offset = 0;
    for (face, count) in mesh.face_counts.iter().enumerate() {
        let ids = &mesh.face_indices[offset..offset + *count as usize];
        let points: Vec<_> = ids.iter().map(|id| mesh.positions[*id as usize]).collect();
        let triangles = triangulate(&points)?;
        let mut normal = [0.; 3];
        for triangle in triangles {
            let [a, b, c] = triangle.map(|i| points[i]);
            let area = cross(sub(b, a), sub(c, a));
            for i in 0..3 {
                normal[i] += area[i];
            }
        }
        if mesh.orientation == Orientation::LeftHanded {
            normal = normal.map(|v| -v);
        }
        let normalized = normalize(normal).ok_or_else(|| {
            SceneError::new(
                "geometry.degenerate",
                id,
                "normal generation encountered degenerate face",
            )
        })?;
        face_normals.push((normal, normalized));
        for vertex in ids {
            adjacency[*vertex as usize].push(face);
        }
        offset += *count as usize;
    }
    let cosine = crease_degrees.to_radians().cos();
    let mut values = Vec::with_capacity(mesh.face_indices.len());
    let mut work = 0usize;
    offset = 0;
    for (face, count) in mesh.face_counts.iter().enumerate() {
        for vertex in &mesh.face_indices[offset..offset + *count as usize] {
            let mut normal = [0.; 3];
            for &neighbor in &adjacency[*vertex as usize] {
                work = work.saturating_add(1);
                if work > limits.max_corners.saturating_mul(32) {
                    return Err(SceneError::new(
                        "geometry.work_limit",
                        id,
                        "normal adjacency work exceeds budget",
                    ));
                }
                if regions[face] == regions[neighbor]
                    && dot(face_normals[face].1, face_normals[neighbor].1) >= cosine - 1e-12
                {
                    let n = if area_weighted {
                        face_normals[neighbor].0
                    } else {
                        face_normals[neighbor].1
                    };
                    for i in 0..3 {
                        normal[i] += n[i];
                    }
                }
            }
            values.push(normalize(normal).unwrap_or(face_normals[face].1));
        }
        offset += *count as usize;
    }
    let mesh = scene.meshes.get_mut(id).expect("mesh exists");
    mesh.normals = Some(Attribute {
        interpolation: Interpolation::FaceVarying,
        values,
        indices: None,
    });
    mesh.tangents = None;
    Ok(GeometryReport {
        operation: "generate_normals".into(),
        mesh: Some(id.into()),
        vertices_before: before_points,
        vertices_after: before_points,
        faces_before: before_faces,
        faces_after: before_faces,
        max_vertex_displacement: 0.,
        notes: vec![
            format!(
                "Face-corner normals use {crease_degrees} degree smoothing with material boundaries retained; area_weighted={area_weighted}."
            ),
            "Existing tangents cleared because their source normals changed; exporters regenerate them when required."
                .into(),
        ],
    })
}
fn face_regions(scene: &Scene, id: &str, limits: &SceneLimits) -> Result<Vec<Vec<(String, String)>>, SceneError> {
    let mesh = scene
        .meshes
        .get(id)
        .ok_or_else(|| SceneError::new("scene.missing_mesh", id, "mesh not found"))?;
    face_regions_for_nodes(
        scene,
        id,
        mesh,
        scene.nodes.iter().filter(|(_, node)| node.mesh.as_deref() == Some(id)),
        limits,
    )
}
fn face_regions_for_nodes<'a>(
    scene: &Scene,
    id: &str,
    mesh: &Mesh,
    nodes: impl Iterator<Item = (&'a String, &'a SceneNode)>,
    limits: &SceneLimits,
) -> Result<Vec<Vec<(String, String)>>, SceneError> {
    let mut regions = vec![Vec::new(); mesh.face_counts.len()];
    let mut work = 0usize;
    for (node_id, node) in nodes {
        let base = effective_material(scene, node_id)?;
        for subset in &node.subsets {
            if base.as_ref() == Some(&subset.material) {
                continue;
            }
            for face in &subset.faces {
                work = work.saturating_add(1);
                if work > limits.max_corners {
                    return Err(SceneError::new(
                        "geometry.work_limit",
                        id,
                        "material boundary work exceeds budget",
                    ));
                }
                let region = regions
                    .get_mut(*face as usize)
                    .ok_or_else(|| SceneError::new("binding.face", node_id, "face subset index out of range"))?;
                region.push((node_id.clone(), subset.material.0.clone()));
            }
        }
    }
    Ok(regions)
}

pub(crate) fn remove_degenerate(
    scene: &mut Scene,
    id: &str,
    tolerance: f64,
    limits: &SceneLimits,
) -> Result<GeometryReport, SceneError> {
    let old = scene
        .meshes
        .get(id)
        .ok_or_else(|| SceneError::new("scene.missing_mesh", id, "repair mesh not found"))?;
    let repaired = repair_mesh(
        old,
        &MeshRepairOptions {
            area_tolerance: tolerance,
            max_faces: limits.max_faces,
            max_corners: limits.max_corners,
            max_face_corners: limits.max_face_corners,
            ..Default::default()
        },
    )?;
    let mut report = GeometryReport::mesh("remove_degenerate_faces", id, old, &repaired.mesh);
    report.notes.push(format!("Removed {} faces with repeated points, collinearity or area <= {tolerance}; all corner attributes and material subset indices remapped.",repaired.removed_faces.len()));
    remap_bindings(scene, id, &repaired.old_to_new_faces);
    scene.meshes.insert(id.into(), repaired.mesh);
    Ok(report)
}
fn remap_bindings(scene: &mut Scene, id: &str, map: &[Option<u32>]) {
    for node in scene.nodes.values_mut() {
        if node.mesh.as_deref() == Some(id) {
            for subset in &mut node.subsets {
                subset.faces = subset
                    .faces
                    .iter()
                    .filter_map(|f| map.get(*f as usize).copied().flatten())
                    .collect();
            }
            node.subsets.retain(|subset| !subset.faces.is_empty());
        }
    }
}

pub(crate) fn weld(
    scene: &mut Scene,
    id: &str,
    tolerance: f64,
    limits: &SceneLimits,
) -> Result<GeometryReport, SceneError> {
    if !tolerance.is_finite() || tolerance <= 0. {
        return Err(SceneError::new(
            "geometry.tolerance",
            id,
            "weld tolerance must be finite and positive",
        ));
    }
    let old = scene
        .meshes
        .get(id)
        .ok_or_else(|| SceneError::new("scene.missing_mesh", id, "weld mesh not found"))?;
    let mut mesh = old.clone();
    let mut buckets: HashMap<[i64; 3], Vec<usize>> = HashMap::new();
    let mut remap = vec![0; mesh.positions.len()];
    let mut representatives = Vec::new();
    let mut displacement = 0f64;
    let mut work = 0usize;
    for (vertex, p) in mesh.positions.iter().enumerate() {
        let mut key = [0; 3];
        for i in 0..3 {
            let cell = (p[i] / tolerance).floor();
            if !cell.is_finite() || cell <= i64::MIN as f64 + 2. || cell >= i64::MAX as f64 - 2. {
                return Err(SceneError::new(
                    "geometry.precision",
                    id,
                    "weld grid exceeds integer precision range; shift coordinates or increase tolerance",
                ));
            }
            key[i] = cell as i64;
        }
        let mut chosen = None;
        let mut candidate_vertex = usize::MAX;
        for x in -1..=1 {
            for y in -1..=1 {
                for z in -1..=1 {
                    if let Some(candidates) = buckets.get(&[key[0] + x, key[1] + y, key[2] + z]) {
                        for &candidate in candidates {
                            work = work.saturating_add(1);
                            if work > limits.max_corners.saturating_mul(32) {
                                return Err(SceneError::new(
                                    "geometry.work_limit",
                                    id,
                                    "weld candidate work exceeds budget",
                                ));
                            }
                            let distance = length(sub(*p, mesh.positions[candidate]));
                            if distance <= tolerance
                                && candidate < candidate_vertex
                                && vertex_attributes_equal(&mesh, vertex, candidate)
                            {
                                chosen = Some((remap[candidate], distance));
                                candidate_vertex = candidate;
                            }
                        }
                    }
                }
            }
        }
        if let Some((target, distance)) = chosen {
            remap[vertex] = target;
            displacement = displacement.max(distance);
        } else {
            remap[vertex] = representatives.len() as u32;
            representatives.push(vertex);
            buckets.entry(key).or_default().push(vertex);
        }
    }
    remap_vertices(&mut mesh, &remap, &representatives);
    let repaired = repair_mesh(
        &mesh,
        &MeshRepairOptions {
            max_faces: limits.max_faces,
            max_corners: limits.max_corners,
            max_face_corners: limits.max_face_corners,
            ..Default::default()
        },
    )?;
    let mut report = GeometryReport::mesh("weld_vertices", id, old, &repaired.mesh);
    report.max_vertex_displacement = displacement;
    report.notes.push("Position clusters retain the earliest compatible vertex. Vertex/varying attributes must match exactly; face-corner seams are retained. Degenerate faces and bindings are remapped.".into());
    remap_bindings(scene, id, &repaired.old_to_new_faces);
    scene.meshes.insert(id.into(), repaired.mesh);
    Ok(report)
}
fn vertex_attributes_equal(mesh: &Mesh, a: usize, b: usize) -> bool {
    fn same<T: PartialEq>(attribute: &Attribute<T>, a: usize, b: usize) -> bool {
        !matches!(attribute.interpolation, Interpolation::Vertex | Interpolation::Varying)
            || attribute.sample(0, 0, a) == attribute.sample(0, 0, b)
    }
    mesh.normals.as_ref().is_none_or(|v| same(v, a, b))
        && mesh.tangents.as_ref().is_none_or(|v| same(v, a, b))
        && mesh.colors.as_ref().is_none_or(|v| same(v, a, b))
        && mesh.uv_sets.values().all(|v| same(v, a, b))
}
fn remap_vertices(mesh: &mut Mesh, map: &[u32], representatives: &[usize]) {
    fn remap<T: Clone>(attribute: &mut Attribute<T>, representatives: &[usize]) {
        if matches!(attribute.interpolation, Interpolation::Vertex | Interpolation::Varying) {
            attribute.values = representatives
                .iter()
                .map(|v| {
                    attribute
                        .sample(0, 0, *v)
                        .expect("valid source vertex attribute")
                        .clone()
                })
                .collect();
            attribute.indices = None;
        }
    }
    for index in &mut mesh.face_indices {
        *index = map[*index as usize];
    }
    mesh.positions = representatives.iter().map(|i| mesh.positions[*i]).collect();
    if let Some(a) = &mut mesh.normals {
        remap(a, representatives);
    }
    if let Some(a) = &mut mesh.tangents {
        remap(a, representatives);
    }
    if let Some(a) = &mut mesh.colors {
        remap(a, representatives);
    }
    for a in mesh.uv_sets.values_mut() {
        remap(a, representatives);
    }
}
fn select_faces(mesh: &Mesh, faces: &[usize]) -> Result<Mesh, SceneError> {
    let mut result = mesh.clone();
    let mut offsets = Vec::with_capacity(mesh.face_counts.len() + 1);
    offsets.push(0);
    for count in &mesh.face_counts {
        offsets.push(offsets.last().copied().unwrap_or(0) + *count as usize);
    }
    result.face_counts = faces.iter().map(|f| mesh.face_counts[*f]).collect();
    result.face_indices = faces
        .iter()
        .flat_map(|face| mesh.face_indices[offsets[*face]..offsets[*face + 1]].iter().copied())
        .collect();
    fn select<T: Clone>(
        attribute: &Attribute<T>,
        mesh: &Mesh,
        faces: &[usize],
        offsets: &[usize],
    ) -> Result<Attribute<T>, SceneError> {
        if !matches!(
            attribute.interpolation,
            Interpolation::Uniform | Interpolation::FaceVarying
        ) {
            return Ok(attribute.clone());
        }
        let mut values = Vec::new();
        for face in faces {
            let range = if attribute.interpolation == Interpolation::Uniform {
                offsets[*face]..offsets[*face] + 1
            } else {
                offsets[*face]..offsets[*face + 1]
            };
            for corner in range {
                let vertex = mesh.face_indices[corner] as usize;
                values.push(
                    attribute
                        .sample(*face, corner, vertex)
                        .ok_or_else(|| SceneError::new("mesh.attribute", "/", "cannot remap malformed face attribute"))?
                        .clone(),
                );
            }
        }
        Ok(Attribute {
            interpolation: attribute.interpolation,
            values,
            indices: None,
        })
    }
    result.normals = mesh
        .normals
        .as_ref()
        .map(|a| select(a, mesh, faces, &offsets))
        .transpose()?;
    result.tangents = mesh
        .tangents
        .as_ref()
        .map(|a| select(a, mesh, faces, &offsets))
        .transpose()?;
    result.colors = mesh
        .colors
        .as_ref()
        .map(|a| select(a, mesh, faces, &offsets))
        .transpose()?;
    result.uv_sets = mesh
        .uv_sets
        .iter()
        .map(|(key, a)| Ok((key.clone(), select(a, mesh, faces, &offsets)?)))
        .collect::<Result<_, SceneError>>()?;
    Ok(result)
}

pub(crate) fn triangulate_mesh(
    scene: &mut Scene,
    id: &str,
    limits: &SceneLimits,
) -> Result<GeometryReport, SceneError> {
    let old = scene
        .meshes
        .get(id)
        .ok_or_else(|| SceneError::new("scene.missing_mesh", id, "triangulation mesh not found"))?;
    check_triangulation_budget(old, limits)?;
    let (mesh, sources) = triangle_mesh(old)?;
    let mut report = GeometryReport::mesh("triangulate_mesh", id, old, &mesh);
    report.notes.push("Deterministic ear clipping of simple planar polygons; all face/corner attributes and material subset assignments remapped.".into());
    remap_bindings_sources(scene, id, &sources);
    scene.meshes.insert(id.into(), mesh);
    Ok(report)
}
fn triangle_mesh(mesh: &Mesh) -> Result<(Mesh, Vec<usize>), SceneError> {
    let mut corners = Vec::new();
    let mut sources = Vec::new();
    let mut offset = 0;
    for (face, count) in mesh.face_counts.iter().enumerate() {
        let points = mesh.face_indices[offset..offset + *count as usize]
            .iter()
            .map(|id| mesh.positions[*id as usize])
            .collect::<Vec<_>>();
        for triangle in triangulate(&points)? {
            corners.extend(triangle.map(|index| offset + index));
            sources.push(face);
        }
        offset += *count as usize;
    }
    let mut output = mesh.clone();
    output.face_counts = vec![3; sources.len()];
    output.face_indices = corners.iter().map(|corner| mesh.face_indices[*corner]).collect();
    fn attr<T: Clone>(
        a: &Attribute<T>,
        mesh: &Mesh,
        corners: &[usize],
        sources: &[usize],
    ) -> Result<Attribute<T>, SceneError> {
        if !matches!(a.interpolation, Interpolation::Uniform | Interpolation::FaceVarying) {
            return Ok(a.clone());
        }
        let count = if a.interpolation == Interpolation::Uniform {
            sources.len()
        } else {
            corners.len()
        };
        let mut values = Vec::with_capacity(count);
        for i in 0..count {
            let (face, corner) = if a.interpolation == Interpolation::Uniform {
                (sources[i], corners[i * 3])
            } else {
                (sources[i / 3], corners[i])
            };
            values.push(
                a.sample(face, corner, mesh.face_indices[corner] as usize)
                    .ok_or_else(|| SceneError::new("mesh.attribute", "/", "invalid triangulation attribute"))?
                    .clone(),
            );
        }
        Ok(Attribute {
            interpolation: a.interpolation,
            values,
            indices: None,
        })
    }
    output.normals = mesh
        .normals
        .as_ref()
        .map(|a| attr(a, mesh, &corners, &sources))
        .transpose()?;
    output.tangents = mesh
        .tangents
        .as_ref()
        .map(|a| attr(a, mesh, &corners, &sources))
        .transpose()?;
    output.colors = mesh
        .colors
        .as_ref()
        .map(|a| attr(a, mesh, &corners, &sources))
        .transpose()?;
    output.uv_sets = mesh
        .uv_sets
        .iter()
        .map(|(k, a)| Ok((k.clone(), attr(a, mesh, &corners, &sources)?)))
        .collect::<Result<_, SceneError>>()?;
    Ok((output, sources))
}
fn remap_bindings_sources(scene: &mut Scene, id: &str, sources: &[usize]) {
    let mut map: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    for (new, old) in sources.iter().enumerate() {
        map.entry(*old as u32).or_default().push(new as u32);
    }
    for node in scene.nodes.values_mut() {
        if node.mesh.as_deref() == Some(id) {
            for subset in &mut node.subsets {
                subset.faces = subset
                    .faces
                    .iter()
                    .flat_map(|f| map.get(f).into_iter().flatten().copied())
                    .collect();
            }
            node.subsets.retain(|s| !s.faces.is_empty());
        }
    }
}

/// A bounded deterministic edge-collapse LOD. It preserves open/material/attribute
/// seam vertices and checks the manifold link condition, local winding and duplicate
/// triangles. Error is maximum ORIGINAL VERTEX displacement, not a Hausdorff or
/// texture error promise. An unreachable target yields a useful partial reduction.
pub(crate) fn simplify(
    scene: &mut Scene,
    id: &str,
    target_faces: usize,
    max_displacement: f64,
    limits: &SceneLimits,
) -> Result<GeometryReport, SceneError> {
    if target_faces == 0 || !max_displacement.is_finite() || max_displacement < 0. {
        return Err(SceneError::new(
            "geometry.simplify_options",
            id,
            "positive face target and finite nonnegative displacement required",
        ));
    }
    let old = scene
        .meshes
        .get(id)
        .ok_or_else(|| SceneError::new("scene.missing_mesh", id, "simplification mesh not found"))?;
    let regions = face_regions(scene, id, limits)?;
    let simplified = simplify_geometry(old, id, &regions, target_faces, max_displacement, limits)?;
    remap_bindings_sources(scene, id, &simplified.face_sources);
    scene.meshes.insert(id.into(), simplified.mesh);
    Ok(simplified.report)
}
struct SimplifiedGeometry {
    mesh: Mesh,
    face_sources: Vec<usize>,
    report: GeometryReport,
}
fn simplify_geometry(
    old: &Mesh,
    id: &str,
    regions: &[Vec<(String, String)>],
    target_faces: usize,
    max_displacement: f64,
    limits: &SceneLimits,
) -> Result<SimplifiedGeometry, SceneError> {
    check_triangulation_budget(old, limits)?;
    let (source, face_sources) = triangle_mesh(old)?;
    if source.face_indices.len() > limits.max_corners || source.face_counts.len() > limits.max_faces {
        return Err(SceneError::new(
            "geometry.limit",
            id,
            "triangulated LOD exceeds geometry budget",
        ));
    }
    let scratch = source
        .positions
        .len()
        .saturating_mul(256)
        .saturating_add(source.face_counts.len().saturating_mul(512));
    if scratch > limits.max_bytes {
        return Err(SceneError::new(
            "geometry.scratch_limit",
            id,
            "LOD adjacency scratch estimate exceeds byte budget; use a smaller mesh or explicit larger budget",
        ));
    }
    let mut triangles = source
        .face_indices
        .as_chunks::<3>()
        .0
        .iter()
        .map(|v| Some([v[0] as usize, v[1] as usize, v[2] as usize]))
        .collect::<Vec<_>>();
    let mut incident = vec![BTreeSet::new(); source.positions.len()];
    let mut first_corner = vec![None; source.positions.len()];
    let mut protected = vec![false; source.positions.len()];
    let mut edges: BTreeMap<(usize, usize), Vec<usize>> = BTreeMap::new();
    for (face, triangle) in triangles.iter().enumerate() {
        let t = triangle.expect("all initial triangles");
        for corner in 0..3 {
            let vertex = t[corner];
            incident[vertex].insert(face);
            if let Some(first) = first_corner[vertex] {
                if !corner_attributes_equal(&source, first, face * 3 + corner) {
                    protected[vertex] = true;
                }
            } else {
                first_corner[vertex] = Some(face * 3 + corner);
            }
            let (a, b) = (vertex, t[(corner + 1) % 3]);
            edges.entry((a.min(b), a.max(b))).or_default().push(face);
        }
    }
    for (&(a, b), faces) in &edges {
        if faces.len() != 2 || regions[face_sources[faces[0]]] != regions[face_sources[faces[1]]] {
            protected[a] = true;
            protected[b] = true;
        }
    }
    let mut groups = (0..source.positions.len()).map(|id| vec![id]).collect::<Vec<_>>();
    let mut alive = triangles.len();
    let mut work = 0usize;
    let max_work = limits.max_corners.saturating_mul(64);
    let mut largest = 0f64;
    let mut limited = false;
    loop {
        if alive <= target_faces {
            break;
        }
        let mut current_edges = BTreeSet::new();
        for t in triangles.iter().flatten() {
            for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
                current_edges.insert((a.min(b), a.max(b)));
            }
        }
        work = work.saturating_add(triangles.len().saturating_mul(3));
        if work > max_work {
            limited = true;
            break;
        }
        let mut candidates = current_edges
            .into_iter()
            .filter(|(a, b)| !protected[*a] && !protected[*b])
            .collect::<Vec<_>>();
        candidates.sort_by(|(a, b), (c, d)| {
            length(sub(source.positions[*a], source.positions[*b]))
                .total_cmp(&length(sub(source.positions[*c], source.positions[*d])))
                .then((a, b).cmp(&(c, d)))
        });
        let mut progress = false;
        for (u, v) in candidates {
            if alive <= target_faces {
                break;
            }
            if groups[u].is_empty() || groups[v].is_empty() {
                continue;
            }
            let touching: Vec<_> = incident[u].intersection(&incident[v]).copied().collect();
            if touching.len() != 2 {
                continue;
            }
            work = work.saturating_add(groups[v].len() + incident[u].len() + incident[v].len());
            if work > max_work {
                limited = true;
                break;
            }
            let displacement = groups[v]
                .iter()
                .map(|original| length(sub(source.positions[*original], source.positions[u])))
                .fold(0f64, f64::max);
            if displacement > max_displacement {
                continue;
            }
            let neighbors = |vertex: usize| {
                incident[vertex]
                    .iter()
                    .filter_map(|f| triangles[*f])
                    .flatten()
                    .filter(|other| *other != vertex)
                    .collect::<BTreeSet<_>>()
            };
            let common = neighbors(u)
                .intersection(&neighbors(v))
                .copied()
                .collect::<BTreeSet<_>>();
            let opposite = touching
                .iter()
                .filter_map(|face| triangles[*face])
                .flatten()
                .filter(|other| *other != u && *other != v)
                .collect::<BTreeSet<_>>();
            if common != opposite || opposite.len() != 2 {
                continue;
            }
            let affected = incident[v].iter().copied().collect::<Vec<_>>();
            let mut replacements = Vec::new();
            let mut accepted = true;
            let mut new_faces = BTreeSet::new();
            for face in &affected {
                let before = triangles[*face].expect("incident faces alive");
                let after = before.map(|vertex| if vertex == v { u } else { vertex });
                if after.iter().collect::<BTreeSet<_>>().len() < 3 {
                    replacements.push((*face, None));
                    continue;
                }
                let a = before.map(|vertex| source.positions[vertex]);
                let b = after.map(|vertex| source.positions[vertex]);
                let na = cross(sub(a[1], a[0]), sub(a[2], a[0]));
                let nb = cross(sub(b[1], b[0]), sub(b[2], b[0]));
                if normalize(nb).is_none() || dot(na, nb) <= 0. {
                    accepted = false;
                    break;
                }
                let mut key = after;
                key.sort_unstable();
                if !new_faces.insert(key) {
                    accepted = false;
                    break;
                }
                for existing in &incident[u] {
                    work = work.saturating_add(1);
                    if work > max_work {
                        limited = true;
                        accepted = false;
                        break;
                    }
                    if affected.contains(existing) {
                        continue;
                    }
                    if let Some(mut t) = triangles[*existing] {
                        t.sort_unstable();
                        if key == t {
                            accepted = false;
                            break;
                        }
                    }
                }
                if !accepted {
                    break;
                }
                replacements.push((*face, Some(after)));
            }
            if !accepted {
                continue;
            }
            for (face, after) in replacements {
                let before = triangles[face].expect("alive");
                for vertex in before {
                    incident[vertex].remove(&face);
                }
                if let Some(after) = after {
                    for vertex in after {
                        incident[vertex].insert(face);
                    }
                } else {
                    alive -= 1;
                }
                triangles[face] = after;
            }
            let moved = std::mem::take(&mut groups[v]);
            groups[u].extend(moved);
            largest = largest.max(displacement);
            progress = true;
        }
        if !progress || limited {
            break;
        }
    }
    let keep = triangles
        .iter()
        .enumerate()
        .filter_map(|(face, t)| t.map(|_| face))
        .collect::<Vec<_>>();
    let mut output = select_faces(&source, &keep)?;
    output.face_indices = triangles.iter().flatten().flat_map(|t| t.map(|v| v as u32)).collect();
    // Preserve UV/normal continuity inside a chart by choosing the representative's
    // value for each moved corner. Seam vertices were locked before collapsing.
    fn reproject<T: Clone>(
        target: &mut Attribute<T>,
        source_attr: &Attribute<T>,
        source: &Mesh,
        indices: &[u32],
        first: &[Option<usize>],
        keep: &[usize],
    ) -> Result<(), SceneError> {
        if target.interpolation != Interpolation::FaceVarying {
            return Ok(());
        }
        target.values = indices
            .iter()
            .enumerate()
            .map(|(index, vertex)| {
                let original_corner = keep[index / 3] * 3 + index % 3;
                let selected = if source.face_indices[original_corner] == *vertex {
                    Some(original_corner)
                } else {
                    first[*vertex as usize]
                };
                let corner = selected
                    .ok_or_else(|| SceneError::new("geometry.attribute", "/", "missing representative corner"))?;
                source_attr
                    .sample(corner / 3, corner, source.face_indices[corner] as usize)
                    .cloned()
                    .ok_or_else(|| SceneError::new("geometry.attribute", "/", "missing attribute value"))
            })
            .collect::<Result<_, _>>()?;
        target.indices = None;
        Ok(())
    }
    if let (Some(target), Some(a)) = (&mut output.normals, &source.normals) {
        reproject(target, a, &source, &output.face_indices, &first_corner, &keep)?;
    }
    if let (Some(target), Some(a)) = (&mut output.colors, &source.colors) {
        reproject(target, a, &source, &output.face_indices, &first_corner, &keep)?;
    }
    for (key, target) in &mut output.uv_sets {
        reproject(
            target,
            &source.uv_sets[key],
            &source,
            &output.face_indices,
            &first_corner,
            &keep,
        )?;
    }
    output.tangents = None;
    let used = output
        .face_indices
        .iter()
        .map(|v| *v as usize)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let mut map = vec![0; output.positions.len()];
    for (new, old) in used.iter().enumerate() {
        map[*old] = new as u32;
    }
    remap_vertices(&mut output, &map, &used);
    let mut report = GeometryReport::mesh("simplify_mesh", id, old, &output);
    report.max_vertex_displacement = largest;
    report.notes.push(format!(
        "Target={target_faces}; reached={}; work_limited={limited}; displacement is measured in scene units.",
        alive <= target_faces
    ));
    report.notes.push("Conservative interior edge collapse locks open edges, material boundaries and corner-attribute seams. Error bound is maximum original vertex displacement, not surface Hausdorff distance, UV distortion, or a global intersection guarantee. Tangents are cleared for regeneration.".into());
    let final_sources = keep.iter().map(|face| face_sources[*face]).collect::<Vec<_>>();
    Ok(SimplifiedGeometry {
        mesh: output,
        face_sources: final_sources,
        report,
    })
}
fn corner_attributes_equal(mesh: &Mesh, a: usize, b: usize) -> bool {
    fn same<T: PartialEq>(attr: &Attribute<T>, mesh: &Mesh, a: usize, b: usize) -> bool {
        attr.sample(a / 3, a, mesh.face_indices[a] as usize) == attr.sample(b / 3, b, mesh.face_indices[b] as usize)
    }
    mesh.normals.as_ref().is_none_or(|v| same(v, mesh, a, b))
        && mesh.tangents.as_ref().is_none_or(|v| same(v, mesh, a, b))
        && mesh.colors.as_ref().is_none_or(|v| same(v, mesh, a, b))
        && mesh.uv_sets.values().all(|v| same(v, mesh, a, b))
}

fn check_triangulation_budget(mesh: &Mesh, limits: &SceneLimits) -> Result<(), SceneError> {
    let triangles = mesh.face_counts.iter().fold(0usize, |total, count| {
        total.saturating_add((*count as usize).saturating_sub(2))
    });
    let corners = triangles.saturating_mul(3);
    if triangles > limits.max_faces || corners > limits.max_corners {
        return Err(SceneError::new(
            "geometry.limit",
            "/",
            "triangulation would exceed face/corner limits",
        ));
    }
    if triangulated_mesh_bytes(mesh, triangles) > limits.max_bytes {
        return Err(SceneError::new(
            "geometry.limit",
            "/",
            "triangulated attribute output exceeds byte limit",
        ));
    }
    Ok(())
}
fn triangulated_mesh_bytes(mesh: &Mesh, triangles: usize) -> usize {
    let corners = triangles.saturating_mul(3);
    fn bytes<T>(a: &Attribute<T>, triangles: usize, corners: usize) -> usize {
        let values = match a.interpolation {
            Interpolation::Uniform => triangles,
            Interpolation::FaceVarying => corners,
            _ => a.values.len(),
        };
        values
            .saturating_mul(std::mem::size_of::<T>())
            .saturating_add(a.indices.as_ref().map_or(0, |v| v.len().saturating_mul(4)))
    }
    let mut size = mesh
        .positions
        .len()
        .saturating_mul(24)
        .saturating_add(triangles.saturating_mul(4))
        .saturating_add(corners.saturating_mul(4));
    if let Some(a) = &mesh.normals {
        size = size.saturating_add(bytes(a, triangles, corners));
    }
    if let Some(a) = &mesh.tangents {
        size = size.saturating_add(bytes(a, triangles, corners));
    }
    if let Some(a) = &mesh.colors {
        size = size.saturating_add(bytes(a, triangles, corners));
    }
    for a in mesh.uv_sets.values() {
        size = size.saturating_add(bytes(a, triangles, corners));
    }
    size
}

/// Independent derivatives of one immutable source. Ratios apply to the source's
/// triangulated face count, never to an earlier generated level.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SceneLodRequest {
    pub name: String,
    pub face_ratio: f64,
    /// Scene-coordinate units. This is an original-vertex displacement bound,
    /// not a surface/Hausdorff, shading, or UV-distortion bound.
    pub max_vertex_displacement: f64,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct SceneLodOptions {
    pub levels: Vec<SceneLodRequest>,
    pub max_levels: usize,
    /// Conservative retained geometry, face bindings, and report estimate across
    /// all requested levels. Shared source materials/images are excluded.
    pub max_output_bytes: usize,
    /// Conservative triangulated source-face upper bound across all levels.
    pub max_output_faces: usize,
    /// Total triangulated source corners processed across all requested levels.
    pub max_source_corners: usize,
}
impl Default for SceneLodOptions {
    fn default() -> Self {
        Self {
            levels: Vec::new(),
            max_levels: 8,
            max_output_bytes: 256 * 1024 * 1024,
            max_output_faces: 5_000_000,
            max_source_corners: 15_000_000,
        }
    }
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct MeshLodReport {
    pub mesh: String,
    pub target_faces: usize,
    pub target_reached: bool,
    pub geometry: GeometryReport,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SceneLodLevelReport {
    pub name: String,
    pub face_ratio: f64,
    pub displacement_limit: f64,
    pub target_reached: bool,
    pub meshes: Vec<MeshLodReport>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SceneLodManifest {
    pub schema: String,
    pub source_scene_id: String,
    /// SHA-256 of the complete, serialized neutral source scene, including edits.
    pub source_scene_sha256: String,
    /// Original source-file hash when available; distinct from neutral content.
    pub source_sha256: Option<String>,
    pub meters_per_unit: f64,
    pub error_policy: String,
    pub levels: Vec<SceneLodLevelReport>,
}
#[derive(Debug, Default)]
struct SceneLodGeometry {
    meshes: BTreeMap<String, Mesh>,
    subsets: BTreeMap<String, Vec<FaceBinding>>,
}
/// Geometry and remapped bindings are owned; hierarchy, metadata, materials and
/// encoded images stay borrowed from the immutable source until materialization.
#[derive(Debug)]
pub struct SceneLodSet<'a> {
    source: &'a Scene,
    manifest: SceneLodManifest,
    geometry: Vec<SceneLodGeometry>,
    limits: SceneLimits,
    max_output_bytes: usize,
}
#[derive(Clone, Debug)]
pub struct SceneLodExport {
    pub level: SceneLodLevelReport,
    pub export: SceneExport,
}
#[derive(Clone, Debug)]
pub struct SceneLodExports {
    pub manifest: SceneLodManifest,
    pub levels: Vec<SceneLodExport>,
}
impl SceneLodSet<'_> {
    pub fn manifest(&self) -> &SceneLodManifest {
        &self.manifest
    }
    /// Materialize and export one level at a time. Aggregate encoded bytes are
    /// bounded by `max_output_bytes`; failure drops all prepared outputs. The
    /// callback must also enforce its own single-export scratch/output budget.
    /// No filesystem or other external publication is performed by this helper.
    pub fn export_with(
        self,
        mut export: impl FnMut(&Scene) -> Result<SceneExport, SceneError>,
    ) -> Result<SceneLodExports, SceneError> {
        let manifest = self.manifest.clone();
        let max_output_bytes = self.max_output_bytes;
        let mut bytes = 0usize;
        let mut levels = Vec::with_capacity(manifest.levels.len());
        for scene in self.into_scenes() {
            let (level, scene) = scene?;
            let output = export(&scene)?;
            bytes = bytes.checked_add(output.bytes.len()).ok_or_else(|| {
                SceneError::new(
                    "geometry.lod_output_limit",
                    &level.name,
                    "encoded LOD byte count overflow",
                )
            })?;
            if bytes > max_output_bytes {
                return Err(SceneError::new(
                    "geometry.lod_output_limit",
                    &level.name,
                    "aggregate encoded LOD outputs exceed max_output_bytes",
                ));
            }
            levels.push(SceneLodExport { level, export: output });
        }
        Ok(SceneLodExports { manifest, levels })
    }
    /// Consume levels sequentially. Replacement geometry moves into each Scene;
    /// common scene headers, metadata and materials/images are copied only here.
    /// Drop/export each yielded scene before advancing to keep live memory low.
    pub fn into_scenes(self) -> impl Iterator<Item = Result<(SceneLodLevelReport, Scene), SceneError>> {
        let source = self.source;
        let limits = self.limits;
        self.manifest
            .levels
            .into_iter()
            .zip(self.geometry)
            .map(move |(report, geometry)| {
                let mut nodes = source.nodes.clone();
                for (id, subsets) in geometry.subsets {
                    nodes.get_mut(&id).expect("validated source node identity").subsets = subsets;
                }
                let mut provenance = source.provenance.clone();
                provenance.operations.push(SceneOperation {
                    operation: "generate_geometry_lod".into(),
                    detail: serde_json::to_string(&report)
                        .map_err(|e| SceneError::new("geometry.lod_report", &report.name, e.to_string()))?,
                });
                let scene = Scene {
                    schema: source.schema.clone(),
                    id: source.id.clone(),
                    name: source.name.clone(),
                    meters_per_unit: source.meters_per_unit,
                    up_axis: source.up_axis,
                    nodes,
                    meshes: geometry.meshes,
                    materials: source.materials.clone(),
                    dependencies: source.dependencies.clone(),
                    provenance,
                    metadata: source.metadata.clone(),
                };
                require_valid_scene(&scene, &limits)?;
                Ok((report, scene))
            })
    }
}

/// Generate bounded, deterministic mesh levels from the same source. Every level
/// retains scene/node/mesh identities and shared placements. Material boundaries
/// across all placements and corner-attribute seams constrain simplification.
/// Ratios need not be sorted: request order is preserved and names must be unique.
/// A valid partial reduction is reported when topology/error/work limits prevent
/// a target. Inputs remain unchanged on success or failure. Bounds are logical
/// work/storage estimates, not an exact allocator or process-RSS guarantee.
pub fn generate_scene_lods<'a>(
    source: &'a Scene,
    options: &SceneLodOptions,
    limits: &SceneLimits,
) -> Result<SceneLodSet<'a>, SceneError> {
    let invalid = |path: &str, detail: &str| SceneError::new("geometry.lod_options", path, detail);
    if options.levels.is_empty() || options.levels.len() > options.max_levels {
        return Err(invalid("/", "request one or more levels within max_levels"));
    }
    let mut names = BTreeSet::new();
    for level in &options.levels {
        if level.name.is_empty()
            || level.name.len() > 64
            || !level
                .name
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
            || !names.insert(&level.name)
        {
            return Err(invalid(
                &level.name,
                "unique 1–64 byte ASCII alphanumeric, underscore or hyphen level names required",
            ));
        }
        if !level.face_ratio.is_finite()
            || level.face_ratio <= 0.
            || level.face_ratio > 1.
            || !level.max_vertex_displacement.is_finite()
            || level.max_vertex_displacement < 0.
        {
            return Err(invalid(
                &level.name,
                "face_ratio must be in (0,1], and displacement finite and nonnegative",
            ));
        }
    }
    require_valid_scene(source, limits)?;
    if source.meshes.is_empty() {
        return Err(invalid("/", "geometry LOD generation requires at least one mesh"));
    }
    if source.provenance.operations.len() >= limits.max_operations {
        return Err(SceneError::new(
            "scene.operation_limit",
            "/",
            "LOD provenance would exceed operation limit",
        ));
    }
    let mut placements: BTreeMap<&str, Vec<(&String, &SceneNode)>> = BTreeMap::new();
    for (id, node) in &source.nodes {
        if let Some(mesh) = &node.mesh {
            placements.entry(mesh).or_default().push((id, node));
        }
    }
    // Preflight the uncollapsed triangulated upper bound. The simplifier may
    // legitimately retain all faces, so requested ratios cannot reserve less.
    let mut faces = 0usize;
    let mut bytes = 0usize;
    let mut counts = BTreeMap::new();
    for (id, mesh) in &source.meshes {
        check_triangulation_budget(mesh, limits)?;
        let triangles = mesh
            .face_counts
            .iter()
            .fold(0usize, |n, count| n.saturating_add((*count as usize).saturating_sub(2)));
        faces = faces.saturating_add(triangles);
        bytes = bytes
            .saturating_add(triangulated_mesh_bytes(mesh, triangles))
            .saturating_add(id.len())
            .saturating_add(2048);
        if let Some(nodes) = placements.get(id.as_str()) {
            for (node_id, node) in nodes {
                if !node.subsets.is_empty() {
                    bytes = bytes.saturating_add(node_id.len());
                    for subset in &node.subsets {
                        let count = subset.faces.iter().fold(0usize, |n, face| {
                            n.saturating_add((mesh.face_counts[*face as usize] as usize).saturating_sub(2))
                        });
                        bytes = bytes
                            .saturating_add(count.saturating_mul(4))
                            .saturating_add(subset.name.len())
                            .saturating_add(subset.material.0.len())
                            .saturating_add(128);
                    }
                }
            }
        }
        counts.insert(id, triangles);
    }
    let levels = options.levels.len();
    if faces.saturating_mul(levels) > options.max_output_faces
        || faces.saturating_mul(3).saturating_mul(levels) > options.max_source_corners
        || bytes.saturating_mul(levels) > options.max_output_bytes
    {
        return Err(SceneError::new(
            "geometry.lod_limit",
            "/",
            "aggregate triangulated LOD output/work estimate exceeds configured budget",
        ));
    }
    let source_scene_sha256 = lod_scene_hash(source)?;
    let mut geometry = (0..levels).map(|_| SceneLodGeometry::default()).collect::<Vec<_>>();
    let mut reports = options
        .levels
        .iter()
        .map(|level| SceneLodLevelReport {
            name: level.name.clone(),
            face_ratio: level.face_ratio,
            displacement_limit: level.max_vertex_displacement,
            target_reached: true,
            meshes: Vec::new(),
        })
        .collect::<Vec<_>>();
    for (id, mesh) in &source.meshes {
        let nodes = placements.get(id.as_str()).map_or(&[][..], Vec::as_slice);
        let regions = face_regions_for_nodes(source, id, mesh, nodes.iter().copied(), limits)?;
        // Charge collapse work against this mesh, not the much larger global cap.
        // Across all requests the loop cap is 64 * max_source_corners.
        let mut mesh_limits = limits.clone();
        mesh_limits.max_corners = counts[id].saturating_mul(3);
        for ((level, output), report) in options.levels.iter().zip(&mut geometry).zip(&mut reports) {
            let target_faces = ((counts[id] as f64 * level.face_ratio).floor() as usize).max(1);
            let simplified = simplify_geometry(
                mesh,
                id,
                &regions,
                target_faces,
                level.max_vertex_displacement,
                &mesh_limits,
            )?;
            let target_reached = simplified.mesh.face_counts.len() <= target_faces;
            report.target_reached &= target_reached;
            report.meshes.push(MeshLodReport {
                mesh: id.clone(),
                target_faces,
                target_reached,
                geometry: simplified.report,
            });
            // One original-face lookup per mesh/level, reused by all placements.
            let mut face_map: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
            for (new, old) in simplified.face_sources.iter().enumerate() {
                face_map.entry(*old as u32).or_default().push(new as u32);
            }
            for (node_id, node) in nodes {
                if node.subsets.is_empty() {
                    continue;
                }
                let mut subsets = node.subsets.clone();
                for subset in &mut subsets {
                    subset.faces = subset
                        .faces
                        .iter()
                        .flat_map(|face| face_map.get(face).into_iter().flatten().copied())
                        .collect();
                }
                subsets.retain(|subset| !subset.faces.is_empty());
                output.subsets.insert((*node_id).clone(), subsets);
            }
            output.meshes.insert(id.clone(), simplified.mesh);
        }
    }
    Ok(SceneLodSet {
        source, geometry, limits: limits.clone(), max_output_bytes: options.max_output_bytes,
        manifest: SceneLodManifest {
            schema: "usd-toolbox.geometry-lods.v1".into(), source_scene_id: source.id.clone(),
            source_scene_sha256, source_sha256: source.provenance.source_sha256.clone(),
            meters_per_unit: source.meters_per_unit,
            error_policy: "Independent original-vertex displacement in source scene units; not surface Hausdorff distance, UV distortion, shading equivalence, or global intersection freedom. Open/material/corner-attribute boundaries are locked; targets may remain unreached. Tangents are cleared for regeneration.".into(),
            levels: reports,
        },
    })
}

fn lod_scene_hash(source: &Scene) -> Result<String, SceneError> {
    use sha2::{Digest, Sha256};
    struct HashWriter(Sha256);
    impl std::io::Write for HashWriter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.update(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut writer = HashWriter(Sha256::new());
    serde_json::to_writer(&mut writer, source)
        .map_err(|e| SceneError::new("geometry.lod_source", "/", e.to_string()))?;
    Ok(format!("{:x}", writer.0.finalize()))
}
