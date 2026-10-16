//! Static glTF 2.0 reader. Hosts supply all resources; no filesystem or network IO.
//! Accessor rules follow the Khronos glTF 2.0 specification (2026-10-16).
mod accessors;
pub(super) mod materials;

use super::*;
use std::borrow::Cow;
use usd_toolbox_core::{ColorSpace, MapRole, NormalConvention, ShadingModel, TextureSource};

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct SceneGltfImportOptions {
    pub id: Option<String>,
    pub name: Option<String>,
    pub limits: SceneLimits,
}

fn invalid(path: impl Into<String>, detail: impl Into<String>) -> SceneError {
    SceneError::new("gltf.invalid", path, detail)
}
fn unsupported(path: impl Into<String>, detail: impl Into<String>) -> SceneError {
    SceneError::new("gltf.unsupported", path, detail)
}
fn limit(path: &str) -> SceneError {
    SceneError::new(
        "scene.limit",
        path,
        "glTF input or decoded data exceeds the configured budget",
    )
}
fn array<'a>(value: &'a JsonValue, key: &str) -> Result<&'a [JsonValue], SceneError> {
    match value.get(key) {
        None => Ok(&[]),
        Some(JsonValue::Array(v)) => Ok(v),
        _ => Err(invalid(key, "expected an array")),
    }
}
fn index(value: &JsonValue, path: &str) -> Result<usize, SceneError> {
    value
        .as_u64()
        .and_then(|v| usize::try_from(v).ok())
        .ok_or_else(|| invalid(path, "expected a nonnegative integer"))
}
fn required(value: &JsonValue, field: &str) -> Result<usize, SceneError> {
    index(&value[field], field)
}
fn optional(value: &JsonValue, field: &str, default: usize) -> Result<usize, SceneError> {
    value.get(field).map_or(Ok(default), |v| index(v, field))
}
fn bool_value(value: &JsonValue, field: &str, default: bool) -> Result<bool, SceneError> {
    value.get(field).map_or(Ok(default), |v| {
        v.as_bool().ok_or_else(|| invalid(field, "expected boolean"))
    })
}
fn numbers<const N: usize>(value: Option<&JsonValue>, default: [f64; N], path: &str) -> Result<[f64; N], SceneError> {
    let Some(value) = value else {
        return Ok(default);
    };
    let values = value
        .as_array()
        .filter(|v| v.len() == N)
        .ok_or_else(|| invalid(path, format!("expected {N} numbers")))?;
    let mut result = default;
    for (out, value) in result.iter_mut().zip(values) {
        *out = value
            .as_f64()
            .filter(|v| v.is_finite())
            .ok_or_else(|| invalid(path, "non-finite or non-numeric value"))?;
    }
    Ok(result)
}

struct Parsed<'a> {
    document: JsonValue,
    binary: Option<&'a [u8]>,
}
fn parse<'a>(bytes: &'a [u8], limits: &SceneLimits) -> Result<Parsed<'a>, SceneError> {
    if bytes.len() > limits.max_bytes {
        return Err(limit("/"));
    }
    let (json, binary) = if bytes.starts_with(b"glTF") {
        let word = |start: usize| {
            bytes
                .get(start..start + 4)
                .map(|v| u32::from_le_bytes(v.try_into().unwrap()) as usize)
                .ok_or_else(|| invalid("/", "truncated GLB header"))
        };
        if word(4)? != 2 || word(8)? != bytes.len() {
            return Err(invalid(
                "/",
                "GLB version must be 2 and declared length must match input",
            ));
        }
        let mut offset = 12usize;
        let mut json = None;
        let mut binary = None;
        while offset < bytes.len() {
            let length = word(offset)?;
            let kind = word(offset + 4)?;
            if !length.is_multiple_of(4) {
                return Err(invalid("/", "GLB chunks must be four-byte aligned"));
            }
            offset = offset
                .checked_add(8)
                .ok_or_else(|| invalid("/", "GLB offset overflow"))?;
            let end = offset
                .checked_add(length)
                .ok_or_else(|| invalid("/", "GLB length overflow"))?;
            let chunk = bytes
                .get(offset..end)
                .ok_or_else(|| invalid("/", "GLB chunk exceeds input"))?;
            match kind {
                0x4e4f534a if json.is_none() && offset == 20 => json = Some(chunk),
                0x004e4942 if json.is_some() && binary.is_none() => binary = Some(chunk),
                _ => return Err(unsupported("/", "duplicate, out-of-order or unsupported GLB chunk")),
            }
            offset = end;
        }
        (json.ok_or_else(|| invalid("/", "missing GLB JSON chunk"))?, binary)
    } else {
        (bytes, None)
    };
    let document: JsonValue = serde_json::from_slice(json).map_err(|e| invalid("/", e.to_string()))?;
    if !document.is_object() || !document["asset"].is_object() || !document["asset"]["version"].is_string() {
        return Err(invalid(
            "asset.version",
            "glTF requires an asset object with a string version",
        ));
    }
    if document["asset"]["version"] != "2.0" {
        return Err(unsupported("asset.version", "only glTF 2.0 documents are supported"));
    }
    if document["asset"].get("minVersion").is_some_and(|v| v != "2.0") {
        return Err(unsupported(
            "asset.minVersion",
            "minimum glTF version exceeds the supported 2.0 profile",
        ));
    }
    Ok(Parsed { document, binary })
}

