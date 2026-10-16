use super::*;
type LayerBundle = (String, Vec<u8>, BTreeMap<String, Vec<u8>>);

/// Lists authored asset dependencies for a static layer. The caller supplies
/// those paths as a confined bundle; this function performs no I/O.
pub fn asset_dependencies(bytes: &[u8], limits: &SceneLimits) -> Result<Vec<String>, SceneError> {
    if bytes.len() > limits.max_bytes {
        return Err(err("scene.limit", "/", "source exceeds byte limit"));
    }
    if bytes.starts_with(b"PK\x03\x04") {
        return Ok(Vec::new());
    }
    let data: Box<dyn AbstractData> = if bytes.starts_with(USDC_MAGIC) {
        Box::new(
            CrateData::open_with_limits(Cursor::new(bytes), true, crate_limits(limits))
                .map_err(|e| err("usd.decode", "/", e))?,
        )
    } else {
        let text = std::str::from_utf8(bytes).map_err(|e| err("usd.decode", "/", e))?;
        preflight::guard_text_nesting(text, limits.max_depth)?;
        Box::new(openusd::usda::parse(text).map_err(|e| err("usd.decode", "/", e))?)
    };
    let mut dependencies = BTreeSet::new();
    for p in data.spec_paths() {
        if token(field(data.as_ref(), &p, "typeName")?).as_deref() != Some("asset") {
            continue;
        }
        if let Some(UsdValue::AssetPath(v)) = field(data.as_ref(), &p, "default")? {
            validate_entry_name(&v.authored_path).map_err(|e| err("usd.dependency_path", &v.authored_path, e))?;
            dependencies.insert(v.authored_path);
        }
    }
    if dependencies.len() > limits.max_dependencies {
        return Err(err("scene.limit", "/", "too many dependencies"));
    }
    Ok(dependencies.into_iter().collect())
}

pub fn import_scene(input: Input<'_>, options: &SceneImportOptions) -> Result<SceneImport, SceneError> {
    let (root_name, root_bytes, mut entries) = input_entries(input, &options.limits)?;
    let total = entries
        .values()
        .try_fold(root_bytes.len(), |n, b| n.checked_add(b.len()))
        .ok_or_else(|| err("scene.limit", "/", "input byte overflow"))?;
    if total > options.limits.max_bytes {
        return Err(err("scene.limit", "/", "input bytes exceed limit"));
    }
    if root_bytes.starts_with(b"PK\x03\x04") {
        if !entries.is_empty() {
            return Err(err("usd.bundle", "/", "USDZ inputs must be self-contained"));
        }
        let (name, bytes, packaged) = unpack(&root_bytes, &options.limits)?;
        let packaged = anchor_entries(&name, packaged)?;
        return read_layer(&name, &bytes, packaged, options, &root_bytes);
    }
    // Anchor a bundle's texture references at the root layer directory.
    entries = anchor_entries(&root_name, entries)?;
    read_layer(&root_name, &root_bytes, entries, options, &root_bytes)
}

pub(super) fn anchor_entries(
    root_name: &str,
    entries: BTreeMap<String, Vec<u8>>,
) -> Result<BTreeMap<String, Vec<u8>>, SceneError> {
    let root_dir = root_name.rsplit_once('/').map_or("", |(dir, _)| dir);
    if !root_dir.is_empty() {
        let prefix = format!("{root_dir}/");
        return entries
            .into_iter()
            .map(|(name, bytes)| {
                let relative = name.strip_prefix(&prefix).ok_or_else(|| {
                    err(
                        "usd.dependency_path",
                        &name,
                        "single-layer dependencies must stay within the root layer directory",
                    )
                })?;
                Ok((relative.to_owned(), bytes))
            })
            .collect();
    }
    Ok(entries)
}

