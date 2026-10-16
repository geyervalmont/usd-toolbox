use super::*;
use std::collections::BTreeSet;

pub fn require_valid_scene(scene: &Scene, limits: &SceneLimits) -> Result<(), SceneError> {
    if let Some(issue) = validate_scene(scene, limits)
        .into_iter()
        .find(|v| v.severity == SceneSeverity::Error)
    {
        return Err(SceneError::new(&issue.code, issue.path, issue.detail));
    }
    Ok(())
}

pub fn validate_scene(scene: &Scene, limits: &SceneLimits) -> Vec<SceneDiagnostic> {
    validate_scene_impl(scene, limits, false)
}

pub(crate) fn require_valid_structure(scene: &Scene, limits: &SceneLimits) -> Result<(), SceneError> {
    if let Some(issue) = validate_scene_impl(scene, limits, true)
        .into_iter()
        .find(|v| v.severity == SceneSeverity::Error)
    {
        return Err(SceneError::new(&issue.code, issue.path, issue.detail));
    }
    Ok(())
}

fn validate_scene_impl(scene: &Scene, limits: &SceneLimits, structure_only: bool) -> Vec<SceneDiagnostic> {
    let mut issues = Vec::new();
    let mut error = |code: &str, path: &str, detail: String| {
        issues.push(SceneDiagnostic {
            code: code.into(),
            severity: SceneSeverity::Error,
            path: path.into(),
            detail,
        });
    };
    if scene.schema != SCENE_SCHEMA {
        error("scene.schema", "/", scene.schema.clone());
    }
    if scene.id.is_empty() {
        error("scene.identity", "/", "scene ID is empty".into());
    }
    if !scene.meters_per_unit.is_finite() || scene.meters_per_unit <= 0. {
        error("scene.units", "/", "meters_per_unit must be finite and positive".into());
    }
    if scene.nodes.len() > limits.max_nodes || scene.dependencies.len() > limits.max_dependencies {
        error("scene.limit", "/", "node or dependency limit exceeded".into());
        return issues;
    }
    if scene.provenance.operations.len() > limits.max_operations {
        error(
            "scene.operation_limit",
            "/provenance/operations",
            "operation history limit exceeded".into(),
        );
        return issues;
    }
    let material_ids: BTreeSet<_> = scene.materials.iter().map(|m| &m.id).collect();
    if !structure_only {
        for issue in crate::validate_materials(&scene.materials) {
            error("scene.material", &issue.path, issue.detail);
        }
    }
    if let Err(issue) = validate_metadata(&scene.metadata, &MetadataLimits::default()) {
        error(&issue.code, &issue.path, issue.detail);
    }
    for id in scene.metadata.nodes.keys() {
        if !scene.nodes.contains_key(id) {
            error("metadata.target", id, "metadata node target does not exist".into());
        }
    }
    for id in scene.metadata.meshes.keys() {
        if !scene.meshes.contains_key(id) {
            error("metadata.target", id, "metadata mesh target does not exist".into());
        }
    }
    for id in scene.metadata.materials.keys() {
        if !material_ids.contains(&MaterialId::new(id)) {
            error("metadata.target", id, "metadata material target does not exist".into());
        }
    }
    let mut points = 0usize;
    let mut faces = 0usize;
    let mut corners = 0usize;
    let mut bytes = 0usize;
    if !structure_only {
        for (id, mesh) in &scene.meshes {
            if id.is_empty() {
                error("scene.identity", id, "mesh ID is empty".into());
            }
            points = points.saturating_add(mesh.positions.len());
            faces = faces.saturating_add(mesh.face_counts.len());
            corners = corners.saturating_add(mesh.face_indices.len());
            bytes = bytes
                .saturating_add(mesh.positions.len().saturating_mul(24))
                .saturating_add(mesh.face_indices.len().saturating_mul(4))
                .saturating_add(mesh.face_counts.len().saturating_mul(4));
            if points > limits.max_points || faces > limits.max_faces || corners > limits.max_corners {
                error("scene.limit", id, "geometry count limit exceeded".into());
                return issues;
            }
            if mesh.positions.is_empty() || mesh.face_counts.is_empty() {
                error("mesh.empty", id, "mesh has no points or faces".into());
            }
            if mesh.positions.iter().flatten().any(|x| !x.is_finite()) {
                error("mesh.non_finite", id, "positions must be finite".into());
            }
            let count = mesh
                .face_counts
                .iter()
                .try_fold(0usize, |a, &b| a.checked_add(b as usize));
            if count != Some(mesh.face_indices.len())
                || mesh
                    .face_counts
                    .iter()
                    .any(|&n| n < 3 || n as usize > limits.max_face_corners)
            {
                error(
                    "mesh.topology",
                    id,
                    "face counts do not match corners, or face size is outside limits".into(),
                );
            }
            if mesh.face_indices.iter().any(|&i| i as usize >= mesh.positions.len()) {
                error("mesh.index", id, "point index out of range".into());
            }
            if let Some(a) = &mesh.normals {
                bytes = bytes.saturating_add(attribute_bytes(a));
                if !attribute_valid(a, mesh) || a.values.iter().any(|v| normalize(*v).is_none()) {
                    error(
                        "mesh.normals",
                        id,
                        "invalid normal values, interpolation count or indices".into(),
                    );
                }
            }
            if let Some(a) = &mesh.tangents {
                bytes = bytes.saturating_add(attribute_bytes(a));
                if !attribute_valid(a, mesh)
                    || a.values.iter().any(|v| {
                        normalize([f64::from(v[0]), f64::from(v[1]), f64::from(v[2])]).is_none()
                            || (v[3] != 1. && v[3] != -1.)
                    })
                {
                    error(
                        "mesh.tangents",
                        id,
                        "invalid tangent values, handedness, count or indices".into(),
                    );
                }
            }
            for (name, a) in &mesh.uv_sets {
                bytes = bytes.saturating_add(attribute_bytes(a));
                if name.is_empty() || !attribute_valid(a, mesh) || a.values.iter().flatten().any(|x| !x.is_finite()) {
                    error("mesh.uv", id, format!("invalid UV set {name}"));
                }
            }
            if let Some(a) = &mesh.colors {
                bytes = bytes.saturating_add(attribute_bytes(a));
                if !attribute_valid(a, mesh)
                    || a.values
                        .iter()
                        .flatten()
                        .any(|x| !x.is_finite() || !(0.0..=1.0).contains(x))
                {
                    error("mesh.colors", id, "invalid color values, count or indices".into());
                }
            }
        }
    }
    validate_hierarchy(scene, limits, &mut error);
    for (id, node) in &scene.nodes {
        if id.is_empty() {
            error("scene.identity", id, "node ID is empty".into());
        }
        if node.transform.iter().flatten().any(|v| !v.is_finite()) || node.transform[3] != [0., 0., 0., 1.] {
            error("node.transform", id, "transform must be a finite affine matrix".into());
        }
        let mesh = node.mesh.as_ref().and_then(|m| scene.meshes.get(m));
        if node.mesh.is_some() && mesh.is_none() {
            error("scene.missing_mesh", id, "mesh reference is unresolved".into());
        }
        if let Some(mat) = &node.material
            && !material_ids.contains(mat)
        {
            error("scene.missing_material", id, mat.0.clone());
        }
        let mut subset_faces = BTreeSet::new();
        let mut subset_names = BTreeSet::new();
        for subset in &node.subsets {
            if subset.name.is_empty() || !subset_names.insert(&subset.name) {
                error("binding.name", id, "empty or duplicate subset name".into());
            }
            if !material_ids.contains(&subset.material) {
                error("scene.missing_material", id, subset.material.0.clone());
            }
            if subset.faces.is_empty() {
                error("binding.empty", id, "empty face subset".into());
            }
            for &face in &subset.faces {
                if mesh.is_none_or(|m| face as usize >= m.face_counts.len()) {
                    error("binding.face", id, "face subset index out of range".into());
                    break;
                }
                if !subset_faces.insert(face) {
                    error(
                        "binding.overlap",
                        id,
                        "face subsets overlap or contain duplicate faces".into(),
                    );
                    break;
                }
            }
        }
    }
    let mut dependency_paths = BTreeSet::new();
    for dependency in &scene.dependencies {
        if !crate::Sha256(dependency.sha256.clone()).is_canonical()
            || !dependency_paths.insert(&dependency.resolved_path)
        {
            error(
                "scene.dependency",
                &dependency.resolved_path,
                "invalid hash or duplicate resolved dependency".into(),
            );
        }
    }
    if !structure_only {
        for material in &scene.materials {
            material.visit_textures(|_, t| {
                for source in t.tiers.values() {
                    bytes = bytes.saturating_add(source.bytes.len());
                }
            });
            if let Some(graph) = &material.materialx {
                for asset in graph.assets.values() {
                    bytes = bytes.saturating_add(asset.bytes.len());
                }
            }
            for asset in &material.auxiliary {
                bytes = bytes.saturating_add(asset.bytes.len());
            }
            for asset in material.provenance.source_assets.values() {
                bytes = bytes.saturating_add(asset.bytes.len());
            }
        }
    }
    if bytes > limits.max_bytes {
        error(
            "scene.limit",
            "/",
            "decoded geometry and texture byte limit exceeded".into(),
        );
    }
    issues.sort_by(|a, b| (&a.path, &a.code, &a.detail).cmp(&(&b.path, &b.code, &b.detail)));
    issues.dedup();
    issues
}

