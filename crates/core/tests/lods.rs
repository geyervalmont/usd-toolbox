use usd_toolbox_core::scene::*;
use usd_toolbox_core::{Material, MaterialId};

fn source() -> Scene {
    let mut mesh = Mesh::default();
    let n = 8;
    for y in 0..=n {
        for x in 0..=n {
            mesh.positions.push([x as f64, y as f64, 0.]);
        }
    }
    let mut uv = Vec::new();
    let mut right = Vec::new();
    for y in 0..n {
        for x in 0..n {
            let a = y * (n + 1) + x;
            let c = a + n + 1;
            if x >= n / 2 {
                right.extend([mesh.face_counts.len() as u32, mesh.face_counts.len() as u32 + 1]);
            }
            mesh.face_counts.extend([3, 3]);
            for v in [a, a + 1, c + 1, a, c + 1, c] {
                mesh.face_indices.push(v as u32);
                // A chart seam coincident with the per-face material boundary.
                let p = mesh.positions[v];
                uv.push([(p[0] + if x >= n / 2 { 10. } else { 0. }) as f32, p[1] as f32]);
            }
        }
    }
    mesh.uv_sets.insert(
        "st".into(),
        Attribute {
            interpolation: Interpolation::FaceVarying,
            values: uv,
            indices: None,
        },
    );
    let mut scene = Scene::new("lod-source");
    scene.meshes.insert("shared-mesh".into(), mesh);
    scene.materials = vec![Material::new("left", "Left"), Material::new("right", "Right")];
    let mut node = SceneNode::new("Part");
    node.mesh = Some("shared-mesh".into());
    node.material = Some(MaterialId::new("left"));
    node.source_path = Some("/Source/Part".into());
    node.subsets.push(FaceBinding {
        name: "right-side".into(),
        material: MaterialId::new("right"),
        faces: right,
    });
    scene.nodes.insert("first".into(), node.clone());
    node.transform[0][3] = 20.;
    scene.nodes.insert("second".into(), node);
    scene
        .metadata
        .meshes
        .entry("shared-mesh".into())
        .or_default()
        .normalized
        .insert(
            "part_id".into(),
            MetadataProperty::authored(MetadataValue::String("stable".into())),
        );
    scene
}
fn options() -> SceneLodOptions {
    SceneLodOptions {
        levels: vec![
            SceneLodRequest {
                name: "medium".into(),
                face_ratio: 0.75,
                max_vertex_displacement: 2.,
            },
            SceneLodRequest {
                name: "low".into(),
                face_ratio: 0.25,
                max_vertex_displacement: 2.,
            },
            SceneLodRequest {
                name: "locked".into(),
                face_ratio: 0.25,
                max_vertex_displacement: 0.,
            },
        ],
        ..Default::default()
    }
}
#[test]
fn levels_are_independent_deterministic_and_preserve_identities_bindings_and_uv_charts() {
    let source = source();
    let unchanged = source.clone();
    let levels = generate_scene_lods(&source, &options(), &Default::default()).unwrap();
    let manifest = levels.manifest().clone();
    assert_eq!(manifest.schema, "usd-toolbox.geometry-lods.v1");
    assert_eq!(manifest.source_scene_sha256.len(), 64);
    assert_eq!(manifest.source_sha256, None);
    assert!(!manifest.levels[2].target_reached);
    let generated = levels.into_scenes().collect::<Result<Vec<_>, _>>().unwrap();
    for (index, (report, level)) in generated.iter().enumerate() {
        let mut only = options();
        only.levels = vec![only.levels.remove(index)];
        let single = generate_scene_lods(&source, &only, &Default::default())
            .unwrap()
            .into_scenes()
            .next()
            .unwrap()
            .unwrap()
            .1;
        assert_eq!(level, &single, "each level derives directly from source");
        assert_eq!(level.id, source.id);
        assert_eq!(level.name, source.name);
        assert_eq!(level.materials, source.materials);
        assert_eq!(level.metadata, source.metadata);
        assert_eq!(level.meshes.len(), 1);
        assert_eq!(level.nodes["first"].mesh, level.nodes["second"].mesh);
        assert_eq!(level.nodes["first"].source_path, source.nodes["first"].source_path);
        assert_eq!(level.nodes["first"].subsets, level.nodes["second"].subsets);
        assert_eq!(scene_bounds(level).unwrap(), scene_bounds(&source).unwrap());
        let mesh = &level.meshes["shared-mesh"];
        let subset = &level.nodes["first"].subsets[0];
        for (face, corners) in mesh.face_indices.as_chunks::<3>().0.iter().enumerate() {
            let material_right = subset.faces.contains(&(face as u32));
            for (corner, vertex) in corners.iter().enumerate() {
                let p = mesh.positions[*vertex as usize];
                assert!(if material_right { p[0] >= 4. } else { p[0] <= 4. });
                let uv = mesh.uv_sets["st"]
                    .sample(face, face * 3 + corner, *vertex as usize)
                    .unwrap();
                assert_eq!(f64::from(uv[0]), p[0] + if material_right { 10. } else { 0. });
            }
        }
        assert!(report.meshes[0].geometry.max_vertex_displacement <= report.displacement_limit);
        assert_eq!(
            report.target_reached,
            mesh.face_counts.len() <= report.meshes[0].target_faces
        );
        assert_eq!(
            level.provenance.operations.last().unwrap().operation,
            "generate_geometry_lod"
        );
    }
    assert_eq!(generated[2].1.meshes["shared-mesh"].face_counts.len(), 128);
    assert!(generated[0].1.meshes["shared-mesh"].face_counts.len() < 128);
    assert_eq!(source, unchanged);
    let repeated = generate_scene_lods(&source, &options(), &Default::default()).unwrap();
    assert_eq!(
        serde_json::to_value(repeated.manifest()).unwrap(),
        serde_json::to_value(manifest).unwrap()
    );
}
#[test]
fn aggregate_budgets_bad_names_bad_options_and_source_fail_without_mutation() {
    let source = source();
    let before = source.clone();
    let mut variants = vec![SceneLodOptions::default()];
    let mut options = options();
    options.max_levels = 2;
    variants.push(options.clone());
    options.max_levels = 8;
    options.levels[0].name = "../unsafe".into();
    variants.push(options.clone());
    options.levels[0].name = "low".into();
    variants.push(options.clone());
    options.levels[0].name = "medium".into();
    options.levels[0].face_ratio = f64::NAN;
    variants.push(options.clone());
    options.levels[0].face_ratio = 0.5;
    options.levels[0].max_vertex_displacement = -1.;
    variants.push(options);
    for invalid in variants {
        assert_eq!(
            generate_scene_lods(&source, &invalid, &Default::default())
                .unwrap_err()
                .code,
            "geometry.lod_options"
        );
    }
    for budget in 0..3 {
        let mut limited = crate::options();
        match budget {
            0 => limited.max_output_bytes = 1,
            1 => limited.max_output_faces = 128,
            _ => limited.max_source_corners = 384,
        }
        assert_eq!(
            generate_scene_lods(&source, &limited, &Default::default())
                .unwrap_err()
                .code,
            "geometry.lod_limit"
        );
    }
    let limits = SceneLimits {
        max_operations: 0,
        ..Default::default()
    };
    assert_eq!(
        generate_scene_lods(&source, &crate::options(), &limits)
            .unwrap_err()
            .code,
        "scene.operation_limit"
    );
    assert_eq!(source, before);
    let mut invalid = source.clone();
    invalid.meshes.get_mut("shared-mesh").unwrap().face_indices[0] = u32::MAX;
    assert!(generate_scene_lods(&invalid, &crate::options(), &Default::default()).is_err());
}
#[test]
fn fingerprints_cover_neutral_edits_and_request_order_does_not_change_levels() {
    let mut source = source();
    let initial = generate_scene_lods(&source, &options(), &Default::default())
        .unwrap()
        .manifest()
        .source_scene_sha256
        .clone();
    source.nodes.get_mut("first").unwrap().name = "Renamed".into();
    source.provenance.source_sha256 = Some("original-file-sha".into());
    let changed = generate_scene_lods(&source, &options(), &Default::default()).unwrap();
    assert_ne!(changed.manifest().source_scene_sha256, initial);
    assert_eq!(changed.manifest().source_sha256.as_deref(), Some("original-file-sha"));
    let normal = changed
        .into_scenes()
        .map(|result| {
            let (report, scene) = result.unwrap();
            (report.name, scene)
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    let mut reverse = options();
    reverse.levels.reverse();
    for item in generate_scene_lods(&source, &reverse, &Default::default())
        .unwrap()
        .into_scenes()
    {
        let (report, scene) = item.unwrap();
        assert_eq!(scene, normal[&report.name]);
    }
}

#[test]
fn export_helper_preserves_order_and_drops_partial_outputs_on_errors_or_budget() {
    let source = source();
    let options = options();
    let exported = generate_scene_lods(&source, &options, &Default::default())
        .unwrap()
        .export_with(|scene| {
            Ok(SceneExport {
                bytes: scene.meshes["shared-mesh"].face_counts.len().to_le_bytes().to_vec(),
                diagnostics: Vec::new(),
            })
        })
        .unwrap();
    assert_eq!(exported.manifest.levels.len(), 3);
    assert_eq!(
        exported
            .levels
            .iter()
            .map(|level| level.level.name.as_str())
            .collect::<Vec<_>>(),
        ["medium", "low", "locked"]
    );
    let mut calls = 0;
    let failure = generate_scene_lods(&source, &options, &Default::default())
        .unwrap()
        .export_with(|_| {
            calls += 1;
            if calls == 2 {
                Err(SceneError::new("test.export", "/", "encoder failed"))
            } else {
                Ok(SceneExport {
                    bytes: vec![0],
                    diagnostics: Vec::new(),
                })
            }
        })
        .unwrap_err();
    assert_eq!(failure.code, "test.export");
    assert_eq!(calls, 2);
    let mut bounded = options;
    bounded.max_output_bytes = 256 * 1024; // Geometry fits; large encoded outputs do not.
    let mut calls = 0;
    let failure = generate_scene_lods(&source, &bounded, &Default::default())
        .unwrap()
        .export_with(|_| {
            calls += 1;
            Ok(SceneExport {
                bytes: vec![0; 150 * 1024],
                diagnostics: Vec::new(),
            })
        })
        .unwrap_err();
    assert_eq!(failure.code, "geometry.lod_output_limit");
    assert_eq!(calls, 2);
}