pub(super) fn input_entries(input: Input<'_>, limits: &SceneLimits) -> Result<LayerBundle, SceneError> {
    let mut entries = BTreeMap::new();
    let (name, bytes) = match input {
        Input::Bytes(b) => ("model.usd", b),
        Input::Named { name, bytes } => (name, bytes),
        Input::Bundle(files) => {
            if files.is_empty() || files.len() > limits.max_dependencies {
                return Err(err("scene.limit", "/", "empty bundle or too many dependencies"));
            }
            let mut size = 0usize;
            for f in files {
                validate_entry_name(f.name).map_err(|e| err("usd.dependency_path", f.name, e))?;
                size = size
                    .checked_add(f.bytes.len())
                    .ok_or_else(|| err("scene.limit", "/", "bundle size overflow"))?;
                if size > limits.max_bytes {
                    return Err(err("scene.limit", "/", "bundle exceeds byte limit"));
                }
                if entries.insert(f.name.to_owned(), f.bytes.to_vec()).is_some() {
                    return Err(err("usd.duplicate_dependency", f.name, "duplicate bundle name"));
                }
            }
            entries.remove(files[0].name);
            (files[0].name, files[0].bytes)
        }
    };
    if bytes.len() > limits.max_bytes {
        return Err(err("scene.limit", name, "source exceeds byte limit"));
    }
    Ok((name.into(), bytes.to_vec(), entries))
}

pub(super) fn unpack(bytes: &[u8], limits: &SceneLimits) -> Result<LayerBundle, SceneError> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| err("usd.archive", "/", e))?;
    if zip.is_empty() || zip.len() > limits.max_dependencies {
        return Err(err("scene.limit", "/", "empty or excessive archive entries"));
    }
    let mut entries = BTreeMap::new();
    let mut root = String::new();
    let mut total = 0u64;
    for i in 0..zip.len() {
        let mut file = zip.by_index(i).map_err(|e| err("usd.archive", "/", e))?;
        let name = file.name().to_owned();
        validate_entry_name(&name).map_err(|e| err("usd.dependency_path", &name, e))?;
        total = total
            .checked_add(file.size())
            .ok_or_else(|| err("scene.limit", &name, "size overflow"))?;
        if total > limits.max_bytes as u64 {
            return Err(err("scene.limit", &name, "archive exceeds byte limit"));
        }
        if file.compression() != CompressionMethod::Stored
            || file.data_start().is_none_or(|n| n % 64 != 0)
            || !is_usdz_allowed_extension(path_extension(&name))
        {
            return Err(err(
                "usd.archive_layout",
                &name,
                "USDZ requires permitted types, stored entries and 64-byte alignment",
            ));
        }
        if i == 0 {
            if !is_usd_layer_extension(path_extension(&name)) {
                return Err(err("usd.archive_root", &name, "first entry must be a USD layer"));
            }
            root = name.clone();
        }
        let mut contents = Vec::new();
        file.by_ref()
            .take(limits.max_bytes as u64 + 1)
            .read_to_end(&mut contents)
            .map_err(|e| err("usd.archive", &name, e))?;
        if contents.len() as u64 != file.size() {
            return Err(err("usd.archive", &name, "entry size mismatch"));
        }
        if entries.insert(name.clone(), contents).is_some() {
            return Err(err("usd.duplicate_dependency", &name, "duplicate archive name"));
        }
    }
    let bytes = entries.remove(&root).expect("root was inserted");
    Ok((root, bytes, entries))
}

fn read_layer(
    name: &str,
    bytes: &[u8],
    entries: BTreeMap<String, Vec<u8>>,
    options: &SceneImportOptions,
    source: &[u8],
) -> Result<SceneImport, SceneError> {
    if bytes.starts_with(USDC_MAGIC) {
        let data = CrateData::open_with_limits(Cursor::new(bytes), true, crate_limits(&options.limits))
            .map_err(|e| err("usd.decode", name, e))?;
        project(&data, name, &entries, options, source)
    } else {
        let text = std::str::from_utf8(bytes).map_err(|e| err("usd.decode", name, e))?;
        preflight::guard_text_nesting(text, options.limits.max_depth)?;
        let data = openusd::usda::parse(text).map_err(|e| err("usd.decode", name, e))?;
        project(&data, name, &entries, options, source)
    }
}