fn safe_path(path: &str) -> Result<(), SceneError> {
    if path.is_empty()
        || path.starts_with('/')
        || path.contains(['\\', ':', '?', '#'])
        || path.chars().any(char::is_control)
        || path.split('/').any(|p| p.is_empty() || p == "." || p == "..")
    {
        return Err(SceneError::new(
            "gltf.dependency_path",
            path,
            "provide a confined relative bundle path without traversal or URL schemes",
        ));
    }
    Ok(())
}
fn dependency_path(source_name: &str, uri: &str) -> Result<String, SceneError> {
    let mut decoded = Vec::with_capacity(uri.len());
    let mut chars = uri.as_bytes().iter().copied();
    while let Some(byte) = chars.next() {
        if byte == b'%' {
            let high = chars.next().and_then(|v| (v as char).to_digit(16));
            let low = chars.next().and_then(|v| (v as char).to_digit(16));
            match (high, low) {
                (Some(a), Some(b)) => decoded.push((a * 16 + b) as u8),
                _ => return Err(invalid(uri, "malformed URI percent escape")),
            }
        } else {
            decoded.push(byte);
        }
    }
    let decoded = String::from_utf8(decoded).map_err(|_| invalid(uri, "URI is not UTF-8"))?;
    safe_path(&decoded)?;
    Ok(source_name
        .rsplit_once('/')
        .map_or(decoded.clone(), |(dir, _)| format!("{dir}/{decoded}")))
}

/// Return normalized bundle paths for external buffers/images, without loading them
/// or requiring that the document's features can be imported.
pub fn discover_dependencies(bytes: &[u8], source_name: &str, limits: &SceneLimits) -> Result<Vec<String>, SceneError> {
    safe_path(source_name)?;
    let parsed = parse(bytes, limits)?;
    let mut paths = BTreeSet::new();
    for kind in ["buffers", "images"] {
        for (i, item) in array(&parsed.document, kind)?.iter().enumerate() {
            if let Some(uri) = item.get("uri") {
                let uri = uri
                    .as_str()
                    .ok_or_else(|| invalid(format!("{kind}/{i}/uri"), "URI must be a string"))?;
                if !uri.starts_with("data:") {
                    paths.insert(dependency_path(source_name, uri)?);
                }
            }
            if paths.len() > limits.max_dependencies {
                return Err(limit("dependencies"));
            }
        }
    }
    Ok(paths.into_iter().collect())
}

/// Import self-contained GLB or glTF bytes. External resources require a bundle.
pub fn import_scene(
    bytes: &[u8],
    source_name: &str,
    options: &SceneGltfImportOptions,
) -> Result<SceneImport, SceneError> {
    import(bytes, source_name, &BTreeMap::new(), options)
}

/// Import bytes with an explicit host-resolved bundle. Keys are normalized,
/// confined relative paths; root-relative URI resolution never reads the host.
pub fn import_scene_bundle(
    root_name: &str,
    files: &BTreeMap<String, Vec<u8>>,
    options: &SceneGltfImportOptions,
) -> Result<SceneImport, SceneError> {
    if files.len() > options.limits.max_dependencies.saturating_add(1) {
        return Err(limit("dependencies"));
    }
    let mut bytes = 0usize;
    for (name, content) in files {
        safe_path(name)?;
        bytes = bytes.saturating_add(content.len());
        if bytes > options.limits.max_bytes {
            return Err(limit("dependencies"));
        }
    }
    let root = files
        .get(root_name)
        .ok_or_else(|| invalid(root_name, "root document is missing from the bundle"))?;
    import(root, root_name, files, options)
}

