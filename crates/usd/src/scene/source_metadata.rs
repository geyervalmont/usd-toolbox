//! Source metadata conversion. A tagged USD type is retained at every dictionary
//! leaf so `token`, `string`, vectors, arrays and wide integers do not collapse.
use super::*;
use usd_toolbox_core::scene::{
    Metadata as ObjectMetadata, MetadataProperty, MetadataSource, MetadataState, MetadataValue as M,
};

pub(super) fn is_metadata_field(data: &dyn AbstractData, path: &Path, field_name: &str) -> Result<bool, SceneError> {
    if path == &Path::abs_root() {
        return Ok(!matches!(
            field_name,
            "primChildren" | "defaultPrim" | "metersPerUnit" | "upAxis" | "subLayers" | "subLayerOffsets"
        ));
    }
    Ok(matches!(
        field_name,
        "assetInfo" | "customData" | "kind" | "documentation" | "displayName" | "hidden"
    ) || (path.is_property_path()
        && field_name == "default"
        && field(data, path, "custom")? == Some(UsdValue::Bool(true))))
}

pub(super) fn property(
    path: &Path,
    field_name: &str,
    type_name: &str,
    value: &UsdValue,
    maximum: usize,
) -> Result<MetadataProperty, SceneError> {
    let mut budget = maximum;
    let value = encode(value, &mut budget, 0).map_err(|mut e| {
        e.path = path.as_str().into();
        e
    })?;
    Ok(MetadataProperty {
        value,
        state: MetadataState::Authored,
        source: Some(MetadataSource {
            format: "usd".into(),
            path: path.as_str().into(),
            property: Some(field_name.into()),
            value_type: (!type_name.is_empty()).then(|| type_name.into()),
        }),
        unit: None,
    })
}

fn charge(budget: &mut usize, amount: usize) -> Result<(), SceneError> {
    *budget = budget
        .checked_sub(amount)
        .ok_or_else(|| err("usd.metadata_limit", "/", "metadata conversion budget exhausted"))?;
    Ok(())
}

fn typed(ty: &str, value: M) -> M {
    M::Typed {
        value_type: format!("usd:{ty}"),
        value: Box::new(value),
    }
}
fn array(ty: &str, values: Vec<M>) -> M {
    M::TypedArray {
        element_type: format!("usd:{ty}"),
        values,
    }
}
fn number(value: f64) -> Result<M, SceneError> {
    if value.is_finite() {
        Ok(M::F64(value))
    } else {
        Err(err(
            "usd.metadata_value",
            "/",
            "non-finite metadata is not portable JSON",
        ))
    }
}
fn floats<const N: usize>(values: [f64; N]) -> Result<M, SceneError> {
    Ok(M::Array(values.into_iter().map(number).collect::<Result<_, _>>()?))
}

