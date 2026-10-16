use super::*;

/// Sidecar files for standalone USDA/USDC. Paths are relative to the exported
/// layer. USDZ already embeds these files. The caller owns storage and transport.
pub fn export_scene_assets(scene: &Scene, limits: &SceneLimits) -> Result<BTreeMap<String, Vec<u8>>, SceneError> {
    require_valid_scene(scene, limits)?;
    validate_payload_hashes(&scene.materials).map_err(|e| err("scene.material", "/", e))?;
    let assets = collect_usd_assets(&scene.materials, false).map_err(|e| err("usd.assets", "/", e))?;
    let mut bytes = 0usize;
    for (path, payload) in &assets {
        validate_entry_name(path).map_err(|e| err("usd.asset_path", path, e))?;
        bytes = bytes.saturating_add(payload.len());
    }
    if bytes > limits.max_bytes || assets.len() > limits.max_dependencies {
        return Err(err("scene.limit", "/", "sidecar assets exceed limits"));
    }
    Ok(assets)
}

pub fn export_scene(scene: &Scene, options: &SceneExportOptions) -> Result<SceneExport, SceneError> {
    require_valid_scene(scene, &options.limits)?;
    if scene.meshes.is_empty() {
        return Err(err("scene.empty", "/", "scene has no meshes"));
    }
    validate_payload_hashes(&scene.materials).map_err(|e| err("scene.material", "/", e))?;
    let manifest = Manifest::from_materials(&scene.materials, options.format == UsdFormat::Usdz);
    let json = serde_json::to_vec(&manifest).map_err(|e| err("usd.encode", "/", e))?;
    let mut data =
        author_layer(&manifest.materials, &json, options.texture_tier).map_err(|e| err("usd.encode", "/", e))?;
    let material_paths: BTreeMap<_, _> = data
        .spec_paths()
        .into_iter()
        .filter_map(|p| match owned_field(&data, &p, "assetInfo") {
            Some(UsdValue::Dictionary(d)) => token(d.get("identifier").cloned()).map(|id| (id, p)),
            _ => None,
        })
        .collect();
    let mut paths_by_id = BTreeMap::new();
    let names: BTreeMap<_, _> = scene
        .nodes
        .keys()
        .enumerate()
        .map(|(i, id)| (id, format!("N{i}")))
        .collect();
    for id in scene.nodes.keys() {
        let mut chain = vec![names[id].clone()];
        let mut parent = scene.nodes[id].parent.as_ref();
        while let Some(p) = parent {
            chain.push(names[p].clone());
            parent = scene.nodes[p].parent.as_ref();
        }
        chain.reverse();
        paths_by_id.insert(
            id.clone(),
            Path::new(&format!("/Model/{}", chain.join("/"))).map_err(|e| err("usd.path", id, e))?,
        );
    }
    let model = Path::new("/Model").unwrap();
    prim(&mut data, &model, "Xform");
    data.set_field(&Path::abs_root(), "defaultPrim", UsdValue::Token("Model".into()));
    data.set_field(
        &Path::abs_root(),
        "metersPerUnit",
        UsdValue::Double(scene.meters_per_unit),
    );
    data.set_field(
        &Path::abs_root(),
        "upAxis",
        UsdValue::Token(
            match scene.up_axis {
                UpAxis::Y => "Y",
                UpAxis::Z => "Z",
            }
            .into(),
        ),
    );
    let metadata = Metadata {
        schema: SCENE_SCHEMA.into(),
        id: scene.id.clone(),
        name: scene.name.clone(),
        nodes: paths_by_id
            .iter()
            .map(|(id, p)| {
                let n = &scene.nodes[id];
                (
                    p.as_str().into(),
                    NodeMetadata {
                        id: id.clone(),
                        name: n.name.clone(),
                        source_path: n.source_path.clone(),
                        mesh: n.mesh.clone(),
                    },
                )
            })
            .collect(),
        dependencies: scene.dependencies.clone(),
        provenance: scene.provenance.clone(),
        metadata: scene.metadata.clone(),
    };
    let Some(UsdValue::Dictionary(mut dictionary)) = field(&data, &Path::abs_root(), "customLayerData")? else {
        unreachable!()
    };
    dictionary.insert(
        "olsyn_scene".into(),
        manifest_json_value(&serde_json::to_string(&metadata).map_err(|e| err("usd.encode", "/", e))?),
    );
    data.set_field(&Path::abs_root(), "customLayerData", UsdValue::Dictionary(dictionary));
    let mut diagnostics = Vec::new();
    for (id, node) in &scene.nodes {
        let p = &paths_by_id[id];
        prim(&mut data, p, if node.mesh.is_some() { "Mesh" } else { "Xform" });
        attr(
            &mut data,
            p,
            "xformOp:transform",
            "matrix4d",
            UsdValue::Matrix4d(gf::Matrix4d(std::array::from_fn(|i| node.transform[i % 4][i / 4]))),
        )?;
        let mut ops = Vec::new();
        if node.reset_transform {
            ops.push("!resetXformStack!".into());
        }
        ops.push("xformOp:transform".into());
        attr(&mut data, p, "xformOpOrder", "token[]", UsdValue::TokenVec(ops))?;
        attr(
            &mut data,
            p,
            "visibility",
            "token",
            UsdValue::Token(if node.visible { "inherited" } else { "invisible" }.into()),
        )?;
        if node.purpose != Purpose::Inherited {
            attr(
                &mut data,
                p,
                "purpose",
                "token",
                UsdValue::Token(
                    match node.purpose {
                        Purpose::Inherited => unreachable!(),
                        Purpose::Default => "default",
                        Purpose::Render => "render",
                        Purpose::Proxy => "proxy",
                        Purpose::Guide => "guide",
                    }
                    .into(),
                ),
            )?;
        }
        if let Some(material) = &node.material {
            bind(&mut data, p, &material_paths[&material.0])?;
        }
        if let Some(mesh_id) = &node.mesh {
            let mesh = &scene.meshes[mesh_id];
            write_mesh(&mut data, p, mesh, &mut diagnostics)?;
            for (i, subset) in node.subsets.iter().enumerate() {
                let s = p
                    .append_path(format!("Subset{i}").as_str())
                    .map_err(|e| err("usd.path", id, e))?;
                prim(&mut data, &s, "GeomSubset");
                data.set_field(
                    &s,
                    "customData",
                    UsdValue::Dictionary(HashMap::from([(
                        "olsyn_name".into(),
                        UsdValue::String(subset.name.clone()),
                    )])),
                );
                attr(&mut data, &s, "elementType", "token", UsdValue::Token("face".into()))?;
                attr(
                    &mut data,
                    &s,
                    "familyName",
                    "token",
                    UsdValue::Token("materialBind".into()),
                )?;
                attr(
                    &mut data,
                    &s,
                    "indices",
                    "int[]",
                    UsdValue::IntVec(to_ints(&subset.faces, p)?),
                )?;
                bind(&mut data, &s, &material_paths[&subset.material.0])?;
            }
            if !node.subsets.is_empty() {
                attr(
                    &mut data,
                    p,
                    "subsetFamily:materialBind:familyType",
                    "token",
                    UsdValue::Token("nonOverlapping".into()),
                )?;
            }
        }
    }
    source_metadata::author(&mut data, scene, &paths_by_id, &material_paths, &mut diagnostics)?;
    rebuild_children(&mut data);
    // openusd 0.7's text writer elides brackets for singleton token list ops,
    // but apiSchemas requires list syntax. An explicit array is equivalent for
    // these newly authored, self-contained layers. Keep typed list ops in USDC.
    let mut text_data = data.clone();
    for p in data.spec_paths() {
        if let Some(UsdValue::TokenListOp(op)) = field(&data, &p, "apiSchemas")? {
            text_data.set_field(&p, "apiSchemas", UsdValue::TokenVec(op.compose_over(&[])));
        }
    }
    for loss in UsdExporter.dry_run(&scene.materials) {
        diagnostics.push(SceneDiagnostic::warning(
            "material.export_loss",
            &loss.material.0,
            format!("{}: {}", loss.parameter, loss.detail),
        ));
    }
    let bytes = match options.format {
        UsdFormat::Usda => TextWriter::write_to_string(&text_data)
            .map(String::into_bytes)
            .map_err(|e| err("usd.encode", "/", e))?,
        UsdFormat::Usdc => {
            let mut cursor = Cursor::new(Vec::new());
            CrateWriter::write(&data, &mut cursor).map_err(|e| err("usd.encode", "/", e))?;
            cursor.into_inner()
        }
        UsdFormat::Usdz => write_usdz(&scene.materials, &text_data, true).map_err(|e| err("usd.encode", "/", e))?,
    };
    if options.format != UsdFormat::Usdz && !export_scene_assets(scene, &options.limits)?.is_empty() {
        diagnostics.push(SceneDiagnostic::warning(
            "usd.external_assets",
            "/",
            "standalone USD layer requires the sidecars returned by export_scene_assets; USDZ embeds them",
        ));
    }
    if bytes.len() > options.limits.max_bytes {
        return Err(err("scene.limit", "/", "encoded output exceeds limit"));
    }
    Ok(SceneExport { bytes, diagnostics })
}