struct Reader<'a> {
    document: &'a JsonValue,
    source_name: &'a str,
    files: &'a BTreeMap<String, Vec<u8>>,
    buffers: Vec<Cow<'a, [u8]>>,
    images: BTreeMap<usize, TextureSource>,
    limits: &'a SceneLimits,
    decoded_bytes: usize,
    image_bytes: usize,
    dependencies: BTreeMap<String, SceneDependency>,
}
impl<'a> Reader<'a> {
    fn charge(&mut self, bytes: usize, path: &str) -> Result<(), SceneError> {
        self.decoded_bytes = self.decoded_bytes.saturating_add(bytes);
        if self.decoded_bytes > self.limits.max_bytes {
            return Err(limit(path));
        }
        Ok(())
    }
    fn resource(&mut self, uri: &str) -> Result<Cow<'a, [u8]>, SceneError> {
        if let Some(data) = uri.strip_prefix("data:") {
            let (header, payload) = data
                .split_once(',')
                .ok_or_else(|| invalid(uri.get(..40).unwrap_or(uri), "invalid data URI"))?;
            if !header.ends_with(";base64") {
                return Err(unsupported("uri", "only base64 data URIs are supported"));
            }
            if payload.len().saturating_mul(3) / 4 > self.limits.max_bytes {
                return Err(limit("uri"));
            }
            return BASE64
                .decode(payload)
                .map(Cow::Owned)
                .map_err(|e| invalid("uri", e.to_string()));
        }
        let path = dependency_path(self.source_name, uri)?;
        let bytes = self.files.get(&path).ok_or_else(|| {
            SceneError::new(
                "gltf.dependency_missing",
                &path,
                "supply this resource in the input bundle",
            )
        })?;
        if !self.dependencies.contains_key(&path) {
            if self.dependencies.len() >= self.limits.max_dependencies {
                return Err(limit("dependencies"));
            }
            self.dependencies.insert(
                path.clone(),
                SceneDependency {
                    authored_path: uri.into(),
                    resolved_path: path,
                    sha256: format!("{:x}", Sha256::digest(bytes)),
                    bytes: bytes.len() as u64,
                },
            );
        }
        Ok(Cow::Borrowed(bytes))
    }
}

const EXTENSIONS: &[&str] = &[
    "KHR_materials_ior",
    "KHR_materials_emissive_strength",
    "KHR_materials_transmission",
    "KHR_materials_clearcoat",
    "KHR_texture_transform",
];
fn check_features(document: &JsonValue) -> Result<(), SceneError> {
    for key in ["extensionsRequired", "extensionsUsed"] {
        for ext in array(document, key)? {
            if !ext.as_str().is_some_and(|name| EXTENSIONS.contains(&name)) {
                return Err(unsupported(
                    key,
                    format!("unsupported extension {ext}; decompress or bake it with the authoring tool"),
                ));
            }
        }
    }
    let mut pending = vec![("/".to_string(), document)];
    while let Some((path, value)) = pending.pop() {
        match value {
            JsonValue::Object(map) => {
                if let Some(extensions) = map.get("extensions") {
                    let extensions = extensions
                        .as_object()
                        .ok_or_else(|| invalid(&path, "extensions must be an object"))?;
                    for name in extensions.keys() {
                        if !EXTENSIONS.contains(&name.as_str())
                            || !path.starts_with("//materials/")
                            || (name == "KHR_texture_transform" && !path.ends_with("Texture"))
                        {
                            return Err(unsupported(
                                &path,
                                format!("unsupported extension {name} at this location"),
                            ));
                        }
                    }
                }
                for (key, v) in map {
                    if key != "extras" {
                        pending.push((format!("{path}/{key}"), v));
                    }
                }
            }
            JsonValue::Array(values) => {
                for (i, v) in values.iter().enumerate() {
                    pending.push((format!("{path}/{i}"), v));
                }
            }
            _ => {}
        }
    }
    for key in ["animations", "skins", "cameras"] {
        if !array(document, key)?.is_empty() {
            return Err(unsupported(
                key,
                "static polygon profile does not support animation, skinning or cameras",
            ));
        }
    }
    Ok(())
}

