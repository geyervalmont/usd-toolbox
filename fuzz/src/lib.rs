//! Bounded fuzz entry points. Panics are deliberately not caught: each is a bug
//! or an upstream parser defect to minimize and triage, never an accepted result.
use std::io::Cursor;
use usd_toolbox_core::{Importer, Input, scene::*};

pub const MAX_INPUT: usize = 64 * 1024;
pub fn limits() -> SceneLimits {
    SceneLimits {
        max_nodes: 256,
        max_depth: 32,
        max_points: 4096,
        max_faces: 4096,
        max_corners: 12288,
        max_face_corners: 64,
        max_bytes: 4 * 1024 * 1024,
        max_dependencies: 32,
        ..SceneLimits::default()
    }
}
fn usd(data: &[u8], name: &str) {
    if data.len() > MAX_INPUT {
        return;
    }
    let input = Input::Named { name, bytes: data };
    let options = usd_toolbox_usd::scene::PreflightOptions {
        limits: limits(),
        max_specs: 1024,
        max_issues: 32,
        max_metadata_bytes: 64 * 1024,
        ..Default::default()
    };
    let report = usd_toolbox_usd::scene::preflight(input, &options);
    assert!(report.issues.len() <= 33, "preflight issue limit escaped");
    if let Ok(imported) =
        usd_toolbox_usd::scene::import_scene(input, &usd_toolbox_usd::scene::SceneImportOptions { limits: limits() })
    {
        require_valid_scene(&imported.scene, &limits()).expect("import returned invalid scene");
        let _ = SceneDocument::new(imported.scene, limits());
    }
}
pub fn usda(data: &[u8]) {
    usd(data, "fuzz.usda")
}
pub fn usdc(data: &[u8]) {
    usd(data, "fuzz.usdc")
}
pub fn usdz(data: &[u8]) {
    usd(data, "fuzz.usdz")
}
pub fn materialx(data: &[u8]) {
    if data.len() > MAX_INPUT {
        return;
    }
    let _ = usd_toolbox_materialx::parse_document(data);
    let _ = usd_toolbox_materialx::referenced_files(data);
    if let Ok(materials) = usd_toolbox_materialx::MaterialXImporter.import(
        Input::Named {
            name: "fuzz.mtlx",
            bytes: data,
        },
        &Default::default(),
    ) {
        assert!(
            usd_toolbox_core::validate_materials(&materials).is_empty(),
            "import returned invalid materials"
        );
    }
}
pub fn gltf(data: &[u8]) {
    if data.len() > MAX_INPUT {
        return;
    }
    let options = usd_toolbox_gltf::scene::SceneGltfImportOptions {
        limits: limits(),
        ..Default::default()
    };
    let _ = usd_toolbox_gltf::scene::preflight_scene(data, "fuzz.glb", &limits());
    let _ = usd_toolbox_gltf::scene::discover_dependencies(data, "fuzz.glb", &limits());
    if let Ok(imported) = usd_toolbox_gltf::scene::import_scene(data, "fuzz.glb", &options) {
        require_valid_scene(&imported.scene, &limits()).expect("import returned invalid scene");
        if let Ok(exported) = usd_toolbox_gltf::scene::export_scene(
            &imported.scene,
            &usd_toolbox_gltf::scene::SceneGltfOptions {
                limits: limits(),
                ..Default::default()
            },
        ) {
            // The exporter may add a coordinate root and primitive nodes, so the
            // reimport gets modest headroom over the input operation's node limit.
            let options = usd_toolbox_gltf::scene::SceneGltfImportOptions {
                limits: SceneLimits {
                    max_nodes: 1024,
                    max_points: 12288,
                    max_depth: 64,
                    max_bytes: 16 * 1024 * 1024,
                    ..limits()
                },
                ..Default::default()
            };
            usd_toolbox_gltf::scene::import_scene(&exported.bytes, "roundtrip.glb", &options)
                .expect("exported glTF must reimport");
        }
    }
}
pub fn images(data: &[u8]) {
    if data.len() > MAX_INPUT {
        return;
    }
    let Ok(reader) = image::ImageReader::new(Cursor::new(data)).with_guessed_format() else {
        return;
    };
    if !matches!(
        reader.format(),
        Some(image::ImageFormat::Png | image::ImageFormat::Jpeg)
    ) {
        return;
    }
    let Ok((width, height)) = reader.into_dimensions() else {
        return;
    };
    if width == 0 || height == 0 || width > 512 || height > 512 {
        return;
    }
    let mut reader = image::ImageReader::new(Cursor::new(data))
        .with_guessed_format()
        .unwrap();
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(4 * 1024 * 1024);
    limits.max_image_width = Some(512);
    limits.max_image_height = Some(512);
    reader.limits(limits);
    let _ = reader.decode();
}
pub fn metadata(data: &[u8]) {
    if data.len() > MAX_INPUT {
        return;
    }
    let Ok(metadata) = serde_json::from_slice::<SceneMetadata>(data) else {
        return;
    };
    let limits = MetadataLimits {
        max_bytes: 64 * 1024,
        max_property_bytes: 16 * 1024,
        max_depth: 16,
        max_properties: 256,
    };
    if validate_metadata(&metadata, &limits).is_ok() {
        let bytes = serde_json::to_vec(&metadata).expect("validated metadata serializes");
        let copy = serde_json::from_slice::<SceneMetadata>(&bytes).expect("metadata reimports");
        assert_eq!(metadata, copy, "typed metadata roundtrip changed a value");
    }
}
fn document() -> SceneDocument {
    let mut scene = Scene::new("fuzz-query");
    for i in 0..64 {
        let id = format!("n{i}");
        let mut node = SceneNode::new(format!("Part café {i}"));
        node.source_path = Some(format!("/World/Part{i}"));
        if i > 0 {
            node.parent = Some(format!("n{}", (i - 1) / 4));
        }
        scene.nodes.insert(id.clone(), node);
        let mut metadata = Metadata::default();
        metadata.normalized.insert(
            "code".into(),
            MetadataProperty::authored(MetadataValue::U64((u64::MAX - i).to_string())),
        );
        metadata.normalized.insert(
            "rank".into(),
            MetadataProperty::authored(MetadataValue::I64(i.to_string())),
        );
        scene.metadata.nodes.insert(id, metadata);
    }
    SceneDocument::with_options(
        scene,
        DocumentOptions {
            scene_limits: limits(),
            metadata_limits: MetadataLimits {
                max_bytes: 64 * 1024,
                max_property_bytes: 4096,
                max_depth: 16,
                max_properties: 256,
            },
            query_limits: QueryLimits {
                max_results: 64,
                max_response_bytes: 128 * 1024,
                max_predicates: 8,
                max_index_bytes: 128 * 1024,
                max_cache_bytes: 128 * 1024,
            },
            metadata_indexes: vec![],
        },
    )
    .expect("fixed fuzz scene is valid")
}
pub fn query(data: &[u8]) {
    if data.len() > 16 * 1024 {
        return;
    }
    let Ok(request) = serde_json::from_slice::<QueryRequest>(data) else {
        return;
    };
    let mut document = document();
    if let Ok(page) = document.query(&request) {
        assert!(page.rows.len() <= request.limit.min(64));
        let repeated = document.query(&request).expect("repeat query remains valid");
        assert_eq!(
            serde_json::to_value(&page).unwrap(),
            serde_json::to_value(repeated).unwrap(),
            "query/cache changed results"
        );
        let _ = document.count(&request.filter);
        if let Some(cursor) = page.next_cursor {
            let mut next = request.clone();
            next.cursor = Some(cursor);
            document.query(&next).expect("issued cursor must resolve");
            document
                .apply(&[SceneEdit::Rename {
                    node: "n0".into(),
                    name: "Changed".into(),
                }])
                .unwrap();
            assert!(document.query(&next).is_err(), "stale cursor accepted after edit");
        }
    }
}
