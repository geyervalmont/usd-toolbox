use super::*;
use usd_toolbox_core::{Channel, ColorSpace, MapRole, MaterialId, NormalConvention, TextureSource};

pub(super) fn import_materials(
    data: &dyn AbstractData,
    entries: &BTreeMap<String, Vec<u8>>,
    authored: bool,
    scene_paths: &[Path],
    diagnostics: &mut Vec<SceneDiagnostic>,
) -> Result<(Vec<Material>, BTreeMap<String, MaterialId>), SceneError> {
    let material_paths: Vec<_> = scene_paths
        .iter()
        .filter(|p| {
            data.spec_type(p) == Some(SpecType::Prim) && token_field(data, p, "typeName").as_deref() == Some("Material")
        })
        .cloned()
        .collect();
    if authored {
        let materials = manifest_from_layer(data).map_err(|e| err("usd.material_manifest", "/", e))?;
        let materials = if entries.is_empty() {
            materials
        } else {
            rehydrate(materials, entries, true).map_err(|e| err("usd.material_assets", "/", e))?
        };
        let mut ids = BTreeMap::new();
        for p in material_paths {
            let id = match field(data, &p, "assetInfo")? {
                Some(UsdValue::Dictionary(d)) => token(d.get("identifier").cloned()),
                _ => None,
            }
            .ok_or_else(|| err("usd.material_identity", p.as_str(), "missing identity"))?;
            if !materials.iter().any(|m| m.id.0 == id) {
                return Err(err(
                    "usd.material_identity",
                    p.as_str(),
                    "identity missing from manifest",
                ));
            }
            ids.insert(p.as_str().into(), MaterialId(id));
        }
        return Ok((materials, ids));
    }
    let mut properties: BTreeMap<Path, Vec<&Path>> = BTreeMap::new();
    for path in scene_paths {
        if path.is_property_path() {
            properties.entry(path.prim_path()).or_default().push(path);
        }
    }
    let mut materials = Vec::new();
    let mut ids = BTreeMap::new();
    for p in material_paths {
        let output = p
            .append_property("outputs:surface")
            .map_err(|e| err("usd.path", p.as_str(), e))?;
        let connections = paths(field(data, &output, "connectionPaths")?)?;
        if connections.len() != 1 {
            return Err(err(
                "usd.shader_unsupported",
                p.as_str(),
                "expected one universal surface connection",
            ));
        }
        let shader = match resolve_property(data, &connections[0])? {
            ResolvedProperty::Output(path) => path.prim_path(),
            ResolvedProperty::Constant(_) => {
                return Err(err(
                    "usd.shader_unsupported",
                    output.as_str(),
                    "surface connection resolves to a constant, not a shader",
                ));
            }
        };
        if token(value(data, &shader, "info:id")?).as_deref() != Some("UsdPreviewSurface") {
            return Err(err(
                "usd.shader_unsupported",
                shader.as_str(),
                "external scene profile supports UsdPreviewSurface",
            ));
        }
        let mut material = Material::new(p.as_str(), p.name().unwrap_or("Material"));
        material.model = ShadingModel::GltfPbr;
        // UsdPreviewSurface fallbacks differ from the neutral material defaults.
        material.surface.base_color = Value::from([0.18, 0.18, 0.18, 1.]);
        material.surface.specular_roughness = Value::from(0.5);
        material.surface.specular_ior = Some(Value::from(1.5));
        material.surface.coat_roughness = Some(Value::from(0.01));
        material
            .provenance
            .metadata
            .insert("usd_prim_path".into(), serde_json::json!(p.as_str()));
        for &property in properties.get(&shader).into_iter().flatten() {
            let Some(name) = property_name(property).and_then(|n| n.strip_prefix("inputs:")) else {
                continue;
            };
            let connections = paths(field(data, property, "connectionPaths")?)?;
            let role = match name {
                "diffuseColor" => Some(MapRole::BaseColor),
                "roughness" => Some(MapRole::Roughness),
                "metallic" => Some(MapRole::Metallic),
                "normal" => Some(MapRole::Normal),
                "emissiveColor" => Some(MapRole::Emissive),
                "opacity" => Some(MapRole::Opacity),
                "occlusion" => Some(MapRole::Ao),
                _ => None,
            };
            let mut constant = field(data, property, "default")?;
            let mut output = None;
            if !connections.is_empty() {
                if connections.len() != 1 {
                    return Err(err("usd.shader_unsupported", property.as_str(), "multiple connections"));
                }
                match resolve_property(data, &connections[0])? {
                    ResolvedProperty::Constant(value) => constant = Some(value),
                    ResolvedProperty::Output(path) => output = Some(path),
                }
            }
            if let Some(connection) = output {
                let role = role.ok_or_else(|| {
                    err(
                        "usd.shader_unsupported",
                        property.as_str(),
                        "connected input is not supported",
                    )
                })?;
                let texture = read_texture(data, &connection, role, entries, diagnostics)?;
                match role {
                    MapRole::BaseColor => material.surface.base_color = Value::Texture { texture },
                    MapRole::Roughness => material.surface.specular_roughness = Value::Texture { texture },
                    MapRole::Metallic => material.surface.base_metalness = Value::Texture { texture },
                    MapRole::Normal => material.geometry.normal = Some(Value::Texture { texture }),
                    MapRole::Emissive => material.surface.emission_color = Some(Value::Texture { texture }),
                    MapRole::Opacity => material.geometry.opacity = Some(Value::Texture { texture }),
                    MapRole::Ao => material.geometry.ambient_occlusion = Some(Value::Texture { texture }),
                    _ => unreachable!(),
                }
            } else if let Some(v) = constant {
                let is_color = matches!(name, "diffuseColor" | "emissiveColor" | "normal" | "specularColor");
                if (is_color && !matches!(v, UsdValue::Vec3f(_) | UsdValue::Vec3d(_)))
                    || (!is_color
                        && name != "opacityMode"
                        && !matches!(v, UsdValue::Float(_) | UsdValue::Double(_) | UsdValue::Int(_)))
                {
                    return Err(err(
                        "usd.shader_type",
                        property.as_str(),
                        "unexpected shader input type",
                    ));
                }
                match name {
                    "diffuseColor" | "roughness" | "metallic" | "emissiveColor" | "opacity" | "ior" => {
                        assign_usd_constant(&mut material, name, v)
                    }
                    "clearcoat" => assign_usd_constant(&mut material, "coat_weight", v),
                    "clearcoatRoughness" => assign_usd_constant(&mut material, "coat_roughness", v),
                    "occlusion" => assign_usd_constant(&mut material, "ambient_occlusion", v),
                    "opacityMode" if token(Some(v.clone())).as_deref() == Some("transparent") => {}
                    "specularColor" if v == UsdValue::Vec3f([0., 0., 0.].into()) => {}
                    "useSpecularWorkflow" if v == UsdValue::Int(0) => {}
                    "displacement" if v == UsdValue::Float(0.) || v == UsdValue::Double(0.) => {}
                    "opacityThreshold"
                        if matches!(v,UsdValue::Float(v) if v.is_finite() && v>=0.)
                            || matches!(v,UsdValue::Double(v) if v.is_finite() && v>=0.) => {}
                    "normal" if v == UsdValue::Vec3f([0., 0., 1.].into()) => {}
                    _ => {
                        return Err(err(
                            "usd.shader_unsupported",
                            property.as_str(),
                            "input cannot be represented by the supported material profile",
                        ));
                    }
                }
            }
        }
        ids.insert(p.as_str().into(), material.id.clone());
        materials.push(material);
    }
    Ok((materials, ids))
}