fn import(
    bytes: &[u8],
    source_name: &str,
    files: &BTreeMap<String, Vec<u8>>,
    options: &SceneGltfImportOptions,
) -> Result<SceneImport, SceneError> {
    safe_path(source_name)?;
    let parsed = parse(bytes, &options.limits)?;
    let doc = &parsed.document;
    check_features(doc)?;
    let mut reader = Reader {
        document: doc,
        source_name,
        files,
        buffers: Vec::new(),
        images: BTreeMap::new(),
        limits: &options.limits,
        decoded_bytes: 0,
        image_bytes: 0,
        dependencies: BTreeMap::new(),
    };
    let mut encoded_bytes = 0usize;
    for (i, buffer) in array(doc, "buffers")?.iter().enumerate() {
        let declared = required(buffer, "byteLength")?;
        let data = if let Some(uri) = buffer.get("uri") {
            reader.resource(
                uri.as_str()
                    .ok_or_else(|| invalid(format!("buffers/{i}/uri"), "URI must be string"))?,
            )?
        } else if i == 0 {
            Cow::Borrowed(
                parsed
                    .binary
                    .ok_or_else(|| invalid("buffers/0", "buffer has neither URI nor GLB binary chunk"))?,
            )
        } else {
            return Err(invalid(
                format!("buffers/{i}"),
                "only the first GLB buffer can omit its URI",
            ));
        };
        if declared == 0 || declared > data.len() || (buffer.get("uri").is_none() && data.len() - declared > 3) {
            return Err(invalid(
                format!("buffers/{i}"),
                "buffer length does not match its bytes",
            ));
        }
        encoded_bytes = encoded_bytes.saturating_add(data.len());
        if encoded_bytes > options.limits.max_bytes {
            return Err(limit("buffers"));
        }
        reader.buffers.push(match data {
            Cow::Borrowed(v) => Cow::Borrowed(&v[..declared]),
            Cow::Owned(mut v) => {
                v.truncate(declared);
                Cow::Owned(v)
            }
        });
    }
    reader.validate_views()?;
    let hash = format!("{:x}", Sha256::digest(bytes));
    let mut scene = Scene::new(
        options
            .id
            .clone()
            .or_else(|| {
                doc.pointer("/asset/extras/olsyn_scene_id")
                    .and_then(JsonValue::as_str)
                    .map(str::to_owned)
            })
            .unwrap_or_else(|| format!("gltf-{}", &hash[..16])),
    );
    scene.name = options.name.clone().unwrap_or_else(|| source_name.into());
    scene.provenance.source_name = Some(source_name.into());
    scene.provenance.source_sha256 = Some(hash);
    let mut diagnostics = Vec::new();
    for (i, material) in array(doc, "materials")?.iter().enumerate() {
        scene.materials.push(reader.material(i, material, &mut diagnostics)?);
    }
    let mesh_docs = array(doc, "meshes")?;
    let mut meshes = Vec::new();
    let mut point_count = 0usize;
    let mut face_count = 0usize;
    for (mi, mesh) in mesh_docs.iter().enumerate() {
        if mesh.get("weights").is_some() {
            return Err(unsupported(
                format!("meshes/{mi}/weights"),
                "morph weights are unsupported",
            ));
        }
        let primitives = array(mesh, "primitives")?;
        if primitives.is_empty() {
            return Err(invalid(format!("meshes/{mi}"), "mesh requires primitives"));
        }
        let mut parts = Vec::new();
        for (pi, primitive) in primitives.iter().enumerate() {
            if primitive.get("targets").is_some() {
                return Err(unsupported(
                    format!("meshes/{mi}/primitives/{pi}/targets"),
                    "morph targets are unsupported",
                ));
            }
            let material = primitive.get("material").map(|v| index(v, "material")).transpose()?;
            if material.is_some_and(|i| i >= scene.materials.len()) {
                return Err(invalid("material", "material index is out of bounds"));
            }
            let model = reader.mesh(primitive, &mut diagnostics)?;
            point_count = point_count.saturating_add(model.positions.len());
            face_count = face_count.saturating_add(model.face_counts.len());
            if point_count > options.limits.max_points
                || face_count > options.limits.max_faces
                || face_count.saturating_mul(3) > options.limits.max_corners
            {
                return Err(limit("meshes"));
            }
            let id = mesh
                .pointer("/extras/olsyn_mesh_id")
                .and_then(JsonValue::as_str)
                .map_or_else(
                    || format!("gltf/meshes/{mi}/primitives/{pi}"),
                    |id| {
                        if primitives.len() == 1 {
                            id.into()
                        } else {
                            format!("{id}/primitives/{pi}")
                        }
                    },
                );
            if let Some(existing) = scene.meshes.get(&id) {
                if existing != &model {
                    return Err(invalid(
                        "meshes/extras/olsyn_mesh_id",
                        "conflicting stable mesh identities",
                    ));
                }
            } else {
                scene.meshes.insert(id.clone(), model);
            }
            parts.push((id, material));
        }
        meshes.push(parts);
    }
    let nodes = array(doc, "nodes")?;
    if nodes.len() > options.limits.max_nodes {
        return Err(limit("nodes"));
    }
    let node_ids = nodes
        .iter()
        .enumerate()
        .map(|(i, n)| {
            n.pointer("/extras/olsyn_node_id")
                .and_then(JsonValue::as_str)
                .map_or_else(|| format!("gltf/nodes/{i}"), str::to_owned)
        })
        .collect::<Vec<_>>();
    if node_ids.iter().collect::<BTreeSet<_>>().len() != node_ids.len() {
        return Err(invalid("nodes/extras/olsyn_node_id", "duplicate stable node identity"));
    }
    let mut parents = vec![None; nodes.len()];
    for (i, node) in nodes.iter().enumerate() {
        if !node.is_object() {
            return Err(invalid(format!("nodes/{i}"), "node must be an object"));
        }
        for key in ["skin", "weights", "camera"] {
            if node.get(key).is_some() {
                return Err(unsupported(
                    format!("nodes/{i}/{key}"),
                    "requires a feature outside the static polygon profile",
                ));
            }
        }
        for child in array(node, "children")? {
            let child = index(child, "children")?;
            let parent = parents
                .get_mut(child)
                .ok_or_else(|| invalid("children", "node index out of bounds"))?;
            if parent.replace(i).is_some() {
                return Err(invalid(
                    "children",
                    "node has multiple parents or duplicate child links",
                ));
            }
        }
    }
    // Validate the entire declared forest, including unreachable cycles.
    let mut states = vec![0u8; nodes.len()];
    for origin in 0..nodes.len() {
        let mut path = Vec::new();
        let mut current = Some(origin);
        while let Some(i) = current {
            match states[i] {
                1 => return Err(invalid(format!("nodes/{i}"), "cycle in node hierarchy")),
                2 => break,
                _ => {}
            }
            if path.len() >= options.limits.max_depth {
                return Err(limit("nodes/depth"));
            }
            states[i] = 1;
            path.push(i);
            current = parents[i];
        }
        for i in path {
            states[i] = 2;
        }
    }
    let scenes = array(doc, "scenes")?;
    let roots = if scenes.is_empty() {
        if doc.get("scene").is_some() {
            return Err(invalid("scene", "default scene index is out of bounds"));
        }
        (0..nodes.len()).filter(|&i| parents[i].is_none()).collect::<Vec<_>>()
    } else {
        let selected = optional(doc, "scene", 0)?;
        let selected_scene = scenes
            .get(selected)
            .ok_or_else(|| invalid("scene", "default scene index is out of bounds"))?;
        if options.name.is_none()
            && let Some(name) = selected_scene["name"].as_str()
        {
            scene.name = name.into();
        }
        if scenes.len() > 1 {
            diagnostics.push(SceneDiagnostic::warning(
                "gltf.scene_selection",
                "scenes",
                format!(
                    "selected scene {selected}; {} other scene definitions omitted",
                    scenes.len() - 1
                ),
            ));
        }
        array(selected_scene, "nodes")?
            .iter()
            .map(|v| index(v, "scenes/nodes"))
            .collect::<Result<Vec<_>, _>>()?
    };
    let mut selected = BTreeSet::new();
    let mut pending = roots.iter().map(|&i| (i, 0usize)).collect::<Vec<_>>();
    for &i in &roots {
        if i >= nodes.len() || parents[i].is_some() {
            return Err(invalid("scenes/nodes", "scene root must reference a parentless node"));
        }
    }
    while let Some((i, depth)) = pending.pop() {
        if !selected.insert(i) {
            return Err(invalid("scenes/nodes", "duplicate root or descendant"));
        }
        if depth >= options.limits.max_depth {
            return Err(limit("nodes/depth"));
        }
        for c in array(&nodes[i], "children")? {
            pending.push((index(c, "children")?, depth + 1));
        }
    }
    for i in selected {
        let source = &nodes[i];
        let id = node_ids[i].clone();
        let mut node = SceneNode::new(
            source["name"]
                .as_str()
                .map_or_else(|| format!("Node {i}"), str::to_owned),
        );
        node.source_path = Some(
            source
                .pointer("/extras/source_path")
                .and_then(JsonValue::as_str)
                .map_or_else(|| format!("/nodes/{i}"), str::to_owned),
        );
        node.parent = parents[i].map(|p| node_ids[p].clone());
        node.transform = transform(source)?;
        if let Some(mesh_index) = source.get("mesh") {
            let mesh_index = index(mesh_index, "mesh")?;
            let parts = meshes
                .get(mesh_index)
                .ok_or_else(|| invalid("mesh", "mesh index out of bounds"))?;
            if parts.len() == 1 {
                node.mesh = Some(parts[0].0.clone());
                node.material = parts[0].1.map(|m| scene.materials[m].id.clone());
            } else {
                for (pi, (mesh, material)) in parts.iter().enumerate() {
                    let mut child = SceneNode::new(format!("{} primitive {pi}", node.name));
                    child.parent = Some(id.clone());
                    child.mesh = Some(mesh.clone());
                    child.material = material.map(|m| scene.materials[m].id.clone());
                    if scene.nodes.insert(format!("{id}/primitives/{pi}"), child).is_some() {
                        return Err(invalid(&id, "stable identity collides with a generated primitive node"));
                    }
                }
            }
        }
        if scene.nodes.insert(id.clone(), node).is_some() {
            return Err(invalid(id, "stable identity collides with an existing node"));
        }
        if scene.nodes.len() > options.limits.max_nodes {
            return Err(limit("nodes"));
        }
    }
    scene.dependencies = reader.dependencies.into_values().collect();
    preserve_metadata(doc, &node_ids, &meshes, &mut scene)?;
    require_valid_scene(&scene, &options.limits)?;
    Ok(SceneImport { scene, diagnostics })
}

