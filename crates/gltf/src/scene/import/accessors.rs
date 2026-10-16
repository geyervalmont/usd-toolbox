use super::*;

struct Decoded {
    values: Vec<f64>,
    components: usize,
    component_type: usize,
    normalized: bool,
}
impl Reader<'_> {
    pub(super) fn validate_views(&self) -> Result<(), SceneError> {
        for (i, view) in array(self.document, "bufferViews")?.iter().enumerate() {
            let buffer = required(view, "buffer")?;
            let offset = optional(view, "byteOffset", 0)?;
            let length = required(view, "byteLength")?;
            let data = self
                .buffers
                .get(buffer)
                .ok_or_else(|| invalid(format!("bufferViews/{i}"), "buffer index is out of bounds"))?;
            if length == 0 || offset.checked_add(length).is_none_or(|end| end > data.len()) {
                return Err(invalid(format!("bufferViews/{i}"), "view exceeds buffer bounds"));
            }
            if let Some(stride) = view.get("byteStride") {
                let stride = index(stride, "byteStride")?;
                if !(4..=252).contains(&stride) || stride % 4 != 0 {
                    return Err(invalid(
                        "byteStride",
                        "stride must be a multiple of four between 4 and 252",
                    ));
                }
            }
            if view.get("target").is_some_and(|v| v != 34962 && v != 34963) {
                return Err(invalid("bufferView/target", "invalid buffer view target"));
            }
        }
        Ok(())
    }
    pub(super) fn view(&self, i: usize) -> Result<(&[u8], &JsonValue), SceneError> {
        let view = array(self.document, "bufferViews")?
            .get(i)
            .ok_or_else(|| invalid("bufferView", "buffer view index is out of bounds"))?;
        let buffer = required(view, "buffer")?;
        let offset = optional(view, "byteOffset", 0)?;
        let length = required(view, "byteLength")?;
        let data = self
            .buffers
            .get(buffer)
            .and_then(|b| b.get(offset..offset.checked_add(length)?))
            .ok_or_else(|| invalid("bufferView", "view exceeds its buffer"))?;
        Ok((data, view))
    }
    fn accessor(&mut self, i: usize) -> Result<Decoded, SceneError> {
        let a = array(self.document, "accessors")?
            .get(i)
            .ok_or_else(|| invalid("accessor", "accessor index is out of bounds"))?;
        let components = match a["type"].as_str() {
            Some("SCALAR") => 1,
            Some("VEC2") => 2,
            Some("VEC3") => 3,
            Some("VEC4") => 4,
            _ => {
                return Err(unsupported(
                    format!("accessors/{i}/type"),
                    "only scalar and vector geometry accessors are supported",
                ));
            }
        };
        let count = required(a, "count")?;
        let component_type = required(a, "componentType")?;
        let width = component_width(component_type)?;
        let normalized = bool_value(a, "normalized", false)?;
        if normalized && matches!(component_type, 5125 | 5126) {
            return Err(invalid(
                "normalized",
                "float and unsigned-int components cannot be normalized",
            ));
        }
        let length = count.checked_mul(components).ok_or_else(|| limit("accessor"))?;
        if count == 0 || count > self.limits.max_corners.max(self.limits.max_points) {
            return Err(limit("accessor/count"));
        }
        self.charge(length.saturating_mul(8), "accessor")?;
        let mut values = vec![0.; length];
        let offset = optional(a, "byteOffset", 0)?;
        if let Some(v) = a.get("bufferView") {
            let (bytes, view) = self.view(index(v, "bufferView")?)?;
            let stride = optional(view, "byteStride", width * components)?;
            if stride < width * components
                || !stride.is_multiple_of(width)
                || optional(view, "byteOffset", 0)?
                    .checked_add(offset)
                    .is_none_or(|n| !n.is_multiple_of(width))
            {
                return Err(invalid("accessor", "invalid component alignment or stride"));
            }
            read_values(
                bytes,
                offset,
                stride,
                component_type,
                normalized,
                components,
                count,
                &mut values,
            )?;
        } else if offset != 0 {
            return Err(invalid("byteOffset", "accessor without a view must have zero offset"));
        }
        if let Some(sparse) = a.get("sparse") {
            let sparse_count = required(sparse, "count")?;
            if sparse_count == 0 || sparse_count > count {
                return Err(invalid("sparse/count", "sparse count is outside accessor bounds"));
            }
            let indices = &sparse["indices"];
            let kind = required(indices, "componentType")?;
            if !matches!(kind, 5121 | 5123 | 5125) {
                return Err(invalid(
                    "sparse/indices",
                    "sparse indices require unsigned integer components",
                ));
            }
            let width_i = component_width(kind)?;
            let (index_bytes, index_view) = self.view(required(indices, "bufferView")?)?;
            let index_offset = optional(indices, "byteOffset", 0)?;
            let sparse_values = &sparse["values"];
            let (value_bytes, value_view) = self.view(required(sparse_values, "bufferView")?)?;
            let value_offset = optional(sparse_values, "byteOffset", 0)?;
            if index_view.get("byteStride").is_some()
                || value_view.get("byteStride").is_some()
                || index_view.get("target").is_some()
                || value_view.get("target").is_some()
            {
                return Err(invalid("sparse", "sparse views must not specify a stride or target"));
            }
            if optional(index_view, "byteOffset", 0)?
                .checked_add(index_offset)
                .is_none_or(|n| !n.is_multiple_of(width_i))
                || optional(value_view, "byteOffset", 0)?
                    .checked_add(value_offset)
                    .is_none_or(|n| !n.is_multiple_of(width))
            {
                return Err(invalid("sparse", "misaligned sparse data"));
            }
            check_range(index_bytes, index_offset, width_i, width_i, sparse_count)?;
            check_range(
                value_bytes,
                value_offset,
                width * components,
                width * components,
                sparse_count,
            )?;
            let mut previous = None;
            for entry in 0..sparse_count {
                let start = index_offset + entry * width_i;
                let target = component(&index_bytes[start..start + width_i], kind, false) as usize;
                if target >= count || previous.is_some_and(|v| v >= target) {
                    return Err(invalid(
                        "sparse/indices",
                        "sparse indices must be in bounds and strictly increasing",
                    ));
                }
                previous = Some(target);
                let start = value_offset + entry * width * components;
                for c in 0..components {
                    values[target * components + c] = component(
                        &value_bytes[start + c * width..start + (c + 1) * width],
                        component_type,
                        normalized,
                    );
                }
            }
        } else if a.get("bufferView").is_none() {
            return Err(invalid("accessor", "accessor needs a bufferView or sparse values"));
        }
        if values.iter().any(|v| !v.is_finite()) {
            return Err(invalid("accessor", "NaN and infinity are prohibited"));
        }
        for key in ["min", "max"] {
            if let Some(bounds) = a.get(key) {
                let bounds = bounds
                    .as_array()
                    .filter(|v| v.len() == components)
                    .ok_or_else(|| invalid(key, "accessor bound has wrong dimension"))?;
                for (c, bound) in bounds.iter().enumerate() {
                    let bound = bound
                        .as_f64()
                        .filter(|v| v.is_finite())
                        .ok_or_else(|| invalid(key, "non-finite accessor bound"))?;
                    // Bounds use the accessor's component representation. JSON's
                    // shortest float spelling need not equal the widened f32 value.
                    let bound = if normalized {
                        normalize_component(bound, component_type)
                    } else if component_type == 5126 {
                        f64::from(bound as f32)
                    } else {
                        bound
                    };
                    if values
                        .chunks_exact(components)
                        .any(|v| if key == "min" { v[c] < bound } else { v[c] > bound })
                    {
                        return Err(invalid(key, "authored accessor bounds do not contain decoded values"));
                    }
                }
            }
        }
        Ok(Decoded {
            values,
            components,
            component_type,
            normalized,
        })
    }
    pub(super) fn mesh(
        &mut self,
        primitive: &JsonValue,
        diagnostics: &mut Vec<SceneDiagnostic>,
    ) -> Result<Mesh, SceneError> {
        let attributes = primitive["attributes"]
            .as_object()
            .ok_or_else(|| invalid("attributes", "primitive requires vertex attributes"))?;
        let mut decoded = BTreeMap::new();
        for (semantic, value) in attributes {
            if !matches!(semantic.as_str(), "POSITION" | "NORMAL" | "TANGENT" | "COLOR_0")
                && !semantic.starts_with("TEXCOORD_")
            {
                return Err(unsupported(
                    semantic,
                    "unsupported vertex semantic; remove skinning or bake custom attributes explicitly",
                ));
            }
            decoded.insert(semantic.as_str(), self.accessor(index(value, semantic)?)?);
        }
        let position = decoded
            .remove("POSITION")
            .ok_or_else(|| invalid("POSITION", "primitive requires positions"))?;
        if position.components != 3 || position.component_type != 5126 {
            return Err(invalid("POSITION", "positions require floating-point VEC3 accessors"));
        }
        let count = position.values.len() / 3;
        if count > self.limits.max_points {
            return Err(limit("POSITION"));
        }
        let mut mesh = Mesh {
            positions: position
                .values
                .as_chunks::<3>()
                .0
                .iter()
                .map(|p| [p[0], p[1], p[2]])
                .collect(),
            ..Default::default()
        };
        for (semantic, a) in decoded {
            if a.values.len() / a.components != count {
                return Err(invalid(semantic, "vertex attribute count differs from POSITION"));
            }
            match semantic {
                "NORMAL" => {
                    if a.components != 3 || a.component_type != 5126 {
                        return Err(invalid(semantic, "normals require floating-point VEC3"));
                    }
                    let values = a
                        .values
                        .as_chunks::<3>()
                        .0
                        .iter()
                        .map(|v| [v[0], v[1], v[2]])
                        .collect::<Vec<_>>();
                    if values
                        .iter()
                        .any(|v| ((v.iter().map(|x| x * x).sum::<f64>()) - 1.).abs() > 1e-3)
                    {
                        return Err(invalid(semantic, "normals must be unit length"));
                    }
                    mesh.normals = Some(vertex(values));
                }
                "TANGENT" => {
                    if a.components != 4 || a.component_type != 5126 {
                        return Err(invalid(semantic, "tangents require floating-point VEC4"));
                    }
                    let values = a
                        .values
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .map(|v| [v[0] as f32, v[1] as f32, v[2] as f32, -v[3] as f32])
                        .collect::<Vec<_>>();
                    if values
                        .iter()
                        .any(|v| ((v[..3].iter().map(|x| x * x).sum::<f32>()) - 1.).abs() > 1e-3 || v[3].abs() != 1.)
                    {
                        return Err(invalid(
                            semantic,
                            "tangent xyz must be unit length and w must be -1 or 1",
                        ));
                    }
                    mesh.tangents = Some(vertex(values));
                }
                "COLOR_0" => {
                    if !matches!(a.components, 3 | 4)
                        || !(a.component_type == 5126 || (matches!(a.component_type, 5121 | 5123) && a.normalized))
                    {
                        return Err(invalid(
                            semantic,
                            "colors require floating-point or normalized unsigned VEC3/VEC4",
                        ));
                    }
                    if a.values.iter().any(|v| !(0. ..=1.).contains(v)) {
                        return Err(invalid(semantic, "vertex colors must be within zero and one"));
                    }
                    mesh.colors = Some(vertex(
                        a.values
                            .chunks_exact(a.components)
                            .map(|v| {
                                [
                                    v[0] as f32,
                                    v[1] as f32,
                                    v[2] as f32,
                                    v.get(3).copied().unwrap_or(1.) as f32,
                                ]
                            })
                            .collect(),
                    ));
                }
                uv => {
                    if a.components != 2
                        || !(a.component_type == 5126 || (matches!(a.component_type, 5121 | 5123) && a.normalized))
                    {
                        return Err(invalid(uv, "UVs require floating-point or normalized unsigned VEC2"));
                    }
                    let set = uv
                        .strip_prefix("TEXCOORD_")
                        .and_then(|n| n.parse::<usize>().ok())
                        .ok_or_else(|| invalid(uv, "invalid texture coordinate semantic"))?;
                    if set > 31 {
                        return Err(limit(uv));
                    }
                    mesh.uv_sets.insert(
                        if set == 0 { "st".into() } else { format!("st{set}") },
                        vertex(
                            a.values
                                .as_chunks::<2>()
                                .0
                                .iter()
                                .map(|v| [v[0] as f32, 1. - v[1] as f32])
                                .collect(),
                        ),
                    );
                }
            }
        }
        if mesh.tangents.is_some() && mesh.normals.is_none() {
            return Err(invalid("TANGENT", "tangents require normals"));
        }
        let indices = if let Some(a) = primitive.get("indices") {
            let a = self.accessor(index(a, "indices")?)?;
            if a.components != 1 || !matches!(a.component_type, 5121 | 5123 | 5125) || a.normalized {
                return Err(invalid("indices", "indices require unsigned, non-normalized scalars"));
            }
            let forbidden = match a.component_type {
                5121 => 255.,
                5123 => 65535.,
                _ => 4294967295.,
            };
            if a.values.contains(&forbidden) {
                return Err(invalid(
                    "indices",
                    "primitive restart index values are forbidden by glTF",
                ));
            }
            a.values.iter().map(|&v| v as u32).collect::<Vec<_>>()
        } else {
            (0..count as u32).collect()
        };
        if indices.iter().any(|&v| v as usize >= count) {
            return Err(invalid("indices", "index exceeds vertex count"));
        }
        let mode = optional(primitive, "mode", 4)?;
        let faces = match mode {
            4 => {
                if indices.len() % 3 != 0 {
                    return Err(invalid("indices", "triangle index count must be divisible by three"));
                }
                indices.len() / 3
            }
            5 | 6 => indices.len().saturating_sub(2),
            _ => {
                return Err(unsupported(
                    "mode",
                    "only triangles, triangle strips and triangle fans are supported",
                ));
            }
        };
        if faces == 0 || faces > self.limits.max_faces || faces.saturating_mul(3) > self.limits.max_corners {
            return Err(limit("triangles"));
        }
        self.charge(faces.saturating_mul(16), "triangles")?;
        match mode {
            4 => mesh.face_indices = indices,
            5 => {
                for i in 0..faces {
                    let mut tri = [indices[i], indices[i + 1], indices[i + 2]];
                    if i % 2 == 1 {
                        tri.swap(0, 1);
                    }
                    mesh.face_indices.extend(tri);
                }
            }
            6 => {
                for i in 0..faces {
                    mesh.face_indices.extend([indices[0], indices[i + 1], indices[i + 2]]);
                }
            }
            _ => unreachable!(),
        }
        // Degenerate strip connectors carry no area and are intentionally omitted.
        let before = mesh.face_indices.len();
        let mut triangles = Vec::with_capacity(before);
        for t in mesh.face_indices.as_chunks::<3>().0.iter() {
            if t[0] != t[1] && t[1] != t[2] && t[0] != t[2] {
                triangles.extend_from_slice(t);
            }
        }
        if triangles.len() != before {
            diagnostics.push(SceneDiagnostic::warning(
                "gltf.degenerate_triangles",
                "meshes",
                "removed repeated-index degenerate triangles",
            ));
        }
        mesh.face_indices = triangles;
        mesh.face_counts = vec![3; mesh.face_indices.len() / 3];
        if mesh.face_counts.is_empty() {
            return Err(invalid("meshes", "primitive contains no nondegenerate triangle"));
        }
        if let Some(m) = primitive.get("material") {
            let material = array(self.document, "materials")?
                .get(index(m, "material")?)
                .ok_or_else(|| invalid("material", "material index out of bounds"))?;
            mesh.double_sided = bool_value(material, "doubleSided", false)?;
        }
        Ok(mesh)
    }
}
fn vertex<T>(values: Vec<T>) -> Attribute<T> {
    Attribute {
        interpolation: Interpolation::Vertex,
        values,
        indices: None,
    }
}
fn component_width(kind: usize) -> Result<usize, SceneError> {
    match kind {
        5120 | 5121 => Ok(1),
        5122 | 5123 => Ok(2),
        5125 | 5126 => Ok(4),
        _ => Err(invalid("componentType", "unsupported accessor component type")),
    }
}
fn normalize_component(value: f64, kind: usize) -> f64 {
    match kind {
        5120 => (value / 127.).max(-1.),
        5121 => value / 255.,
        5122 => (value / 32767.).max(-1.),
        5123 => value / 65535.,
        _ => value,
    }
}
fn component(bytes: &[u8], kind: usize, normalized: bool) -> f64 {
    let value = match kind {
        5120 => f64::from(bytes[0] as i8),
        5121 => f64::from(bytes[0]),
        5122 => f64::from(i16::from_le_bytes(bytes.try_into().unwrap())),
        5123 => f64::from(u16::from_le_bytes(bytes.try_into().unwrap())),
        5125 => f64::from(u32::from_le_bytes(bytes.try_into().unwrap())),
        5126 => f64::from(f32::from_le_bytes(bytes.try_into().unwrap())),
        _ => unreachable!(),
    };
    if normalized {
        normalize_component(value, kind)
    } else {
        value
    }
}
fn check_range(bytes: &[u8], offset: usize, stride: usize, element: usize, count: usize) -> Result<(), SceneError> {
    let end = count
        .checked_sub(1)
        .and_then(|n| n.checked_mul(stride))
        .and_then(|n| offset.checked_add(n))
        .and_then(|n| n.checked_add(element));
    if end.is_none_or(|end| end > bytes.len()) {
        return Err(invalid("accessor", "accessor data exceeds its buffer view"));
    }
    Ok(())
}
#[allow(clippy::too_many_arguments)]
fn read_values(
    bytes: &[u8],
    offset: usize,
    stride: usize,
    kind: usize,
    normalized: bool,
    components: usize,
    count: usize,
    out: &mut [f64],
) -> Result<(), SceneError> {
    let width = component_width(kind)?;
    check_range(bytes, offset, stride, components * width, count)?;
    for i in 0..count {
        for c in 0..components {
            let start = offset + i * stride + c * width;
            out[i * components + c] = component(&bytes[start..start + width], kind, normalized);
        }
    }
    Ok(())
}