fn read_texture(
    data: &dyn AbstractData,
    connection: &Path,
    role: MapRole,
    entries: &BTreeMap<String, Vec<u8>>,
    diagnostics: &mut Vec<SceneDiagnostic>,
) -> Result<TextureRef, SceneError> {
    let p = connection.prim_path();
    if token(value(data, &p, "info:id")?).as_deref() != Some("UsdUVTexture") {
        return Err(err("usd.texture_graph", p.as_str(), "expected UsdUVTexture"));
    }
    let _ = texture_settings(data, &p)?;
    for (name, default, normal_default) in [
        ("inputs:scale", [1., 1., 1., 1.], [2., 2., 2., 1.]),
        ("inputs:bias", [0., 0., 0., 0.], [-1., -1., -1., 0.]),
    ] {
        if let Some(v) = input_value(data, &p, name)? {
            let expected = if role == MapRole::Normal {
                normal_default
            } else {
                default
            };
            if v != UsdValue::Vec4f(expected.into()) {
                return Err(err("usd.texture_transform", p.as_str(), format!("unsupported {name}")));
            }
        } else if role == MapRole::Normal {
            return Err(err(
                "usd.normal_encoding",
                p.as_str(),
                "normal texture requires explicit scale 2 and bias -1",
            ));
        }
    }
    let name = match input_value(data, &p, "inputs:file")? {
        Some(UsdValue::AssetPath(v)) => v.authored_path,
        _ => return Err(err("usd.texture_file", p.as_str(), "missing asset path")),
    };
    validate_entry_name(&name).map_err(|e| err("usd.dependency_path", &name, e))?;
    let bytes = entries
        .get(&name)
        .ok_or_else(|| err("usd.missing_dependency", &name, "texture bytes were not supplied"))?;
    let extension = path_extension(&name).to_ascii_lowercase();
    let media = match extension.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        _ => {
            return Err(err(
                "usd.texture_codec",
                &name,
                "only PNG/JPEG texture processing is supported",
            ));
        }
    };
    let color_space = match token(input_value(data, &p, "inputs:sourceColorSpace")?).as_deref() {
        Some("sRGB") => ColorSpace::Srgb,
        Some("raw") => ColorSpace::Raw,
        _ => {
            return Err(err(
                "usd.texture_color_space",
                p.as_str(),
                "declare sourceColorSpace as sRGB or raw",
            ));
        }
    };
    let channel = match property_name(connection) {
        Some("outputs:rgb") => Channel::Rgb,
        Some("outputs:r") => Channel::R,
        Some("outputs:g") => Channel::G,
        Some("outputs:b") => Channel::B,
        Some("outputs:a") => Channel::A,
        _ => return Err(err("usd.texture_channel", connection.as_str(), "unsupported output")),
    };
    if matches!(role, MapRole::BaseColor | MapRole::Normal | MapRole::Emissive) != (channel == Channel::Rgb) {
        return Err(err(
            "usd.texture_channel",
            connection.as_str(),
            "channel type does not match shader input",
        ));
    }
    diagnostics.push(SceneDiagnostic::warning(
        "usd.texture_tier_assigned",
        &name,
        "original image retained in preview tier; no resizing performed",
    ));
    Ok(TextureRef {
        role,
        tiers: BTreeMap::from([(
            Tier::Preview,
            TextureSource {
                hash: digest(bytes),
                extension,
                media_type: media.into(),
                width: None,
                height: None,
                bytes: bytes.clone(),
            },
        )]),
        color_space,
        channel,
        normal_convention: (role == MapRole::Normal).then_some(NormalConvention::OpenGl),
    })
}