fn prim(data: &mut sdf::Data, p: &Path, kind: &str) {
    let s = data.create_spec(p.clone(), SpecType::Prim);
    s.add(FieldKey::Specifier, UsdValue::Specifier(Specifier::Def));
    s.add(FieldKey::TypeName, UsdValue::Token(kind.into()));
}

fn attr(data: &mut sdf::Data, p: &Path, name: &str, ty: &str, value: UsdValue) -> Result<(), SceneError> {
    create_attribute(data, p, name, ty, Some(value), None, None).map_err(|e| err("usd.encode", p.as_str(), e))?;
    if matches!(
        name,
        "xformOpOrder"
            | "purpose"
            | "subdivisionScheme"
            | "orientation"
            | "doubleSided"
            | "elementType"
            | "familyName"
            | "subsetFamily:materialBind:familyType"
    ) {
        data.set_field(
            &p.append_property(name).map_err(|e| err("usd.path", p.as_str(), e))?,
            "variability",
            UsdValue::Variability(sdf::Variability::Uniform),
        );
    }
    Ok(())
}

fn bind(data: &mut sdf::Data, p: &Path, material: &Path) -> Result<(), SceneError> {
    data.set_field(
        p,
        "apiSchemas",
        UsdValue::TokenListOp(ListOp::prepended([Token::new("MaterialBindingAPI")])),
    );
    let s = data.create_spec(
        p.append_property("material:binding")
            .map_err(|e| err("usd.path", p.as_str(), e))?,
        SpecType::Relationship,
    );
    s.add(FieldKey::Variability, UsdValue::Variability(sdf::Variability::Uniform));
    s.add(
        FieldKey::TargetPaths,
        UsdValue::PathListOp(ListOp::explicit([material.clone()])),
    );
    Ok(())
}