fn transform(node: &JsonValue) -> Result<Matrix4, SceneError> {
    if node.get("matrix").is_some() {
        if ["translation", "rotation", "scale"]
            .iter()
            .any(|k| node.get(k).is_some())
        {
            return Err(invalid("matrix", "matrix and TRS cannot be authored together"));
        }
        let values = numbers(node.get("matrix"), [0.; 16], "matrix")?;
        let matrix = std::array::from_fn(|r| std::array::from_fn(|c| values[c * 4 + r]));
        if matrix[3] != [0., 0., 0., 1.] || !super::is_trs(matrix) {
            return Err(invalid("matrix", "glTF matrix must be affine and decomposable to TRS"));
        }
        return Ok(matrix);
    }
    let [x, y, z, w] = numbers(node.get("rotation"), [0., 0., 0., 1.], "rotation")?;
    let length = x * x + y * y + z * z + w * w;
    if (length - 1.).abs() > 1e-5 {
        return Err(invalid("rotation", "quaternion must have unit length"));
    }
    let scale = numbers(node.get("scale"), [1.; 3], "scale")?;
    let translation = numbers(node.get("translation"), [0.; 3], "translation")?;
    let mut m = [
        [
            1. - 2. * (y * y + z * z),
            2. * (x * y - z * w),
            2. * (x * z + y * w),
            translation[0],
        ],
        [
            2. * (x * y + z * w),
            1. - 2. * (x * x + z * z),
            2. * (y * z - x * w),
            translation[1],
        ],
        [
            2. * (x * z - y * w),
            2. * (y * z + x * w),
            1. - 2. * (x * x + y * y),
            translation[2],
        ],
        [0., 0., 0., 1.],
    ];
    for row in m.iter_mut().take(3) {
        for (cell, s) in row.iter_mut().zip(scale).take(3) {
            *cell *= s;
        }
    }
    Ok(m)
}