// USD interface connections may forward through Material/NodeGraph inputs and
// outputs. Only a single typed source is resolved; cycles and ambiguous sources
// remain explicit errors. This does not evaluate arbitrary shader nodes.
enum ResolvedProperty {
    Constant(UsdValue),
    Output(Path),
}
fn resolve_property(data: &dyn AbstractData, start: &Path) -> Result<ResolvedProperty, SceneError> {
    let mut current = start.clone();
    let mut visited = BTreeSet::new();
    for _ in 0..64 {
        if !current.is_property_path() || !visited.insert(current.as_str().to_owned()) {
            return Err(err(
                "usd.shader_cycle",
                start.as_str(),
                "cyclic or invalid shader connection",
            ));
        }
        let connected = paths(field(data, &current, "connectionPaths")?)?;
        if connected.len() > 1 {
            return Err(err(
                "usd.shader_unsupported",
                current.as_str(),
                "multiple shader connection sources",
            ));
        }
        if let Some(next) = connected.first() {
            current = next.clone();
            continue;
        }
        let owner = current.prim_path();
        let kind = token(field(data, &owner, "typeName")?).unwrap_or_default();
        if kind == "Shader" && property_name(&current).is_some_and(|p| p.starts_with("outputs:")) {
            return Ok(ResolvedProperty::Output(current));
        }
        if !matches!(kind.as_str(), "Material" | "NodeGraph" | "Shader") {
            return Err(err(
                "usd.shader_interface",
                current.as_str(),
                "connection source is not a supported shading container",
            ));
        }
        if let Some(value) = field(data, &current, "default")? {
            return Ok(ResolvedProperty::Constant(value));
        }
        return Err(err(
            "usd.shader_unresolved",
            current.as_str(),
            "interface has no authored value or source",
        ));
    }
    Err(err(
        "usd.shader_depth",
        start.as_str(),
        "shader interface chain exceeds 64 connections",
    ))
}
fn input_value(data: &dyn AbstractData, p: &Path, name: &str) -> Result<Option<UsdValue>, SceneError> {
    let path = p.append_property(name).map_err(|e| err("usd.path", p.as_str(), e))?;
    if paths(field(data, &path, "connectionPaths")?)?.is_empty() {
        return field(data, &path, "default");
    }
    match resolve_property(data, &path)? {
        ResolvedProperty::Constant(value) => Ok(Some(value)),
        ResolvedProperty::Output(source) => Err(err(
            "usd.shader_interface",
            source.as_str(),
            "expected a constant interface value, found a shader output",
        )),
    }
}