fn encode(v: &UsdValue, budget: &mut usize, depth: usize) -> Result<M, SceneError> {
    if depth > 24 {
        return Err(err("usd.metadata_limit", "/", "metadata dictionary nesting exceeds 24"));
    }
    charge(budget, 64)?;
    macro_rules! vector {
        ($v:expr, $t:ty, $n:expr, $name:expr) => {{
            charge(budget, $n * 24)?;
            typed($name, floats(<[$t; $n]>::from(*$v).map(|v| v as f64))?)
        }};
    }
    macro_rules! vector_array {
        ($v:expr, $t:ty, $n:expr, $name:expr) => {{
            charge(budget, $v.len().saturating_mul($n * 24 + 16))?;
            array(
                $name,
                $v.iter()
                    .map(|v| floats(<[$t; $n]>::from(*v).map(|v| v as f64)))
                    .collect::<Result<_, _>>()?,
            )
        }};
    }
    macro_rules! scalar_array {
        ($v:expr,$name:expr,$map:expr) => {{
            charge(budget, $v.len().saturating_mul(32))?;
            array($name, $v.iter().map($map).collect())
        }};
    }
    Ok(match v {
        UsdValue::Bool(v) => typed("bool", M::Bool(*v)),
        UsdValue::Uchar(v) => typed("uchar", M::U64(v.to_string())),
        UsdValue::Int(v) => typed("int", M::I64(v.to_string())),
        UsdValue::Uint(v) => typed("uint", M::U64(v.to_string())),
        UsdValue::Int64(v) => typed("int64", M::I64(v.to_string())),
        UsdValue::Uint64(v) => typed("uint64", M::U64(v.to_string())),
        UsdValue::Float(v) => typed("float", number(f64::from(*v))?),
        UsdValue::Double(v) => typed("double", number(*v)?),
        UsdValue::String(v) => {
            charge(budget, v.len())?;
            typed("string", M::String(v.clone()))
        }
        UsdValue::Token(v) => {
            charge(budget, v.as_str().len())?;
            typed("token", M::String(v.to_string()))
        }
        UsdValue::AssetPath(v) => {
            charge(budget, v.authored_path.len())?;
            typed("asset", M::String(v.authored_path.clone()))
        }
        UsdValue::BoolVec(v) => scalar_array!(v, "bool", |v| M::Bool(*v)),
        UsdValue::UcharVec(v) => scalar_array!(v, "uchar", |v| M::U64(v.to_string())),
        UsdValue::IntVec(v) => scalar_array!(v, "int", |v| M::I64(v.to_string())),
        UsdValue::UintVec(v) => scalar_array!(v, "uint", |v| M::U64(v.to_string())),
        UsdValue::Int64Vec(v) => scalar_array!(v, "int64", |v| M::I64(v.to_string())),
        UsdValue::Uint64Vec(v) => scalar_array!(v, "uint64", |v| M::U64(v.to_string())),
        UsdValue::FloatVec(v) => {
            charge(budget, v.len().saturating_mul(24))?;
            array(
                "float",
                v.iter().map(|v| number(f64::from(*v))).collect::<Result<_, _>>()?,
            )
        }
        UsdValue::DoubleVec(v) => {
            charge(budget, v.len().saturating_mul(24))?;
            array("double", v.iter().map(|v| number(*v)).collect::<Result<_, _>>()?)
        }
        UsdValue::StringVec(v) => {
            charge(budget, v.iter().map(|v| v.len().saturating_add(24)).sum())?;
            array("string", v.iter().cloned().map(M::String).collect())
        }
        UsdValue::TokenVec(v) => {
            charge(budget, v.iter().map(|v| v.as_str().len().saturating_add(24)).sum())?;
            array("token", v.iter().map(|v| M::String(v.to_string())).collect())
        }
        UsdValue::AssetPathVec(v) => {
            charge(budget, v.iter().map(|v| v.authored_path.len().saturating_add(24)).sum())?;
            array("asset", v.iter().map(|v| M::String(v.authored_path.clone())).collect())
        }
        UsdValue::Vec2f(v) => vector!(v, f32, 2, "float2"),
        UsdValue::Vec3f(v) => vector!(v, f32, 3, "float3"),
        UsdValue::Vec4f(v) => vector!(v, f32, 4, "float4"),
        UsdValue::Vec2d(v) => vector!(v, f64, 2, "double2"),
        UsdValue::Vec3d(v) => vector!(v, f64, 3, "double3"),
        UsdValue::Vec4d(v) => vector!(v, f64, 4, "double4"),
        UsdValue::Vec2i(v) => vector!(v, i32, 2, "int2"),
        UsdValue::Vec3i(v) => vector!(v, i32, 3, "int3"),
        UsdValue::Vec4i(v) => vector!(v, i32, 4, "int4"),
        UsdValue::Vec2fVec(v) => vector_array!(v, f32, 2, "float2"),
        UsdValue::Vec3fVec(v) => vector_array!(v, f32, 3, "float3"),
        UsdValue::Vec4fVec(v) => vector_array!(v, f32, 4, "float4"),
        UsdValue::Vec2dVec(v) => vector_array!(v, f64, 2, "double2"),
        UsdValue::Vec3dVec(v) => vector_array!(v, f64, 3, "double3"),
        UsdValue::Vec4dVec(v) => vector_array!(v, f64, 4, "double4"),
        UsdValue::Vec2iVec(v) => vector_array!(v, i32, 2, "int2"),
        UsdValue::Vec3iVec(v) => vector_array!(v, i32, 3, "int3"),
        UsdValue::Vec4iVec(v) => vector_array!(v, i32, 4, "int4"),
        UsdValue::Matrix4d(v) => {
            charge(budget, 16 * 24)?;
            typed("matrix4d", floats(v.0)?)
        }
        UsdValue::Matrix3d(v) => {
            charge(budget, 9 * 24)?;
            typed("matrix3d", floats(v.0)?)
        }
        UsdValue::Matrix2d(v) => {
            charge(budget, 4 * 24)?;
            typed("matrix2d", floats(v.0)?)
        }
        UsdValue::Dictionary(v) => {
            let mut result = BTreeMap::new();
            for (key, value) in v {
                charge(budget, key.len())?;
                result.insert(key.clone(), encode(value, budget, depth + 1)?);
            }
            M::Dictionary(result)
        }
        UsdValue::PathListOp(v) => {
            let mut result = BTreeMap::from([("explicit".into(), M::Bool(v.explicit))]);
            for (name, items) in [
                ("explicit_items", &v.explicit_items),
                ("added_items", &v.added_items),
                ("prepended_items", &v.prepended_items),
                ("appended_items", &v.appended_items),
                ("deleted_items", &v.deleted_items),
                ("ordered_items", &v.ordered_items),
            ] {
                charge(budget, items.iter().map(|p| p.as_str().len() + 24).sum())?;
                result.insert(
                    name.into(),
                    M::TypedArray {
                        element_type: "usd:path".into(),
                        values: items.iter().map(|p| M::String(p.as_str().into())).collect(),
                    },
                );
            }
            typed("path_list_op", M::Dictionary(result))
        }
        UsdValue::Variability(v) => typed("variability", M::String(format!("{v:?}"))),
        UsdValue::Specifier(v) => typed("specifier", M::String(format!("{v:?}"))),
        UsdValue::ValueBlock => typed("block", M::Null),
        _ => {
            return Err(err(
                "usd.metadata_type",
                "/",
                format!("unsupported metadata value type {}", sdf::ValueKind::from(v)),
            ));
        }
    })
}