fn preserve_metadata(
    doc: &JsonValue,
    node_ids: &[String],
    meshes: &[Vec<(String, Option<usize>)>],
    scene: &mut Scene,
) -> Result<(), SceneError> {
    fn original(value: &JsonValue, path: String) -> Metadata {
        if let Some(stored) = value.pointer("/extras/usd_toolbox_metadata")
            && let Ok(metadata) = serde_json::from_value::<Metadata>(stored.clone())
        {
            return metadata;
        }
        let mut metadata = Metadata::default();
        if let Some(extras) = value.get("extras") {
            let mut property = MetadataProperty::authored(MetadataValue::from_json(extras.clone()));
            property.source = Some(MetadataSource {
                format: "gltf".into(),
                path,
                property: Some("extras".into()),
                value_type: Some("json".into()),
            });
            metadata.original.insert("gltf:extras".into(), property);
        }
        metadata
    }
    scene.metadata.scene = if doc["asset"].pointer("/extras/usd_toolbox_metadata").is_some() {
        original(&doc["asset"], "/asset".into())
    } else {
        original(doc, "/".into())
    };
    for (i, node) in array(doc, "nodes")?.iter().enumerate() {
        let id = node_ids[i].clone();
        if scene.nodes.contains_key(&id) {
            scene.metadata.nodes.insert(id, original(node, format!("/nodes/{i}")));
        }
    }
    for (i, mesh) in array(doc, "meshes")?.iter().enumerate() {
        for (p, primitive) in array(mesh, "primitives")?.iter().enumerate() {
            let mut metadata = original(mesh, format!("/meshes/{i}"));
            for (key, value) in original(primitive, format!("/meshes/{i}/primitives/{p}")).original {
                metadata.original.insert(format!("primitive:{key}"), value);
            }
            scene.metadata.meshes.insert(meshes[i][p].0.clone(), metadata);
        }
    }
    for (i, material) in array(doc, "materials")?.iter().enumerate() {
        let mut metadata = original(material, format!("/materials/{i}"));
        metadata
            .original
            .entry("gltf:material".into())
            .or_insert_with(|| MetadataProperty::authored(MetadataValue::from_json(material.clone())));
        metadata.original.insert(
            "gltf:render_settings".into(),
            MetadataProperty::authored(MetadataValue::from_json(materials::render_settings(doc, material)?)),
        );
        scene
            .metadata
            .materials
            .insert(scene.materials[i].id.0.clone(), metadata);
    }
    Ok(())
}