fn project(
    data: &dyn AbstractData,
    name: &str,
    entries: &BTreeMap<String, Vec<u8>>,
    options: &SceneImportOptions,
    source: &[u8],
) -> Result<SceneImport, SceneError> {
    let metadata = match field(data, &Path::abs_root(), "customLayerData")? {
        Some(UsdValue::Dictionary(v)) => match v.get("olsyn_scene").and_then(manifest_json_text) {
            Some(s) => Some(serde_json::from_str::<Metadata>(&s).map_err(|e| err("usd.scene_metadata", "/", e))?),
            None => None,
        },
        _ => None,
    };
    if metadata.as_ref().is_some_and(|m| m.schema != SCENE_SCHEMA) {
        return Err(err("scene.schema", "/", "unsupported embedded scene version"));
    }
    let authored_paths = data.spec_paths();
    let mut inactive = BTreeSet::new();
    for p in &authored_paths {
        if data.spec_type(p) == Some(SpecType::Prim) && field(data, p, "active")? == Some(UsdValue::Bool(false)) {
            inactive.insert(p.as_str().to_owned());
        }
    }
    let paths_all: Vec<_> = authored_paths
        .iter()
        .filter(|p| {
            let mut current = Some(p.prim_path());
            while let Some(p) = current {
                if inactive.contains(p.as_str()) {
                    return false;
                }
                current = p.parent();
            }
            true
        })
        .cloned()
        .collect();
    let mut property_paths: BTreeMap<String, Vec<Path>> = BTreeMap::new();
    let mut child_prims: BTreeMap<String, Vec<Path>> = BTreeMap::new();
    for p in &paths_all {
        if p.is_property_path() {
            property_paths
                .entry(p.prim_path().as_str().into())
                .or_default()
                .push(p.clone());
        } else if data.spec_type(p) == Some(SpecType::Prim)
            && let Some(parent) = p.parent()
        {
            child_prims.entry(parent.as_str().into()).or_default().push(p.clone());
        }
    }
    if paths_all.len() > options.limits.max_nodes.saturating_mul(64) {
        return Err(err("scene.limit", "/", "spec count limit exceeded"));
    }
    check_profile(data, &paths_all, metadata.is_some())?;
    let mut scene = Scene::new(metadata.as_ref().map_or(name, |m| m.id.as_str()));
    scene.name = metadata.as_ref().map_or_else(|| name.to_owned(), |m| m.name.clone());
    scene.meters_per_unit = match field(data, &Path::abs_root(), "metersPerUnit")? {
        Some(UsdValue::Double(v)) => v,
        Some(UsdValue::Float(v)) => f64::from(v),
        None => 0.01,
        _ => return Err(err("scene.units", "/", "invalid metersPerUnit")),
    };
    scene.up_axis = match token(field(data, &Path::abs_root(), "upAxis")?).as_deref() {
        None | Some("Y") => UpAxis::Y,
        Some("Z") => UpAxis::Z,
        Some(v) => return Err(err("scene.axis", "/", v)),
    };
    let mut diagnostics = Vec::new();
    if metadata.is_none() {
        diagnostics.push(SceneDiagnostic::warning("usd.static_projection", "/", "imports the supported static scene profile; supported typed source metadata is retained, unsupported properties receive individual loss reports"));
    }
    for path in &inactive {
        diagnostics.push(SceneDiagnostic::warning(
            "usd.inactive_omitted",
            path,
            "inactive prim and descendants are excluded from the static scene",
        ));
    }
    let (materials, material_paths) =
        shading::import_materials(data, entries, metadata.is_some(), &paths_all, &mut diagnostics)?;
    scene.materials = materials;
    if let Some(m) = &metadata {
        scene.provenance = m.provenance.clone();
        scene.dependencies = m.dependencies.clone();
        scene.metadata = m.metadata.clone();
    } else {
        scene.provenance.source_name = Some(name.into());
        scene.provenance.source_sha256 = Some(digest(source).0);
        scene.dependencies = entries
            .iter()
            .map(|(name, bytes)| SceneDependency {
                authored_path: name.clone(),
                resolved_path: name.clone(),
                sha256: digest(bytes).0,
                bytes: bytes.len() as u64,
            })
            .collect();
    }
    let mut path_ids = BTreeMap::new();
    let mut node_ids = BTreeSet::new();
    for p in &paths_all {
        if data.spec_type(p) != Some(SpecType::Prim) {
            continue;
        }
        let kind = token(field(data, p, "typeName")?).unwrap_or_default();
        if !matches!(kind.as_str(), "" | "Xform" | "Scope" | "Mesh")
            || is_material_child(p, &material_paths)
            || (metadata.is_some() && p.as_str() == "/Materials")
        {
            continue;
        }
        if metadata.is_some() && p.as_str() == "/Model" {
            continue;
        }
        let meta = metadata.as_ref().and_then(|m| m.nodes.get(p.as_str()));
        if metadata.is_some() && meta.is_none() {
            return Err(err(
                "usd.scene_metadata",
                p.as_str(),
                "scene node missing from identity map",
            ));
        }
        let id = meta.map_or_else(|| p.as_str().to_owned(), |m| m.id.clone());
        if !node_ids.insert(id.clone()) {
            return Err(err("scene.identity", p.as_str(), "duplicate node identity"));
        }
        path_ids.insert(p.as_str().to_owned(), id);
    }
    if metadata.as_ref().is_some_and(|m| m.nodes.len() != path_ids.len()) {
        return Err(err("usd.scene_metadata", "/", "identity map contains missing nodes"));
    }
    for (p, id) in &path_ids {
        let p = Path::new(p).map_err(|e| err("usd.path", p, e))?;
        let meta = metadata.as_ref().and_then(|m| m.nodes.get(p.as_str()));
        let mut node = SceneNode::new(meta.map_or_else(|| p.name().unwrap_or("node").into(), |m| m.name.clone()));
        node.source_path = meta.map_or_else(|| Some(p.as_str().into()), |m| m.source_path.clone());
        node.parent = p.parent().and_then(|parent| path_ids.get(parent.as_str()).cloned());
        let (transform, reset, projected) = transforms::read_transform(data, &p)?;
        node.transform = transform;
        node.reset_transform = reset;
        if projected {
            diagnostics.push(SceneDiagnostic::warning(
                "usd.transform_projected",
                p.as_str(),
                "authored transform operations evaluated to one static matrix",
            ));
        }
        node.visible = match token(value(data, &p, "visibility")?).as_deref() {
            Some("invisible") => false,
            None | Some("inherited") => true,
            Some(v) => return Err(err("usd.visibility", p.as_str(), v)),
        };
        node.purpose = match token(value(data, &p, "purpose")?).as_deref() {
            None => Purpose::Inherited,
            Some("default") => Purpose::Default,
            Some("render") => Purpose::Render,
            Some("proxy") => Purpose::Proxy,
            Some("guide") => Purpose::Guide,
            Some(v) => return Err(err("usd.purpose", p.as_str(), v)),
        };
        node.material = resolve_binding(data, &p, &material_paths, false)?;
        if token(field(data, &p, "typeName")?).as_deref() == Some("Mesh") {
            let mesh = read_mesh(data, &p, &property_paths)?;
            let shared_mesh = match field(data, &p, "customData")? {
                Some(UsdValue::Dictionary(values)) => token(values.get("usd_toolbox_shared_mesh").cloned()),
                _ => None,
            };
            let mesh_id = meta
                .and_then(|m| m.mesh.clone())
                .or(shared_mesh)
                .unwrap_or_else(|| id.clone());
            if let Some(existing) = scene.meshes.get(&mesh_id)
                && existing != &mesh
            {
                return Err(err("usd.mesh_identity", p.as_str(), "shared mesh occurrences disagree"));
            }
            scene.meshes.insert(mesh_id.clone(), mesh);
            node.mesh = Some(mesh_id);
            for subset in child_prims.get(p.as_str()).into_iter().flatten() {
                if token(field(data, subset, "typeName")?).as_deref() != Some("GeomSubset") {
                    continue;
                }
                if token(value(data, subset, "elementType")?).as_deref() != Some("face")
                    || token(value(data, subset, "familyName")?).as_deref() != Some("materialBind")
                {
                    return Err(err(
                        "usd.subset_unsupported",
                        subset.as_str(),
                        "only face materialBind subsets are supported",
                    ));
                }
                let mat = resolve_binding(data, subset, &material_paths, true)?
                    .ok_or_else(|| err("binding.missing", subset.as_str(), "subset has no material"))?;
                let name = match field(data, subset, "customData")? {
                    Some(UsdValue::Dictionary(d)) => token(d.get("olsyn_name").cloned()),
                    _ => None,
                }
                .unwrap_or_else(|| subset.name().unwrap_or("subset").into());
                node.subsets.push(FaceBinding {
                    name,
                    faces: ints(value(data, subset, "indices")?, subset.as_str())?,
                    material: mat,
                });
            }
            if token(value(data, &p, "subsetFamily:materialBind:familyType")?).as_deref() == Some("partition") {
                let covered: BTreeSet<_> = node.subsets.iter().flat_map(|s| s.faces.iter().copied()).collect();
                if covered.len() != scene.meshes[node.mesh.as_ref().unwrap()].face_counts.len() {
                    return Err(err(
                        "binding.partition",
                        p.as_str(),
                        "partition subsets must cover every face",
                    ));
                }
            }
        }
        scene.nodes.insert(id.clone(), node);
    }
    if metadata.is_none() {
        scene.metadata = source_metadata::capture(data, &authored_paths, &path_ids, &material_paths, &mut diagnostics)?;
    }
    if metadata.is_none() {
        for (id, settings) in shading::render_settings(data, &material_paths)? {
            scene
                .metadata
                .materials
                .entry(id)
                .or_default()
                .original
                .insert("gltf:render_settings".into(), settings);
        }
    }
    if scene.meshes.is_empty() {
        return Err(err("scene.empty", "/", "input contains no supported meshes"));
    }
    require_valid_scene(&scene, &options.limits)?;
    validate_payload_hashes(&scene.materials).map_err(|e| err("usd.material_integrity", "/", e))?;
    scene_bounds(&scene)?;
    Ok(SceneImport { scene, diagnostics })
}