fn decode(v: &M) -> Result<UsdValue, SceneError> {
    let invalid = || err("usd.metadata_value", "/", "invalid typed USD metadata");
    match v {
        M::Dictionary(v) => Ok(UsdValue::Dictionary(
            v.iter()
                .map(|(k, v)| Ok((k.clone(), decode(v)?)))
                .collect::<Result<_, SceneError>>()?,
        )),
        M::Typed { value_type, value } => decode_scalar(value_type.strip_prefix("usd:").ok_or_else(invalid)?, value),
        M::TypedArray { element_type, values } => {
            let kind = element_type.strip_prefix("usd:").ok_or_else(invalid)?;
            let mut decoded = values
                .iter()
                .map(|v| decode_scalar(kind, v))
                .collect::<Result<Vec<_>, _>>()?;
            macro_rules! packed {
                ($scalar:ident,$array:ident) => {{
                    UsdValue::$array(
                        decoded
                            .drain(..)
                            .map(|v| match v {
                                UsdValue::$scalar(v) => Ok(v),
                                _ => Err(invalid()),
                            })
                            .collect::<Result<_, _>>()?,
                    )
                }};
            }
            Ok(match kind {
                "bool" => packed!(Bool, BoolVec),
                "uchar" => packed!(Uchar, UcharVec),
                "int" => packed!(Int, IntVec),
                "uint" => packed!(Uint, UintVec),
                "int64" => packed!(Int64, Int64Vec),
                "uint64" => packed!(Uint64, Uint64Vec),
                "float" => packed!(Float, FloatVec),
                "double" => packed!(Double, DoubleVec),
                "string" => packed!(String, StringVec),
                "token" => packed!(Token, TokenVec),
                "asset" => packed!(AssetPath, AssetPathVec),
                "float2" => packed!(Vec2f, Vec2fVec),
                "float3" => packed!(Vec3f, Vec3fVec),
                "float4" => packed!(Vec4f, Vec4fVec),
                "double2" => packed!(Vec2d, Vec2dVec),
                "double3" => packed!(Vec3d, Vec3dVec),
                "double4" => packed!(Vec4d, Vec4dVec),
                "int2" => packed!(Vec2i, Vec2iVec),
                "int3" => packed!(Vec3i, Vec3iVec),
                "int4" => packed!(Vec4i, Vec4iVec),
                _ => return Err(invalid()),
            })
        }
        _ => Err(invalid()),
    }
}