/// Additional render state lives in typed per-material metadata. The shared
/// glTF bridge consumes it while the original USD source graph stays archived.
pub(super) fn render_settings(
    data: &dyn AbstractData,
    material_paths: &BTreeMap<String, MaterialId>,
) -> Result<BTreeMap<String, MetadataProperty>, SceneError> {
    let mut result = BTreeMap::new();
    for (path, id) in material_paths {
        let material = Path::new(path).map_err(|e| err("usd.path", path, e))?;
        let output = material
            .append_property("outputs:surface")
            .map_err(|e| err("usd.path", path, e))?;
        let ResolvedProperty::Output(shader) = resolve_property(data, &output)? else {
            return Err(err("usd.shader_unsupported", path, "surface is not a shader"));
        };
        let shader = shader.prim_path();
        let threshold = scalar_input(data, &shader, "inputs:opacityThreshold", 0.)?;
        let opacity_property = shader
            .append_property("inputs:opacity")
            .map_err(|e| err("usd.path", path, e))?;
        let opacity_connected = !paths(field(data, &opacity_property, "connectionPaths")?)?.is_empty();
        let opacity = if opacity_connected {
            0.5
        } else {
            scalar_input(data, &shader, "inputs:opacity", 1.)?
        };
        let mode = if threshold > 0. {
            "MASK"
        } else if opacity < 1. {
            "BLEND"
        } else {
            "OPAQUE"
        };
        let mut textures = serde_json::Map::new();
        for (input, slot) in [
            ("diffuseColor", "base_color"),
            ("roughness", "metallic_roughness"),
            ("metallic", "metallic_roughness"),
            ("normal", "normal"),
            ("emissiveColor", "emissive"),
            ("occlusion", "occlusion"),
            ("opacity", "base_color"),
        ] {
            let property = shader
                .append_property(format!("inputs:{input}").as_str())
                .map_err(|e| err("usd.path", path, e))?;
            if paths(field(data, &property, "connectionPaths")?)?.is_empty() {
                continue;
            }
            let ResolvedProperty::Output(source) = resolve_property(data, &property)? else {
                continue;
            };
            let settings = texture_settings(data, &source.prim_path())?;
            if let Some(prior) = textures.get(slot)
                && prior != &settings
            {
                return Err(err(
                    "usd.texture_packing",
                    property.as_str(),
                    "textures combined into one preview slot require identical UV transforms, UV sets and wrapping",
                ));
            }
            textures.insert(slot.into(), settings);
        }
        let value = serde_json::json!({"alpha_mode":mode,"alpha_cutoff":threshold,"textures":textures});
        result.insert(
            id.0.clone(),
            MetadataProperty {
                value: MetadataValue::from_json(value),
                state: MetadataState::Authored,
                source: Some(MetadataSource {
                    format: "usd".into(),
                    path: path.clone(),
                    property: Some(
                        "PreviewSurface and UVTexture state projected to portable glTF render settings".into(),
                    ),
                    value_type: Some("usd-toolbox.gltf-render-settings.v1".into()),
                }),
                unit: None,
            },
        );
    }
    Ok(result)
}