fn to_ints(v: &[u32], p: &Path) -> Result<Vec<i32>, SceneError> {
    v.iter()
        .map(|&i| i32::try_from(i).map_err(|_| err("mesh.index", p.as_str(), "index exceeds USD int range")))
        .collect()
}

fn write_mesh(
    data: &mut sdf::Data,
    p: &Path,
    m: &Mesh,
    diagnostics: &mut Vec<SceneDiagnostic>,
) -> Result<(), SceneError> {
    if m.positions.iter().flatten().any(|&v| !(v as f32).is_finite()) {
        return Err(err("mesh.precision", p.as_str(), "positions exceed USD point3f range"));
    }
    if m.positions.iter().flatten().any(|&v| f64::from(v as f32) != v) {
        diagnostics.push(SceneDiagnostic::warning(
            "mesh.precision",
            p.as_str(),
            "positions converted to USD float32 points",
        ));
    }
    attr(
        data,
        p,
        "points",
        "point3f[]",
        UsdValue::Vec3fVec(m.positions.iter().map(|p| p.map(|v| v as f32).into()).collect()),
    )?;
    attr(
        data,
        p,
        "faceVertexCounts",
        "int[]",
        UsdValue::IntVec(to_ints(&m.face_counts, p)?),
    )?;
    attr(
        data,
        p,
        "faceVertexIndices",
        "int[]",
        UsdValue::IntVec(to_ints(&m.face_indices, p)?),
    )?;
    attr(data, p, "subdivisionScheme", "token", UsdValue::Token("none".into()))?;
    attr(
        data,
        p,
        "orientation",
        "token",
        UsdValue::Token(
            match m.orientation {
                Orientation::RightHanded => "rightHanded",
                Orientation::LeftHanded => "leftHanded",
            }
            .into(),
        ),
    )?;
    attr(data, p, "doubleSided", "bool", UsdValue::Bool(m.double_sided))?;
    if let Some(n) = &m.normals {
        if n.values.iter().flatten().any(|&v| !(v as f32).is_finite()) {
            return Err(err("mesh.precision", p.as_str(), "normals exceed USD normal3f range"));
        }
        attr(
            data,
            p,
            "primvars:normals",
            "normal3f[]",
            UsdValue::Vec3fVec(n.values.iter().map(|p| p.map(|v| v as f32).into()).collect()),
        )?;
        write_interpolation(data, p, "primvars:normals", n.interpolation, n.indices.as_deref())?;
    }
    if let Some(tangents) = &m.tangents {
        attr(
            data,
            p,
            "primvars:toolbox:tangents",
            "float4[]",
            UsdValue::Vec4fVec(tangents.values.iter().map(|v| (*v).into()).collect()),
        )?;
        write_interpolation(
            data,
            p,
            "primvars:toolbox:tangents",
            tangents.interpolation,
            tangents.indices.as_deref(),
        )?;
    }
    for (name, uv) in &m.uv_sets {
        if usd_identifier(name) != *name {
            return Err(err("mesh.uv_name", p.as_str(), name));
        }
        let name = format!("primvars:{name}");
        attr(
            data,
            p,
            &name,
            "texCoord2f[]",
            UsdValue::Vec2fVec(uv.values.iter().map(|&v| v.into()).collect()),
        )?;
        write_interpolation(data, p, &name, uv.interpolation, uv.indices.as_deref())?;
    }
    if let Some(colors) = &m.colors {
        if colors.values.iter().any(|v| v[3] != 1.) {
            return Err(err(
                "usd.color_alpha",
                p.as_str(),
                "vertex alpha is not yet supported in the USD profile",
            ));
        }
        attr(
            data,
            p,
            "primvars:displayColor",
            "color3f[]",
            UsdValue::Vec3fVec(colors.values.iter().map(|v| [v[0], v[1], v[2]].into()).collect()),
        )?;
        write_interpolation(
            data,
            p,
            "primvars:displayColor",
            colors.interpolation,
            colors.indices.as_deref(),
        )?;
    }
    Ok(())
}