fn decode_scalar(kind: &str, value: &M) -> Result<UsdValue, SceneError> {
    let invalid = || err("usd.metadata_value", "/", format!("invalid USD {kind} value"));
    let s = || match value {
        M::String(v) | M::I64(v) | M::U64(v) => Ok(v.as_str()),
        _ => Err(invalid()),
    };
    let f = || match value {
        M::F64(v) if v.is_finite() => Ok(*v),
        _ => Err(invalid()),
    };
    macro_rules! integer {
        ($variant:ident,$ty:ty) => {
            UsdValue::$variant(s()?.parse::<$ty>().map_err(|_| invalid())?)
        };
    }
    macro_rules! vector {
        ($variant:ident,$n:expr,$ty:ty) => {{
            let v = components::<$n>(value)?;
            if v.iter().any(|v| !(*v as $ty as f64).is_finite()) {
                return Err(invalid());
            }
            UsdValue::$variant(v.map(|v| v as $ty).into())
        }};
    }
    Ok(match kind {
        "bool" => UsdValue::Bool(match value {
            M::Bool(v) => *v,
            _ => return Err(invalid()),
        }),
        "uchar" => integer!(Uchar, u8),
        "int" => integer!(Int, i32),
        "uint" => integer!(Uint, u32),
        "int64" => integer!(Int64, i64),
        "uint64" => integer!(Uint64, u64),
        "float" => {
            let v = f()? as f32;
            if !v.is_finite() {
                return Err(invalid());
            }
            UsdValue::Float(v)
        }
        "double" => UsdValue::Double(f()?),
        "string" => UsdValue::String(s()?.into()),
        "token" => UsdValue::Token(s()?.into()),
        "asset" => UsdValue::AssetPath(AssetPath::new(s()?)),
        "float2" => vector!(Vec2f, 2, f32),
        "float3" => vector!(Vec3f, 3, f32),
        "float4" => vector!(Vec4f, 4, f32),
        "double2" => vector!(Vec2d, 2, f64),
        "double3" => vector!(Vec3d, 3, f64),
        "double4" => vector!(Vec4d, 4, f64),
        "int2" => vector!(Vec2i, 2, i32),
        "int3" => vector!(Vec3i, 3, i32),
        "int4" => vector!(Vec4i, 4, i32),
        "matrix4d" => UsdValue::Matrix4d(gf::Matrix4d(components(value)?)),
        "matrix3d" => UsdValue::Matrix3d(gf::Mat3d(components(value)?)),
        "matrix2d" => UsdValue::Matrix2d(gf::Mat2d(components(value)?)),
        "block" if matches!(value, M::Null) => UsdValue::ValueBlock,
        _ => return Err(invalid()),
    })
}
fn components<const N: usize>(value: &M) -> Result<[f64; N], SceneError> {
    let M::Array(values) = value else {
        return Err(err("usd.metadata_value", "/", "expected vector components"));
    };
    let values = values
        .iter()
        .map(|v| match v {
            M::F64(v) if v.is_finite() => Ok(*v),
            _ => Err(err("usd.metadata_value", "/", "invalid vector component")),
        })
        .collect::<Result<Vec<_>, _>>()?;
    values
        .try_into()
        .map_err(|_| err("usd.metadata_value", "/", "incorrect vector size"))
}

