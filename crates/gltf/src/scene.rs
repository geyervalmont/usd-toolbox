//! Portable static glTF scene import and indexed export.
mod import;
mod ordering;
use super::*;
pub use import::{
    SceneGltfImportOptions, discover_dependencies, gltf_scene_capabilities, import_scene, import_scene_bundle,
    preflight_scene,
};
use usd_toolbox_core::MaterialId;
use usd_toolbox_core::scene::*;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct SceneGltfOptions {
    pub format: GltfFormat,
    /// Maximum preview image edge; sources smaller than this tier are never enlarged.
    pub texture_tier: Tier,
    /// Attempt bounded vertex-cache ordering for opaque, non-transmissive primitives.
    /// Candidates are retained only if FIFO-16/FIFO-32 simulations do not regress.
    pub optimize_vertex_cache: bool,
    pub include_hidden: bool,
    pub limits: SceneLimits,
}
impl Default for SceneGltfOptions {
    fn default() -> Self {
        Self {
            format: GltfFormat::Glb,
            texture_tier: Tier::K2,
            optimize_vertex_cache: false,
            include_hidden: false,
            limits: SceneLimits::default(),
        }
    }
}

fn error(path: &str, detail: impl std::fmt::Display) -> SceneError {
    SceneError::new("gltf.scene", path, detail.to_string())
}