fn attribute_bytes<T>(a: &Attribute<T>) -> usize {
    a.values
        .len()
        .saturating_mul(std::mem::size_of::<T>())
        .saturating_add(a.indices.as_ref().map_or(0, |v| v.len().saturating_mul(4)))
}

pub(crate) fn attribute_valid<T>(a: &Attribute<T>, mesh: &Mesh) -> bool {
    let n = match a.interpolation {
        Interpolation::Constant => 1,
        Interpolation::Uniform => mesh.face_counts.len(),
        Interpolation::Vertex | Interpolation::Varying => mesh.positions.len(),
        Interpolation::FaceVarying => mesh.face_indices.len(),
    };
    if let Some(indices) = &a.indices {
        indices.len() == n && indices.iter().all(|&i| (i as usize) < a.values.len())
    } else {
        a.values.len() == n
    }
}

fn validate_hierarchy(scene: &Scene, limits: &SceneLimits, error: &mut impl FnMut(&str, &str, String)) {
    let mut depths: BTreeMap<&str, Option<usize>> = BTreeMap::new();
    for id in scene.nodes.keys() {
        if depths.contains_key(id.as_str()) {
            continue;
        }
        let mut trail = Vec::new();
        let mut seen = BTreeSet::new();
        let mut current = id.as_str();
        let mut depth = loop {
            if let Some(depth) = depths.get(current) {
                break *depth;
            }
            if !seen.insert(current) {
                error("scene.cycle", id, "parent cycle".into());
                break None;
            }
            let Some(node) = scene.nodes.get(current) else {
                error("scene.missing_node", id, format!("missing parent {current}"));
                break None;
            };
            trail.push(current);
            if trail.len() > limits.max_depth {
                error("scene.depth", id, "hierarchy depth limit exceeded".into());
                break None;
            }
            match node.parent.as_deref() {
                Some(parent) => current = parent,
                None => break Some(0),
            }
        };
        for key in trail.into_iter().rev() {
            depth = depth.and_then(|value| value.checked_add(1));
            if depth.is_some_and(|value| value > limits.max_depth) {
                error("scene.depth", key, "hierarchy depth limit exceeded".into());
                depth = None;
            }
            depths.insert(key, depth);
        }
    }
}