pub(super) fn capture(
    data: &dyn AbstractData,
    paths: &[Path],
    node_ids: &BTreeMap<String, String>,
    material_ids: &BTreeMap<String, usd_toolbox_core::MaterialId>,
    diagnostics: &mut Vec<SceneDiagnostic>,
) -> Result<SceneMetadata, SceneError> {
    let mut metadata = SceneMetadata::default();
    let mut remaining = 8 * 1024 * 1024usize;
    for p in paths {
        let mut values = ObjectMetadata::default();
        for f in data.list_fields(p).unwrap_or_default() {
            if !is_metadata_field(data, p, f.as_str())? {
                continue;
            }
            if f.as_str() == "customLayerData"
                && let Some(UsdValue::Dictionary(d)) = field(data, p, f.as_str())?
                && d.contains_key("olsyn_scene")
            {
                continue;
            }
            let Some(v) = field(data, p, f.as_str())? else {
                continue;
            };
            let ty = token(field(data, p, "typeName")?).unwrap_or_default();
            let key = if p.is_property_path() {
                format!("usd:attribute:{}", property_name(p).unwrap_or(""))
            } else {
                format!("usd:{f}")
            };
            match property(p, f.as_str(), &ty, &v, remaining.min(1024 * 1024)) {
                Ok(prop) => {
                    let bytes = serde_json::to_vec(&prop)
                        .map_err(|e| err("usd.metadata", p.as_str(), e))?
                        .len();
                    remaining = remaining
                        .checked_sub(bytes)
                        .ok_or_else(|| err("usd.metadata_limit", p.as_str(), "total source metadata exceeds 8 MiB"))?;
                    values.original.insert(key, prop);
                }
                Err(error) => diagnostics.push(SceneDiagnostic::warning(
                    "usd.metadata_loss",
                    p.as_str(),
                    format!("{f}: {}", error.detail),
                )),
            }
        }
        if values.original.is_empty() {
            continue;
        }
        let prim = p.prim_path();
        if p == &Path::abs_root() {
            metadata.scene.original.extend(values.original);
        } else if let Some(id) = node_ids.get(prim.as_str()) {
            metadata
                .nodes
                .entry(id.clone())
                .or_default()
                .original
                .extend(values.original);
        } else if let Some(id) = material_ids.get(prim.as_str()) {
            metadata
                .materials
                .entry(id.0.clone())
                .or_default()
                .original
                .extend(values.original);
        } else {
            for (key, property) in values.original {
                diagnostics.push(SceneDiagnostic::warning(
                    "usd.metadata_loss",
                    p.as_str(),
                    format!("{key}: object is outside the projected scene; retained only in preflight/source"),
                ));
                let _ = property;
            }
        }
    }
    // Archive authored shading graphs independently of their preview projection.
    // Paths retain their original namespace. Reauthoring arbitrary graphs into
    // generated namespaces would require full USD connection/path rewriting.
    let mut graphs: BTreeMap<String, BTreeMap<String, M>> = BTreeMap::new();
    for p in paths {
        let mut ancestor = Some(p.prim_path());
        let mut owner = None;
        while let Some(path) = ancestor {
            if let Some(id) = material_ids.get(path.as_str()) {
                owner = Some(id.0.clone());
                break;
            }
            ancestor = path.parent();
        }
        let Some(owner) = owner else {
            continue;
        };
        let mut fields = BTreeMap::new();
        for f in data.list_fields(p).unwrap_or_default() {
            if matches!(f.as_str(), "primChildren" | "properties" | "propertyChildren") {
                continue;
            }
            if let Some(v) = field(data, p, f.as_str())? {
                match encode(&v, &mut remaining, 0) {
                    Ok(v) => {
                        fields.insert(f.to_string(), v);
                    }
                    Err(error) => diagnostics.push(SceneDiagnostic::warning(
                        "usd.graph_property_loss",
                        p.as_str(),
                        format!("{f}: {}", error.detail),
                    )),
                }
            }
        }
        graphs
            .entry(owner)
            .or_default()
            .insert(p.as_str().into(), M::Dictionary(fields));
    }
    for (id, graph) in graphs {
        metadata.materials.entry(id).or_default().original.insert(
            "usd:source_graph".into(),
            MetadataProperty::authored(M::Dictionary(graph)),
        );
    }
    Ok(metadata)
}