fn write_interpolation(
    data: &mut sdf::Data,
    p: &Path,
    name: &str,
    i: Interpolation,
    indices: Option<&[u32]>,
) -> Result<(), SceneError> {
    let property = p.append_property(name).map_err(|e| err("usd.path", p.as_str(), e))?;
    data.set_field(
        &property,
        "interpolation",
        UsdValue::Token(interpolation_name(i).into()),
    );
    if let Some(indices) = indices {
        attr(
            data,
            p,
            &format!("{name}:indices"),
            "int[]",
            UsdValue::IntVec(to_ints(indices, p)?),
        )?;
    }
    Ok(())
}

fn rebuild_children(data: &mut sdf::Data) {
    let all = data.spec_paths();
    let mut children: BTreeMap<Path, Vec<Token>> = BTreeMap::new();
    let mut properties: BTreeMap<Path, Vec<Token>> = BTreeMap::new();
    for p in all {
        if p.is_prim_path()
            && data.spec_type(&p) == Some(SpecType::Prim)
            && let Some(parent) = p.parent()
        {
            children.entry(parent).or_default().push(p.name().unwrap_or("").into());
        }
        if p.is_property_path() && matches!(data.spec_type(&p), Some(SpecType::Attribute | SpecType::Relationship)) {
            properties
                .entry(p.prim_path())
                .or_default()
                .push(property_name(&p).unwrap_or("").into());
        }
    }
    for (p, names) in children {
        data.set_field(&p, ChildrenKey::PrimChildren.as_str(), UsdValue::TokenVec(names));
    }
    for (p, names) in properties {
        data.set_field(&p, ChildrenKey::PropertyChildren.as_str(), UsdValue::TokenVec(names));
    }
}