fn is_material_child(path: &Path, materials: &BTreeMap<String, usd_toolbox_core::MaterialId>) -> bool {
    let mut current = Some(path.clone());
    while let Some(p) = current {
        if materials.contains_key(p.as_str()) {
            return true;
        }
        current = p.parent();
    }
    false
}

fn check_profile(data: &dyn AbstractData, paths: &[Path], authored: bool) -> Result<(), SceneError> {
    for p in paths {
        let material_scope = authored && (p.as_str() == "/Materials" || p.as_str().starts_with("/Materials/"));
        for f in data.list_fields(p).unwrap_or_default() {
            if matches!(
                f.as_str(),
                "references" | "payload" | "subLayers" | "inheritPaths" | "specializes" | "relocates"
            ) || (matches!(f.as_str(), "timeSamples" | "variantSetNames" | "variantSelection") && !material_scope)
            {
                return Err(err(
                    "usd.unsupported_composition",
                    p.as_str(),
                    format!("{f}: supply a static, precomposed model"),
                ));
            }
        }
        if field(data, p, "instanceable")? == Some(UsdValue::Bool(true)) {
            return Err(err(
                "usd.instancing_unsupported",
                p.as_str(),
                "instances require a precomposed input",
            ));
        }
        if data.spec_type(p) == Some(SpecType::Prim) {
            let kind = token(field(data, p, "typeName")?).unwrap_or_default();
            if matches!(kind.as_str(), "Scope" | "") && value(data, p, "xformOpOrder")?.is_some() {
                return Err(err(
                    "usd.transform_unsupported",
                    p.as_str(),
                    "transform operations require an Xformable prim",
                ));
            }
            if !matches!(
                kind.as_str(),
                "" | "Xform" | "Scope" | "Mesh" | "GeomSubset" | "Material" | "Shader" | "NodeGraph"
            ) {
                return Err(err("usd.prim_unsupported", p.as_str(), kind));
            }
            if !material_scope && matches!(kind.as_str(), "Xform" | "Scope" | "Mesh" | "GeomSubset" | "") {
                let schemas = match field(data, p, "apiSchemas")? {
                    None => Vec::new(),
                    Some(UsdValue::TokenListOp(op)) => op.compose_over(&[]),
                    Some(UsdValue::TokenVec(v)) => v,
                    _ => return Err(err("usd.schema_unsupported", p.as_str(), "invalid applied schema list")),
                };
                if let Some(schema) = schemas.iter().find(|s| s.as_str() != "MaterialBindingAPI") {
                    return Err(err("usd.schema_unsupported", p.as_str(), schema));
                }
            }
        }
        if p.is_property_path() && !material_scope {
            let name = property_name(p).unwrap_or("");
            if matches!(name, "velocities" | "accelerations" | "angularVelocities") {
                return Err(err(
                    "usd.property_unsupported",
                    p.as_str(),
                    "motion attributes require preprocessing",
                ));
            }
        }
        if p.is_property_path()
            && property_name(p).is_some_and(|n| n.starts_with("material:binding:") || n.starts_with("collection:"))
        {
            return Err(err(
                "usd.binding_unsupported",
                p.as_str(),
                "collection and purpose-specific bindings are not supported",
            ));
        }
        if token(field(data, p, "bindMaterialAs")?)
            .is_some_and(|v| !matches!(v.as_str(), "weakerThanDescendants" | "strongerThanDescendants"))
        {
            return Err(err("usd.binding_strength", p.as_str(), "invalid binding strength"));
        }
    }
    Ok(())
}