pub(super) fn author(
    data: &mut sdf::Data,
    scene: &Scene,
    paths: &BTreeMap<String, Path>,
    materials: &BTreeMap<String, Path>,
    diagnostics: &mut Vec<SceneDiagnostic>,
) -> Result<(), SceneError> {
    author_scope(data, &Path::abs_root(), &scene.metadata.scene, diagnostics)?;
    for (id, metadata) in &scene.metadata.nodes {
        if let Some(p) = paths.get(id) {
            author_scope(data, p, metadata, diagnostics)?;
        }
    }
    for (id, metadata) in &scene.metadata.materials {
        if let Some(p) = materials.get(id) {
            author_scope(data, p, metadata, diagnostics)?;
        }
    }
    for (node_id, node) in &scene.nodes {
        if let Some(mesh) = &node.mesh
            && let Some(metadata) = scene.metadata.meshes.get(mesh)
        {
            author_scope(data, &paths[node_id], metadata, diagnostics)?;
        }
    }
    Ok(())
}

fn author_scope(
    data: &mut sdf::Data,
    path: &Path,
    metadata: &ObjectMetadata,
    diagnostics: &mut Vec<SceneDiagnostic>,
) -> Result<(), SceneError> {
    for (key, property) in &metadata.original {
        let Some(name) = key.strip_prefix("usd:") else {
            if key.starts_with("gltf:") {
                diagnostics.push(SceneDiagnostic::warning(
                    "usd.material_source_projection",
                    path.as_str(),
                    format!(
                        "{key}: source settings retained in metadata; USD graph uses the neutral material projection"
                    ),
                ));
            }
            continue;
        };
        if name == "source_graph" {
            diagnostics.push(SceneDiagnostic::warning(
                "usd.graph_archived",
                path.as_str(),
                "original graph retained in typed metadata; exported graph is the supported material projection",
            ));
            continue;
        }
        if property.state == MetadataState::Unresolved {
            diagnostics.push(SceneDiagnostic::warning(
                "usd.metadata_unresolved",
                path.as_str(),
                format!("{key}: archived in manifest only"),
            ));
            continue;
        }
        let value = match decode(&property.value) {
            Ok(v) => v,
            Err(e) => {
                diagnostics.push(SceneDiagnostic::warning(
                    "usd.metadata_projection_loss",
                    path.as_str(),
                    format!("{key}: {}; retained in neutral manifest", e.detail),
                ));
                continue;
            }
        };
        if let Some(attribute) = name.strip_prefix("attribute:") {
            let target = path
                .append_property(attribute)
                .map_err(|e| err("usd.metadata_path", path.as_str(), e))?;
            if data.spec_type(&target).is_some() {
                diagnostics.push(SceneDiagnostic::warning(
                    "usd.metadata_collision",
                    target.as_str(),
                    "custom metadata conflicts with generated attribute; archived in manifest only",
                ));
                continue;
            }
            let Some(ty) = property.source.as_ref().and_then(|s| s.value_type.as_deref()) else {
                continue;
            };
            create_attribute(data, path, attribute, ty, Some(value), None, None)
                .map_err(|e| err("usd.metadata", path.as_str(), e))?;
            data.set_field(&target, "custom", UsdValue::Bool(true));
        } else if name == "customLayerData" || name == "customData" || name == "assetInfo" {
            let UsdValue::Dictionary(mut dictionary) = value else {
                return Err(err("usd.metadata_type", path.as_str(), "expected dictionary"));
            };
            if let Some(UsdValue::Dictionary(existing)) = field(data, path, name)? {
                for (k, v) in existing {
                    dictionary.insert(k, v);
                }
            }
            data.set_field(path, name, UsdValue::Dictionary(dictionary));
        } else if matches!(
            name,
            "kind"
                | "documentation"
                | "displayName"
                | "hidden"
                | "comment"
                | "startTimeCode"
                | "endTimeCode"
                | "timeCodesPerSecond"
                | "framesPerSecond"
        ) {
            data.set_field(path, name, value);
        } else {
            diagnostics.push(SceneDiagnostic::warning(
                "usd.metadata_projection_loss",
                path.as_str(),
                format!("{key}: retained in manifest only; not reauthored as a USD field"),
            ));
        }
    }
    Ok(())
}