/// Stable, discoverable format profile. No filesystem, browser globals or native
/// libraries are required by these operations.
pub fn gltf_scene_capabilities() -> JsonValue {
    json!({"schema":"usd-toolbox.gltf-capabilities.v1","import_formats":["glb","gltf"],
        "export_formats":["glb","gltf"],"portable":true,"dependency_resolution":"caller_supplied_confined_bundle",
        "features":["static_hierarchy","shared_meshes","triangles","triangle_strips","triangle_fans","sparse_accessors","interleaved_accessors","normalized_attributes","indexed_export","authored_tangents","typed_extras_metadata","pbr_metallic_roughness","png","jpeg","alpha_mask","texture_samplers","texture_uv_sets","texture_transforms"],
        "extensions":EXTENSIONS,"unsupported":["animation","skinning","morph_targets","cameras","lights","draco","meshopt","ktx2","basisu","exr","hdr","tiff","udim_assembly"],
        "texture_preview":{"tier_is_maximum_edge":true,"upscaling":false,"filter":"pixel_area","srgb_filtering":"linear_light","data_filtering":"independent_linear_channels","normal_filtering":"renormalized_signed_vectors","alpha_filtering":"linear_with_colour_weighting_for_mask_and_blend","output":"rgba8_png","source_images_retained":true},
        "vertex_cache_ordering":{"option":"optimize_vertex_cache","default":false,"scope":"opaque_nontransmissive_primitives","algorithm":"bounded_adjacency_walk_and_first_use_remap","acceptance":"fifo16_and_fifo32_nonregression_with_one_strict_improvement","preserves":["render_vertex_attribute_bits","triangle_winding","material_boundaries","transparent_triangle_order"],"budget_exhaustion":"keep_original_order"},
        "settings_projection":"alpha/sampler/UV-transform/normal-scale metadata is evaluated by scene GLB export; other spokes must report projection loss",
        "preflight":{"geometry_decoding":false,"image_decoding":false,"max_diagnostics":1000},
        "metadata_schema":METADATA_SCHEMA,"limits":SceneLimits::default()})
}