fn read_mesh(data: &dyn AbstractData, p: &Path, all: &BTreeMap<String, Vec<Path>>) -> Result<Mesh, SceneError> {
    if token(value(data, p, "subdivisionScheme")?).as_deref() != Some("none") {
        return Err(err(
            "usd.subdivision_unsupported",
            p.as_str(),
            "polygon profile requires subdivisionScheme = none",
        ));
    }
    for name in ["holeIndices", "creaseIndices", "cornerIndices"] {
        if let Some(v) = value(data, p, name)?
            && !ints(Some(v), p.as_str())?.is_empty()
        {
            return Err(err("usd.mesh_feature_unsupported", p.as_str(), name));
        }
    }
    let mut mesh = Mesh {
        positions: vectors3(value(data, p, "points")?, p.as_str())?,
        face_counts: ints(value(data, p, "faceVertexCounts")?, p.as_str())?,
        face_indices: ints(value(data, p, "faceVertexIndices")?, p.as_str())?,
        ..Mesh::default()
    };
    mesh.orientation = match token(value(data, p, "orientation")?).as_deref() {
        None | Some("rightHanded") => Orientation::RightHanded,
        Some("leftHanded") => Orientation::LeftHanded,
        Some(v) => return Err(err("usd.orientation", p.as_str(), v)),
    };
    mesh.double_sided = value(data, p, "doubleSided")? == Some(UsdValue::Bool(true));
    let mut primvars: BTreeMap<String, Path> = BTreeMap::new();
    let mut blocked = BTreeSet::new();
    let mut ancestor = Some(p.clone());
    while let Some(current) = ancestor {
        for property in all.get(current.as_str()).into_iter().flatten() {
            let name = property_name(property).unwrap_or("");
            if !name.starts_with("primvars:")
                || name.ends_with(":indices")
                || primvars.contains_key(name)
                || blocked.contains(name)
            {
                continue;
            }
            if current != *p && interpolation(data, &current, name, Interpolation::Constant)? != Interpolation::Constant
            {
                continue;
            }
            match value(data, &current, name)? {
                Some(UsdValue::ValueBlock) => {
                    blocked.insert(name.to_owned());
                }
                Some(_) => {
                    primvars.insert(name.to_owned(), current.clone());
                }
                None => {}
            }
        }
        ancestor = current.parent();
    }
    if let Some(owner) = primvars.remove("primvars:normals") {
        mesh.normals = Some(attribute(
            data,
            &owner,
            "primvars:normals",
            Interpolation::Constant,
            vectors3(value(data, &owner, "primvars:normals")?, owner.as_str())?,
        )?);
    } else if let Some(v) = value(data, p, "normals")? {
        mesh.normals = Some(attribute(
            data,
            p,
            "normals",
            Interpolation::Vertex,
            vectors3(Some(v), p.as_str())?,
        )?);
    }
    for (name, owner) in primvars {
        match value(data, &owner, &name)? {
            Some(UsdValue::Vec2fVec(v)) => {
                mesh.uv_sets.insert(
                    name[9..].into(),
                    attribute(
                        data,
                        &owner,
                        &name,
                        Interpolation::Constant,
                        v.into_iter().map(<[f32; 2]>::from).collect(),
                    )?,
                );
            }
            Some(UsdValue::Vec3fVec(v)) if name == "primvars:displayColor" => {
                mesh.colors = Some(attribute(
                    data,
                    &owner,
                    &name,
                    Interpolation::Constant,
                    v.into_iter()
                        .map(|v| {
                            let [r, g, b] = v.into();
                            [r, g, b, 1.]
                        })
                        .collect(),
                )?);
            }
            Some(UsdValue::Vec4fVec(v)) if name == "primvars:toolbox:tangents" => {
                mesh.tangents = Some(attribute(
                    data,
                    &owner,
                    &name,
                    Interpolation::Constant,
                    v.into_iter().map(<[f32; 4]>::from).collect(),
                )?);
            }
            _ => {
                return Err(err(
                    "usd.primvar_unsupported",
                    format!("{}.{name}", owner.as_str()),
                    "unsupported primvar type or semantic",
                ));
            }
        }
    }
    Ok(mesh)
}