pub fn export_scene(scene: &Scene, options: &SceneGltfOptions) -> Result<SceneExport, SceneError> {
    require_valid_scene(scene, &options.limits)?;
    validate_payload_hashes(&scene.materials).map_err(|e| error("materials", e))?;
    let (previews, mut diagnostics) = preview::TexturePreviews::prepare(scene, options)?;
    let mut registry = ImageRegistry {
        previews,
        ..Default::default()
    };
    let mut extensions = BTreeSet::new();
    let mut losses = GltfExporter.dry_run(&scene.materials);
    let material_options = GltfExportOptions {
        format: options.format,
        texture_tier: options.texture_tier,
        ..Default::default()
    };
    let mut materials = scene
        .materials
        .iter()
        .map(|m| {
            registry.previews.alpha_weighted = preview::uses_alpha(scene, m);
            encode_material(m, &material_options, &mut registry, &mut extensions, &mut losses)
                .map_err(|e| error(&m.id.0, e))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let material_ids: BTreeMap<_, _> = scene
        .materials
        .iter()
        .enumerate()
        .map(|(i, m)| (m.id.clone(), i))
        .collect();
    diagnostics.extend(losses.iter().map(|l| {
        SceneDiagnostic::warning(
            "material.export_loss",
            &l.material.0,
            format!("{}: {}", l.parameter, l.detail),
        )
    }));
    let mut root = Map::new();
    root.insert("asset".into(),json!({"version":"2.0","generator":format!("usd-toolbox {}",env!("CARGO_PKG_VERSION")),"extras":{"olsyn_scene_id":scene.id,"provenance":scene.provenance,"usd_toolbox_metadata":scene.metadata.scene}}));
    if !extensions.is_empty() {
        root.insert("extensionsUsed".into(), json!(extensions));
    }
    registry.add_to_document(&mut root, options.format);
    restore_render_settings(scene, &mut materials, &mut root)?;
    let views = root
        .remove("bufferViews")
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default();
    let mut buffers = Buffers {
        bytes: registry.binary,
        views,
        accessors: Vec::new(),
        max_bytes: options.limits.max_bytes,
    };
    let mut nodes = Vec::new();
    let mut meshes = Vec::new();
    let mut mesh_cache = BTreeMap::new();
    let mut double_materials = BTreeMap::new();
    let mut exported_material_ids: BTreeSet<_> = scene.materials.iter().map(|m| m.id.0.clone()).collect();
    let node_ids: BTreeMap<_, _> = scene.nodes.keys().enumerate().map(|(i, id)| (id.clone(), i)).collect();
    let mut children_by_parent: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for (id, node) in &scene.nodes {
        if let Some(parent) = &node.parent {
            children_by_parent.entry(parent).or_default().push(node_ids[id]);
        }
    }
    for (id, node) in &scene.nodes {
        let mut matrix = node.transform;
        if node.reset_transform
            && let Some(parent) = &node.parent
        {
            matrix = multiply(
                inverse(world_transform(scene, parent)?)
                    .ok_or_else(|| error(id, "reset transform beneath singular parent"))?,
                matrix,
            );
        }
        if !is_trs(matrix) {
            return Err(SceneError::new(
                "gltf.shear",
                id,
                "glTF node transforms cannot represent shear; bake or factor the transform at source",
            ));
        }
        let mut encoded = json!({"name":node.name,"matrix":matrix_column_major(matrix),"extras":{"olsyn_node_id":id,"source_path":node.source_path}});
        if let Some(metadata) = scene.metadata.nodes.get(id) {
            encoded["extras"]["usd_toolbox_metadata"] = json!(metadata);
        }
        if let Some(children) = children_by_parent.get(id.as_str()) {
            encoded["children"] = json!(children);
        }
        let (visible, purpose) = effective_visibility(scene, id)?;
        let included = options.include_hidden || (visible && matches!(purpose, Purpose::Default | Purpose::Render));
        if let Some(mesh_id) = &node.mesh {
            if included {
                let mut bound_node = node.clone();
                bound_node.material = effective_material(scene, id)?;
                let node = &bound_node;
                let key = serde_json::to_string(&(mesh_id, &node.material, &node.subsets)).map_err(|e| error(id, e))?;
                let index = if let Some(&index) = mesh_cache.get(&key) {
                    index
                } else {
                    let mesh = &scene.meshes[mesh_id];
                    let primitives = encode_mesh(
                        mesh,
                        node,
                        scene,
                        &material_ids,
                        &mut buffers,
                        &mut diagnostics,
                        options.optimize_vertex_cache,
                    )?;
                    let mut primitives = primitives;
                    if mesh.double_sided {
                        for primitive in &mut primitives {
                            let original = primitive.get("material").and_then(|v| v.as_u64()).map(|v| v as usize);
                            let index = *double_materials.entry(original).or_insert_with(|| {
                                let mut material = original.map_or_else(|| json!({}), |i| materials[i].clone());
                                material["doubleSided"] = json!(true);
                                let index = materials.len();
                                // A render variant is a distinct material. Copying
                                // its source identity makes our own GLB importer
                                // reject double-sided assets as duplicate IDs.
                                let base = original.map_or("default", |i| scene.materials[i].id.0.as_str());
                                let mut suffix = 0usize;
                                let id = loop {
                                    let id = format!("{base}/double-sided/{suffix}");
                                    if exported_material_ids.insert(id.clone()) {
                                        break id;
                                    }
                                    suffix += 1;
                                };
                                material["extras"]["olsyn"]["id"] = json!(id);
                                materials.push(material);
                                index
                            });
                            primitive["material"] = json!(index);
                        }
                    }
                    let index = meshes.len();
                    meshes.push(json!({"name":mesh_id,"primitives":primitives,"extras":{"olsyn_mesh_id":mesh_id,"usd_toolbox_metadata":scene.metadata.meshes.get(mesh_id).cloned().unwrap_or_default()}}));
                    mesh_cache.insert(key, index);
                    index
                };
                encoded["mesh"] = json!(index);
            } else {
                diagnostics.push(SceneDiagnostic::warning(
                    "gltf.visibility_projection",
                    id,
                    "hidden, proxy or guide geometry omitted from preview",
                ));
            }
        }
        nodes.push(encoded);
    }
    if meshes.is_empty() {
        return Err(error("/", "no visible model geometry to export"));
    }
    let s = scene.meters_per_unit;
    let coordinates = match scene.up_axis {
        UpAxis::Y => [[s, 0., 0., 0.], [0., s, 0., 0.], [0., 0., s, 0.], [0., 0., 0., 1.]],
        UpAxis::Z => [[s, 0., 0., 0.], [0., 0., s, 0.], [0., -s, 0., 0.], [0., 0., 0., 1.]],
    };
    let roots: Vec<_> = scene
        .nodes
        .iter()
        .filter(|(_, n)| n.parent.is_none())
        .map(|(id, _)| node_ids[id])
        .collect();
    let coordinate_index = nodes.len();
    nodes.push(json!({"name":"Scene coordinates","matrix":matrix_column_major(coordinates),"children":roots}));
    root.insert("nodes".into(), json!(nodes));
    root.insert("meshes".into(), json!(meshes));
    root.insert("scenes".into(), json!([{"name":scene.name,"nodes":[coordinate_index]}]));
    root.insert("scene".into(), json!(0));
    if !materials.is_empty() {
        root.insert("materials".into(), json!(materials));
    }
    root.insert("bufferViews".into(), json!(buffers.views));
    root.insert("accessors".into(), json!(buffers.accessors));
    let mut buffer = json!({"byteLength":buffers.bytes.len()});
    if options.format == GltfFormat::Gltf {
        buffer["uri"] = json!(format!(
            "data:application/octet-stream;base64,{}",
            BASE64.encode(&buffers.bytes)
        ));
    }
    root.insert("buffers".into(), json!([buffer]));
    diagnostics.push(SceneDiagnostic::warning("gltf.geometry_projection","/","polygons triangulated; compatible render vertices indexed while preserving attribute seams; geometry encoded as float32; units and up axis mapped to glTF"));
    diagnostics.sort_by(|a, b| (&a.path, &a.code, &a.detail).cmp(&(&b.path, &b.code, &b.detail)));
    diagnostics.dedup();
    let bytes = match options.format {
        GltfFormat::Glb => write_glb(&JsonValue::Object(root), &buffers.bytes).map_err(|e| error("/", e))?,
        GltfFormat::Gltf => serde_json::to_vec(&root).map_err(|e| error("/", e))?,
    };
    if bytes.len() > options.limits.max_bytes {
        return Err(SceneError::new(
            "scene.limit",
            "/",
            "GLB/glTF output exceeds byte budget",
        ));
    }
    Ok(SceneExport { bytes, diagnostics })
}

fn matrix_column_major(m: Matrix4) -> [f64; 16] {
    std::array::from_fn(|i| m[i % 4][i / 4])
}
fn is_trs(m: Matrix4) -> bool {
    for a in 0..3 {
        for b in a + 1..3 {
            let dot = (0..3).map(|r| m[r][a] * m[r][b]).sum::<f64>();
            let lengths = ((0..3).map(|r| m[r][a] * m[r][a]).sum::<f64>()
                * (0..3).map(|r| m[r][b] * m[r][b]).sum::<f64>())
            .sqrt();
            if dot.abs() > 1e-8 * lengths.max(1e-20) {
                return false;
            }
        }
    }
    true
}

struct Buffers {
    bytes: Vec<u8>,
    views: Vec<JsonValue>,
    accessors: Vec<JsonValue>,
    max_bytes: usize,
}
impl Buffers {
    fn indices(&mut self, values: &[u32]) -> Result<usize, SceneError> {
        let short = values.iter().all(|&v| v < u32::from(u16::MAX));
        let width = if short { 2 } else { 4 };
        let length = values
            .len()
            .checked_mul(width)
            .ok_or_else(|| error("geometry", "index buffer overflow"))?;
        if self.bytes.len().saturating_add(length).saturating_add(3) > self.max_bytes {
            return Err(SceneError::new("scene.limit", "geometry", "index buffer exceeds limit"));
        }
        while !self.bytes.len().is_multiple_of(4) {
            self.bytes.push(0);
        }
        let offset = self.bytes.len();
        for &value in values {
            if short {
                self.bytes.extend_from_slice(&(value as u16).to_le_bytes());
            } else {
                self.bytes.extend_from_slice(&value.to_le_bytes());
            }
        }
        let view = self.views.len();
        self.views
            .push(json!({"buffer":0,"byteOffset":offset,"byteLength":length,"target":34963}));
        let accessor = self.accessors.len();
        self.accessors.push(
            json!({"bufferView":view,"componentType":if short {5123} else {5125},"count":values.len(),"type":"SCALAR"}),
        );
        Ok(accessor)
    }
    fn floats(&mut self, values: &[f32], components: usize, ty: &str, bounds: bool) -> Result<usize, SceneError> {
        if values.is_empty() || values.iter().any(|v| !v.is_finite()) {
            return Err(error("geometry", "empty or non-finite vertex attribute"));
        }
        let byte_length = values
            .len()
            .checked_mul(4)
            .ok_or_else(|| error("geometry", "buffer size overflow"))?;
        if self.bytes.len().saturating_add(byte_length).saturating_add(3) > self.max_bytes {
            return Err(SceneError::new(
                "scene.limit",
                "geometry",
                "expanded vertex buffer exceeds limit",
            ));
        }
        while !self.bytes.len().is_multiple_of(4) {
            self.bytes.push(0);
        }
        let offset = self.bytes.len();
        for value in values {
            self.bytes.extend_from_slice(&value.to_le_bytes());
        }
        let view = self.views.len();
        self.views
            .push(json!({"buffer":0,"byteOffset":offset,"byteLength":byte_length,"target":34962}));
        let mut accessor = json!({"bufferView":view,"componentType":5126,"count":values.len()/components,"type":ty});
        if bounds {
            accessor["min"] = json!(
                (0..components)
                    .map(|c| values
                        .chunks_exact(components)
                        .map(|v| v[c])
                        .fold(f32::INFINITY, f32::min))
                    .collect::<Vec<_>>()
            );
            accessor["max"] = json!(
                (0..components)
                    .map(|c| values
                        .chunks_exact(components)
                        .map(|v| v[c])
                        .fold(f32::NEG_INFINITY, f32::max))
                    .collect::<Vec<_>>()
            );
        }
        let index = self.accessors.len();
        self.accessors.push(accessor);
        Ok(index)
    }
}

type FaceTriangle = (usize, [usize; 3]);
#[allow(clippy::too_many_arguments)]
fn encode_mesh(
    mesh: &Mesh,
    node: &SceneNode,
    scene: &Scene,
    material_ids: &BTreeMap<MaterialId, usize>,
    buffers: &mut Buffers,
    diagnostics: &mut Vec<SceneDiagnostic>,
    optimize_vertex_cache: bool,
) -> Result<Vec<JsonValue>, SceneError> {
    let mut groups: BTreeMap<Option<MaterialId>, Vec<FaceTriangle>> = BTreeMap::new();
    let overrides: BTreeMap<_, _> = node
        .subsets
        .iter()
        .flat_map(|s| s.faces.iter().map(|f| (*f as usize, s.material.clone())))
        .collect();
    let mut start = 0usize;
    for (face, &count) in mesh.face_counts.iter().enumerate() {
        let points: Vec<_> = mesh.face_indices[start..start + count as usize]
            .iter()
            .map(|&i| mesh.positions[i as usize])
            .collect();
        let triangles = triangulate(&points)
            .map_err(|e| SceneError::new(&e.code, format!("{}/faces/{face}", node.name), e.detail))?;
        let material = overrides.get(&face).cloned().or_else(|| node.material.clone());
        for mut triangle in triangles {
            if mesh.orientation == Orientation::LeftHanded {
                triangle.swap(1, 2);
            }
            groups
                .entry(material.clone())
                .or_default()
                .push((face, triangle.map(|v| v + start)));
        }
        start += count as usize;
    }
    let mut primitives = Vec::new();
    // st is the shader's primary UV set; other sets are retained in stable order.
    let mut uv_names: Vec<_> = mesh.uv_sets.keys().collect();
    uv_names.sort_by_key(|name| {
        (
            *name != "st",
            name.strip_prefix("st")
                .and_then(|n| n.parse::<usize>().ok())
                .unwrap_or(usize::MAX),
            *name,
        )
    });
    let mut uv_indices = BTreeSet::new();
    let uv_indices = uv_names
        .iter()
        .map(|name| {
            let requested = if name.as_str() == "st" {
                Some(0)
            } else {
                name.strip_prefix("st").and_then(|n| n.parse::<usize>().ok())
            };
            let index =
                requested.unwrap_or_else(|| (0..).find(|i| !uv_indices.contains(i)).expect("finite UV set list"));
            uv_indices.insert(index);
            index
        })
        .collect::<Vec<_>>();
    for (material, triangles) in groups {
        let source = material.as_ref().map(|id| &scene.materials[material_ids[id]]);
        let mut textured = false;
        if let Some(m) = source {
            m.visit_textures(|_, _| textured = true);
        }
        let settings = material
            .as_ref()
            .and_then(|id| scene.metadata.materials.get(&id.0))
            .and_then(|m| m.original.get("gltf:render_settings"))
            .and_then(MetadataProperty::resolved)
            .map(MetadataValue::to_json);
        let needed_sets = settings
            .as_ref()
            .and_then(|s| s["textures"].as_object())
            .map(|textures| {
                textures
                    .values()
                    .map(|info| {
                        info.pointer("/transform/texCoord")
                            .or_else(|| info.get("tex_coord"))
                            .and_then(JsonValue::as_u64)
                            .unwrap_or(0) as usize
                    })
                    .collect::<BTreeSet<_>>()
            })
            .unwrap_or_else(|| BTreeSet::from([0]));
        if textured && needed_sets.iter().any(|i| !uv_indices.contains(i)) {
            return Err(SceneError::new(
                "gltf.missing_uv",
                &node.name,
                "textured model is missing a required UV set",
            ));
        }
        let normal_mapped = mesh.tangents.is_some()
            || source.is_some_and(|m| m.geometry.normal.as_ref().and_then(Value::texture).is_some());
        let count = triangles
            .len()
            .checked_mul(3)
            .ok_or_else(|| error(&node.name, "vertex count overflow"))?;
        let components =
            6 + 2 * uv_names.len() + usize::from(mesh.colors.is_some()) * 4 + usize::from(normal_mapped) * 4;
        if count.saturating_mul(components).saturating_mul(4) > buffers.max_bytes {
            return Err(SceneError::new(
                "scene.limit",
                &node.name,
                "expanded geometry exceeds limit",
            ));
        }
        let mut positions = Vec::with_capacity(count.min(mesh.positions.len()) * 3);
        let mut normals = Vec::with_capacity(count.min(mesh.positions.len()) * 3);
        let mut uvs = vec![Vec::new(); uv_names.len()];
        let mut colors = Vec::new();
        let mut tangents = Vec::new();
        let mut vertex_indices = Vec::with_capacity(count);
        let mut unique_vertices = std::collections::HashMap::<Vec<u32>, u32>::new();
        let mut values = Vec::with_capacity(components);
        let mut key = Vec::with_capacity(components);
        for (face, corners) in triangles {
            let vertices = corners.map(|c| mesh.face_indices[c] as usize);
            let p = vertices.map(|v| mesh.positions[v]);
            let edge1 = std::array::from_fn(|i| p[1][i] - p[0][i]);
            let edge2 = std::array::from_fn(|i| p[2][i] - p[0][i]);
            let face_normal = normalize(cross(edge1, edge2)).ok_or_else(|| error(&node.name, "degenerate triangle"))?;
            let basis = if normal_mapped && mesh.tangents.is_none() {
                let normal_set = settings
                    .as_ref()
                    .and_then(|s| {
                        s.pointer("/textures/normal/transform/texCoord")
                            .or_else(|| s.pointer("/textures/normal/tex_coord"))
                    })
                    .and_then(JsonValue::as_u64)
                    .unwrap_or(0) as usize;
                let uv_name = uv_indices
                    .iter()
                    .position(|&i| i == normal_set)
                    .map(|i| uv_names[i])
                    .ok_or_else(|| error(&node.name, "normal map UV set is missing"))?;
                let st = &mesh.uv_sets[uv_name];
                let t = std::array::from_fn::<_, 3, _>(|i| {
                    *st.sample(face, corners[i], vertices[i]).expect("validated UVs")
                });
                let du1 = f64::from(t[1][0] - t[0][0]);
                let dv1 = f64::from(t[0][1] - t[1][1]);
                let du2 = f64::from(t[2][0] - t[0][0]);
                let dv2 = f64::from(t[0][1] - t[2][1]);
                let d = du1 * dv2 - du2 * dv1;
                if d.abs() < 1e-15 {
                    return Err(SceneError::new(
                        "gltf.degenerate_uv",
                        &node.name,
                        "normal map tangent cannot be computed from collapsed UVs",
                    ));
                }
                Some((
                    std::array::from_fn::<_, 3, _>(|i| (edge1[i] * dv2 - edge2[i] * dv1) / d),
                    std::array::from_fn::<_, 3, _>(|i| (edge2[i] * du1 - edge1[i] * du2) / d),
                ))
            } else {
                None
            };
            for i in 0..3 {
                let vertex = vertices[i];
                let corner = corners[i];
                values.clear();
                values.extend(p[i].map(|v| v as f32));
                let n = mesh
                    .normals
                    .as_ref()
                    .and_then(|a| a.sample(face, corner, vertex))
                    .copied()
                    .unwrap_or(face_normal);
                let n = normalize(n).ok_or_else(|| error(&node.name, "invalid normal"))?;
                values.extend(n.map(|v| v as f32));
                for name in &uv_names {
                    let [u, v] = *mesh.uv_sets[*name].sample(face, corner, vertex).expect("validated UV");
                    values.extend([u, 1. - v]);
                }
                if let Some(a) = &mesh.colors {
                    values.extend(*a.sample(face, corner, vertex).expect("validated color"));
                }
                if normal_mapped && let Some(a) = &mesh.tangents {
                    let [x, y, z, w] = *a.sample(face, corner, vertex).expect("validated tangent");
                    values.extend([x, y, z, -w]);
                } else if let Some((t, b)) = basis {
                    let dot = (0..3).map(|i| n[i] * t[i]).sum::<f64>();
                    let t = normalize(std::array::from_fn(|i| t[i] - n[i] * dot))
                        .ok_or_else(|| error(&node.name, "invalid tangent"))?;
                    let c = cross(n, t);
                    let handedness = if (0..3).map(|i| c[i] * b[i]).sum::<f64>() < 0. {
                        -1.
                    } else {
                        1.
                    };
                    values.extend([t[0] as f32, t[1] as f32, t[2] as f32, handedness]);
                }
                // Attribute bits, not source point indices, define a render vertex.
                // This retains UV seams, authored hard normals, colours and handedness.
                key.clear();
                key.extend(values.iter().map(|v| if *v == 0. { 0 } else { v.to_bits() }));
                let next = u32::try_from(unique_vertices.len()).map_err(|_| error(&node.name, "too many vertices"))?;
                let index = if let Some(&index) = unique_vertices.get(key.as_slice()) {
                    index
                } else {
                    positions.extend_from_slice(&values[..3]);
                    normals.extend_from_slice(&values[3..6]);
                    let mut offset = 6;
                    for uv in &mut uvs {
                        uv.extend_from_slice(&values[offset..offset + 2]);
                        offset += 2;
                    }
                    if mesh.colors.is_some() {
                        colors.extend_from_slice(&values[offset..offset + 4]);
                        offset += 4;
                    }
                    if normal_mapped {
                        tangents.extend_from_slice(&values[offset..offset + 4]);
                    }
                    unique_vertices.insert(key.clone(), next);
                    next
                };
                vertex_indices.push(index);
            }
        }
        // Deduplication is complete. Release its keys before optional scratch
        // work, which preserves the exact render-vertex tuples and triangle winding.
        drop(unique_vertices);
        if optimize_vertex_cache && ordering::is_opaque(source, settings.as_ref()) {
            let vertex_count = positions.len() / 3;
            let occupied = vertex_count
                .saturating_mul(components)
                .saturating_mul(4)
                .saturating_add(vertex_indices.len().saturating_mul(4))
                .saturating_add(buffers.bytes.len());
            match ordering::optimize(
                &mut vertex_indices,
                vertex_count,
                buffers.max_bytes.saturating_sub(occupied),
            ) {
                ordering::Outcome::Improved(report) => {
                    ordering::remap(&mut positions, 3, &report.original_vertices);
                    ordering::remap(&mut normals, 3, &report.original_vertices);
                    for uv in &mut uvs {
                        ordering::remap(uv, 2, &report.original_vertices);
                    }
                    if !colors.is_empty() {
                        ordering::remap(&mut colors, 4, &report.original_vertices);
                    }
                    if !tangents.is_empty() {
                        ordering::remap(&mut tangents, 4, &report.original_vertices);
                    }
                    diagnostics.push(SceneDiagnostic::warning("gltf.vertex_cache_optimized", &node.name,
                        format!("opaque primitive {:?}: FIFO-16 misses {} -> {}; FIFO-32 misses {} -> {}; triangle winding and render attributes unchanged", material.as_ref().map(|id|id.0.as_str()), report.before[0], report.after[0], report.before[1], report.after[1])));
                }
                ordering::Outcome::Budget => diagnostics.push(SceneDiagnostic::warning(
                    "gltf.vertex_cache_skipped",
                    &node.name,
                    "optional vertex-cache ordering scratch exceeds remaining byte budget",
                )),
                ordering::Outcome::Unchanged => {}
            }
        }
        let mut attributes = Map::new();
        attributes.insert("POSITION".into(), json!(buffers.floats(&positions, 3, "VEC3", true)?));
        attributes.insert("NORMAL".into(), json!(buffers.floats(&normals, 3, "VEC3", false)?));
        for (uv, i) in uvs.iter().zip(&uv_indices) {
            attributes.insert(format!("TEXCOORD_{i}"), json!(buffers.floats(uv, 2, "VEC2", false)?));
        }
        if !colors.is_empty() {
            attributes.insert("COLOR_0".into(), json!(buffers.floats(&colors, 4, "VEC4", false)?));
        }
        if !tangents.is_empty() {
            attributes.insert("TANGENT".into(), json!(buffers.floats(&tangents, 4, "VEC4", false)?));
        }
        let mut primitive = json!({"mode":4,"attributes":attributes,"indices":buffers.indices(&vertex_indices)?});
        if let Some(id) = material {
            primitive["material"] = json!(material_ids[&id]);
        }
        primitives.push(primitive);
    }
    if mesh.normals.is_none() {
        diagnostics.push(SceneDiagnostic::warning(
            "gltf.normals_generated",
            &node.name,
            "faceted normals generated for polygon geometry without authored normals",
        ));
    }
    if mesh.colors.is_some() {
        diagnostics.push(SceneDiagnostic::warning(
            "gltf.vertex_color",
            &node.name,
            "vertex colors multiply the glTF base color; USD displayColor can be a fallback instead",
        ));
    }
    Ok(primitives)
}

/// Restore settings that have no direct OpenPBR scalar representation. Texture
/// indices are rebuilt from the newly encoded material, never reused from source.
fn restore_render_settings(
    scene: &Scene,
    materials: &mut [JsonValue],
    root: &mut Map<String, JsonValue>,
) -> Result<(), SceneError> {
    let mut textures = root
        .get("textures")
        .and_then(JsonValue::as_array)
        .cloned()
        .unwrap_or_default();
    let mut samplers = root
        .get("samplers")
        .and_then(JsonValue::as_array)
        .cloned()
        .unwrap_or_default();
    let mut transforms = false;
    for (material, encoded) in scene.materials.iter().zip(materials) {
        let Some(metadata) = scene.metadata.materials.get(&material.id.0) else {
            continue;
        };
        encoded["extras"]["usd_toolbox_metadata"] = json!(metadata);
        let Some(settings) = metadata
            .original
            .get("gltf:render_settings")
            .and_then(MetadataProperty::resolved)
            .map(MetadataValue::to_json)
        else {
            continue;
        };
        let mode = settings["alpha_mode"]
            .as_str()
            .ok_or_else(|| error(&material.id.0, "invalid glTF alpha metadata"))?;
        if !matches!(mode, "OPAQUE" | "BLEND" | "MASK") {
            return Err(error(&material.id.0, "invalid glTF alpha mode"));
        }
        encoded["alphaMode"] = json!(mode);
        if mode == "MASK" {
            let cutoff = settings["alpha_cutoff"]
                .as_f64()
                .filter(|v| v.is_finite() && *v >= 0.)
                .ok_or_else(|| error(&material.id.0, "invalid glTF alpha cutoff"))?;
            encoded["alphaCutoff"] = json!(cutoff);
        }
        for (name, path) in import::materials::TEXTURE_SLOTS {
            let Some(setting) = settings["textures"].get(name) else {
                continue;
            };
            let Some(info) = encoded.pointer_mut(path) else {
                continue;
            };
            let coord = setting["tex_coord"]
                .as_u64()
                .filter(|v| *v <= 31)
                .ok_or_else(|| error(&material.id.0, "invalid texture coordinate metadata"))?;
            info["texCoord"] = json!(coord);
            if let Some(transform) = setting.get("transform") {
                if !transform.is_object() {
                    return Err(error(&material.id.0, "invalid texture transform metadata"));
                }
                info["extensions"] = json!({"KHR_texture_transform":transform});
                transforms = true;
            }
            if *name == "normal"
                && let Some(scale) = setting.get("scale")
            {
                if !scale.as_f64().is_some_and(f64::is_finite) {
                    return Err(error(&material.id.0, "invalid normal scale metadata"));
                }
                info["scale"] = scale.clone();
            }
            let original = info["index"]
                .as_u64()
                .and_then(|i| textures.get(i as usize))
                .cloned()
                .ok_or_else(|| error(&material.id.0, "missing encoded texture"))?;
            let sampler = &setting["sampler"];
            if !sampler.is_object() {
                return Err(error(&material.id.0, "invalid sampler metadata"));
            }
            let sampler_index = if let Some(i) = samplers.iter().position(|s| s == sampler) {
                i
            } else {
                samplers.push(sampler.clone());
                samplers.len() - 1
            };
            let mut texture = original;
            texture["sampler"] = json!(sampler_index);
            let texture_index = if let Some(i) = textures.iter().position(|t| t == &texture) {
                i
            } else {
                textures.push(texture);
                textures.len() - 1
            };
            info["index"] = json!(texture_index);
        }
    }
    if !textures.is_empty() {
        root.insert("textures".into(), json!(textures));
    }
    if !samplers.is_empty() {
        root.insert("samplers".into(), json!(samplers));
    }
    if transforms {
        let extensions = root
            .entry("extensionsUsed")
            .or_insert_with(|| json!([]))
            .as_array_mut()
            .expect("generated extensions array");
        if !extensions.iter().any(|e| e == "KHR_texture_transform") {
            extensions.push(json!("KHR_texture_transform"));
        }
    }
    Ok(())
}