/// Inspect authored structure without resolving buffers or decoding images. This
/// is compatibility preflight, not a replacement for complete import validation.
/// Recoverable issues accumulate; the bounded scan reports incomplete work.
pub fn preflight_scene(bytes: &[u8], source_name: &str, limits: &SceneLimits) -> Result<JsonValue, SceneError> {
    safe_path(source_name)?;
    let parsed = parse(bytes, limits)?;
    let doc = &parsed.document;
    let mut diagnostics = Vec::new();
    let mut complete = true;
    let max_work = limits
        .max_nodes
        .saturating_mul(32)
        .saturating_add(limits.max_dependencies.saturating_mul(8))
        .saturating_add(1024);
    let mut visited = 0usize;
    let mut pending = vec![(String::new(), doc)];
    let mut extensions = BTreeSet::new();
    let mut dependencies = BTreeSet::new();
    let mut declared_buffer_bytes = 0usize;
    let mut declared_points = 0usize;
    while let Some((path, value)) = pending.pop() {
        visited += 1;
        if visited > max_work || diagnostics.len() >= 1000 {
            complete = false;
            break;
        }
        match value {
            JsonValue::Object(map) => {
                if let Some(ext) = map.get("extensions").and_then(JsonValue::as_object) {
                    for name in ext.keys() {
                        extensions.insert(name.clone());
                        if !EXTENSIONS.contains(&name.as_str())
                            || !path.starts_with("/materials/")
                            || (name == "KHR_texture_transform" && !path.ends_with("Texture"))
                        {
                            diagnostics.push(json!({"code":"gltf.unsupported_extension","severity":"error","path":format!("{path}/extensions/{name}"),"detail":format!("extension {name} is outside the static import profile"),"remedy":"Bake or remove this feature in the authoring application; retain the original source."}));
                        }
                    }
                }
                if path.starts_with("/meshes/") && map.contains_key("targets") {
                    diagnostics.push(json!({"code":"gltf.morph_targets","severity":"error","path":format!("{path}/targets"),"detail":"morph targets require an unsupported deformation model","remedy":"Export a requested static pose."}));
                }
                if path.starts_with("/meshes/")
                    && map.contains_key("attributes")
                    && !matches!(optional(value, "mode", 4), Ok(4..=6))
                {
                    diagnostics.push(json!({"code":"gltf.primitive_mode","severity":"error","path":format!("{path}/mode"),"detail":"only triangle, strip and fan primitives are supported","remedy":"Convert points or curves into polygon geometry."}));
                }
                if path.starts_with("/nodes/") {
                    for key in ["skin", "weights", "camera"] {
                        if map.contains_key(key) {
                            diagnostics.push(json!({"code":format!("gltf.{key}"),"severity":"error","path":format!("{path}/{key}"),"detail":"feature is outside the static polygon profile","remedy":"Bake a static polygon asset or omit the unsupported object explicitly."}));
                        }
                    }
                }
                for (key, child) in map {
                    // Metadata may be arbitrarily structured; it does not contain glTF schema objects.
                    if key != "extras" {
                        pending.push((format!("{path}/{key}"), child));
                    }
                }
            }
            JsonValue::Array(items) => {
                for (i, child) in items.iter().enumerate() {
                    pending.push((format!("{path}/{i}"), child));
                }
            }
            _ => {}
        }
    }
    for key in ["animations", "skins", "cameras"] {
        if let Ok(items) = array(doc, key)
            && !items.is_empty()
        {
            diagnostics.push(json!({"code":format!("gltf.{key}"),"severity":"error","path":format!("/{key}"),"detail":format!("{} {key} definitions require unsupported features",items.len()),"remedy":"Export the intended static polygon representation."}));
        }
    }
    for key in ["extensionsUsed", "extensionsRequired"] {
        for ext in array(doc, key)? {
            let name = ext
                .as_str()
                .ok_or_else(|| invalid(key, "extension name must be a string"))?;
            if extensions.insert(name.into()) && !EXTENSIONS.contains(&name) {
                diagnostics.push(json!({"code":"gltf.unsupported_extension","severity":"error","path":format!("/{key}"),"detail":format!("unsupported extension {name}"),"remedy":"Export without this extension or decode it using a capable authoring tool."}));
            }
        }
    }
    for kind in ["buffers", "images"] {
        for (i, item) in array(doc, kind)?.iter().enumerate() {
            if kind == "buffers" {
                declared_buffer_bytes = declared_buffer_bytes.saturating_add(optional(item, "byteLength", 0)?);
            }
            if let Some(uri) = item.get("uri").and_then(JsonValue::as_str)
                && !uri.starts_with("data:")
            {
                match dependency_path(source_name,uri) {
                    Ok(path)=> {dependencies.insert(path);},
                    Err(e)=>diagnostics.push(json!({"code":e.code,"severity":"error","path":format!("/{kind}/{i}/uri"),"detail":e.detail,"remedy":"Supply confined relative dependencies in the host bundle."})),
                }
            }
            if dependencies.len() > limits.max_dependencies {
                complete = false;
                break;
            }
        }
    }
    for mesh in array(doc, "meshes")? {
        for primitive in array(mesh, "primitives")? {
            if let Some(i) = primitive.pointer("/attributes/POSITION").and_then(JsonValue::as_u64) {
                if let Some(accessor) = array(doc, "accessors")?.get(i as usize) {
                    declared_points = declared_points.saturating_add(optional(accessor, "count", 0)?);
                } else {
                    diagnostics.push(json!({"code":"gltf.invalid","severity":"error","path":"/meshes","detail":"POSITION accessor index out of bounds","remedy":"Repair the malformed glTF before importing."}));
                }
            }
        }
    }
    if declared_buffer_bytes > limits.max_bytes
        || declared_points > limits.max_points
        || array(doc, "nodes")?.len() > limits.max_nodes
    {
        diagnostics.push(json!({"code":"scene.limit","severity":"error","path":"/","detail":"declared resource totals exceed configured limits","remedy":"Split or reduce the asset; raise limits only after measuring the target runtime."}));
    }
    if diagnostics.len() > 1000 {
        diagnostics.truncate(1000);
        complete = false;
    }
    if !complete {
        diagnostics.push(json!({"code":"gltf.scan_limit","severity":"warning","path":"/","detail":"scan budget reached; findings and dependencies are incomplete","remedy":"Retry with an appropriate higher limit or inspect a smaller input."}));
    }
    diagnostics.sort_by_key(|d| format!("{}:{}:{}", d["path"], d["code"], d["detail"]));
    diagnostics.dedup();
    Ok(
        json!({"schema":"usd-toolbox.gltf-preflight.v1","format":if bytes.starts_with(b"glTF") {"glb"} else {"gltf"},"source_name":source_name,
        "complete":complete,"compatible":complete && !diagnostics.iter().any(|d|d["severity"]=="error"),"diagnostics":diagnostics,
        "counts":{"nodes":array(doc,"nodes")?.len(),"meshes":array(doc,"meshes")?.len(),"materials":array(doc,"materials")?.len(),"images":array(doc,"images")?.len(),"declared_points":declared_points},
        "resource_estimates":{"encoded_input_bytes":bytes.len(),"declared_buffer_bytes":declared_buffer_bytes,"decoded_geometry_bytes":null,"decoded_image_bytes":null},
        "extensions":extensions,"dependencies":dependencies,"metadata":{"document_extras_present":doc.get("extras").is_some(),"document_extra_keys":doc["extras"].as_object().map(|v|v.keys().take(100).collect::<Vec<_>>()),"asset_version":doc["asset"]["version"]},
        "validation":{"json":"performed","glb_envelope":"performed_if_present","buffers":"deferred","geometry":"deferred","images":"deferred","integrity":"deferred"},
        "note":"Authored-structure preflight does not establish that geometry, images, graph references or output targets are valid."}),
    )
}