fn resolve_binding(
    data: &dyn AbstractData,
    p: &Path,
    materials: &BTreeMap<String, usd_toolbox_core::MaterialId>,
    inherit: bool,
) -> Result<Option<usd_toolbox_core::MaterialId>, SceneError> {
    let mut current = Some(p.clone());
    let mut result = None;
    while let Some(current_path) = current {
        if current_path == Path::abs_root() {
            break;
        }
        let rel = current_path
            .append_property("material:binding")
            .map_err(|e| err("usd.path", current_path.as_str(), e))?;
        let targets = paths(field(data, &rel, "targetPaths")?)?;
        if targets.len() > 1 {
            return Err(err(
                "usd.binding",
                current_path.as_str(),
                "direct binding must have a single target",
            ));
        }
        let stronger = token(field(data, &rel, "bindMaterialAs")?).as_deref() == Some("strongerThanDescendants");
        if let Some(target) = targets.first()
            && (stronger || (result.is_none() && (inherit || current_path == *p)))
        {
            result = Some(materials.get(target.as_str()).cloned().ok_or_else(|| {
                err(
                    "usd.binding",
                    current_path.as_str(),
                    format!("unresolved material {target}"),
                )
            })?);
        }
        current = current_path.parent();
    }
    Ok(result)
}