fn scalar_input(data: &dyn AbstractData, p: &Path, name: &str, default: f64) -> Result<f64, SceneError> {
    let value = match input_value(data, p, name)? {
        None => default,
        Some(UsdValue::Float(v)) => f64::from(v),
        Some(UsdValue::Double(v)) => v,
        Some(UsdValue::Int(v)) => f64::from(v),
        _ => return Err(err("usd.shader_type", p.as_str(), format!("{name} expects a scalar"))),
    };
    if !value.is_finite() {
        return Err(err("usd.shader_value", p.as_str(), format!("{name} must be finite")));
    }
    Ok(value)
}
fn vector2_input(data: &dyn AbstractData, p: &Path, name: &str, default: [f64; 2]) -> Result<[f64; 2], SceneError> {
    let value = match input_value(data, p, name)? {
        None => default,
        Some(UsdValue::Vec2f(v)) => <[f32; 2]>::from(v).map(f64::from),
        Some(UsdValue::Vec2d(v)) => v.into(),
        _ => return Err(err("usd.shader_type", p.as_str(), format!("{name} expects float2"))),
    };
    if value.iter().any(|v| !v.is_finite()) {
        return Err(err("usd.shader_value", p.as_str(), format!("{name} must be finite")));
    }
    Ok(value)
}
fn texture_settings(data: &dyn AbstractData, texture: &Path) -> Result<serde_json::Value, SceneError> {
    let st = texture
        .append_property("inputs:st")
        .map_err(|e| err("usd.path", texture.as_str(), e))?;
    let ResolvedProperty::Output(source) = resolve_property(data, &st)? else {
        return Err(err(
            "usd.texture_uv",
            st.as_str(),
            "texture requires an explicit primvar reader",
        ));
    };
    let mut reader = source.prim_path();
    let mut transform = None;
    if token(value(data, &reader, "info:id")?).as_deref() == Some("UsdTransform2d") {
        let scale = vector2_input(data, &reader, "inputs:scale", [1., 1.])?;
        let translation = vector2_input(data, &reader, "inputs:translation", [0., 0.])?;
        let rotation = scalar_input(data, &reader, "inputs:rotation", 0.)?.to_radians();
        // Conjugate SRT by the V-axis flip used by the GLB geometry writer.
        transform = Some(
            serde_json::json!({"offset":[translation[0]-scale[1]*rotation.sin(),1.-translation[1]-scale[1]*rotation.cos()],"scale":scale,"rotation":-rotation}),
        );
        let input = reader
            .append_property("inputs:in")
            .map_err(|e| err("usd.path", reader.as_str(), e))?;
        let ResolvedProperty::Output(source) = resolve_property(data, &input)? else {
            return Err(err(
                "usd.texture_uv",
                input.as_str(),
                "transform requires a primvar source",
            ));
        };
        reader = source.prim_path();
    }
    if token(value(data, &reader, "info:id")?).as_deref() != Some("UsdPrimvarReader_float2") {
        return Err(err(
            "usd.texture_uv",
            reader.as_str(),
            "expected primvar reader or one UsdTransform2d",
        ));
    }
    let name = token(input_value(data, &reader, "inputs:varname")?)
        .ok_or_else(|| err("usd.texture_uv", reader.as_str(), "missing primvar name"))?;
    let coord = if name == "st" {
        0
    } else {
        let suffix = name.strip_prefix("st").ok_or_else(|| {
            err(
                "usd.texture_uv",
                reader.as_str(),
                "preview UV sets must use st or st1 through st31",
            )
        })?;
        suffix
            .parse::<u32>()
            .ok()
            .filter(|&i| i > 0 && i <= 31 && format!("st{i}") == name)
            .ok_or_else(|| {
                err(
                    "usd.texture_uv",
                    reader.as_str(),
                    "preview UV sets must use st or st1 through st31",
                )
            })?
    };
    let mut sampler = serde_json::Map::new();
    for (input, key) in [("inputs:wrapS", "wrapS"), ("inputs:wrapT", "wrapT")] {
        let wrap = token(input_value(data, texture, input)?).unwrap_or_else(|| "useMetadata".into());
        let code = match wrap.as_str() {
            "repeat" => 10497,
            "clamp" => 33071,
            "mirror" => 33648,
            _ => {
                return Err(err(
                    "usd.texture_wrap",
                    texture.as_str(),
                    "declare repeat, clamp or mirror; useMetadata/black border sampling cannot be represented by glTF samplers",
                ));
            }
        };
        sampler.insert(key.into(), code.into());
    }
    let mut result = serde_json::json!({"tex_coord":coord,"sampler":sampler});
    if let Some(transform) = transform {
        result["transform"] = transform;
    }
    Ok(result)
}
