use usd_toolbox_core::scene::*;
use usd_toolbox_core::{Input, InputFile};
use usd_toolbox_usd::{UsdFormat, scene::*};

fn input() -> Vec<InputFile<'static>> {
    vec![
        InputFile {
            name: "panels.usda",
            bytes: include_bytes!("../../../fixtures/models/panels.usda"),
        },
        InputFile {
            name: "checker.png",
            bytes: include_bytes!("../../../fixtures/models/checker.png"),
        },
    ]
}

#[test]
fn external_scene_preserves_hierarchy_uvs_bindings_and_world_scale() {
    let files = input();
    let result = import_scene(Input::Bundle(&files), &SceneImportOptions::default()).unwrap();
    let scene = &result.scene;
    assert_eq!(scene.materials.len(), 2);
    assert_eq!(scene.meshes.len(), 1);
    assert_eq!(scene.nodes["/Panels/Surface"].parent.as_deref(), Some("/Panels"));
    assert_eq!(scene.nodes["/Panels/Surface"].subsets[0].material.0, "/Looks/Trim");
    let inspection = inspect_scene(scene, &SceneLimits::default()).unwrap();
    assert_eq!(
        inspection.bounds.unwrap(),
        Bounds {
            min: [750., 450., -250.],
            max: [1750., 450., 250.]
        }
    );
    assert_eq!(inspection.unique_faces, 2);
    assert_eq!(
        scene.meshes["/Panels/Surface"].uv_sets["st"]
            .indices
            .as_ref()
            .unwrap()
            .len(),
        8
    );
    for format in [UsdFormat::Usda, UsdFormat::Usdc, UsdFormat::Usdz] {
        let options = SceneExportOptions {
            format,
            ..Default::default()
        };
        let first = export_scene(scene, &options).unwrap();
        let second = export_scene(scene, &options).unwrap();
        assert_eq!(first.bytes, second.bytes, "deterministic output");
        let read = import_scene(Input::Bytes(&first.bytes), &SceneImportOptions::default()).unwrap();
        assert_eq!(scene, &read.scene, "scene round trip {format:?}");
    }
}

#[test]
fn missing_texture_and_unsupported_geometry_fail_explicitly() {
    let bytes = include_bytes!("../../../fixtures/models/panels.usda");
    assert_eq!(
        import_scene(Input::Bytes(bytes), &Default::default()).unwrap_err().code,
        "usd.missing_dependency"
    );
    let text = String::from_utf8(bytes.to_vec())
        .unwrap()
        .replace("subdivisionScheme = \"none\"", "subdivisionScheme = \"catmullClark\"");
    let files = [
        InputFile {
            name: "model.usda",
            bytes: text.as_bytes(),
        },
        input()[1],
    ];
    assert_eq!(
        import_scene(Input::Bundle(&files), &Default::default())
            .unwrap_err()
            .code,
        "usd.subdivision_unsupported"
    );
    let text = String::from_utf8(bytes.to_vec()).unwrap().replace(
        "metersPerUnit = 0.001",
        "subLayers = [@missing.usda@]\n metersPerUnit = 0.001",
    );
    assert_eq!(
        import_scene(Input::Bytes(text.as_bytes()), &Default::default())
            .unwrap_err()
            .code,
        "usd.unsupported_composition"
    );
}

#[test]
fn model_edits_are_atomic_and_extraction_preserves_world_placement() {
    let files = input();
    let scene = import_scene(Input::Bundle(&files), &Default::default()).unwrap().scene;
    let extracted = extract_subtree(&scene, "/Panels/Surface", &Default::default()).unwrap();
    assert_eq!(scene_bounds(&scene).unwrap(), scene_bounds(&extracted).unwrap());
    assert_eq!(extracted.nodes.len(), 1);
    let converted = edit_scene(
        &scene,
        &[SceneEdit::ConvertUnits { meters_per_unit: 1. }],
        &Default::default(),
    )
    .unwrap();
    assert_eq!(scene_bounds(&converted).unwrap().unwrap().min, [0.75, 0.45, -0.25]);
    assert!(!diff_scenes(&scene, &converted).is_empty());
    assert!(
        edit_scene(
            &scene,
            &[SceneEdit::ConvertUnits { meters_per_unit: 0. }],
            &Default::default()
        )
        .is_err()
    );
    assert_eq!(scene.meters_per_unit, 0.001);
}

#[test]
fn transform_stacks_match_openusd_reference_for_all_rotation_orders_pivots_and_resets() {
    let input = include_bytes!("../../../fixtures/models/transforms.usda");
    let oracle: serde_json::Value =
        serde_json::from_slice(include_bytes!("../../../fixtures/models/transforms-reference.json")).unwrap();
    let scene = import_scene(Input::Bytes(input), &Default::default()).unwrap().scene;
    assert_eq!(scene.up_axis, UpAxis::Z);
    for (id, expected) in oracle["nodes"].as_object().unwrap() {
        assert_eq!(scene.nodes[id].reset_transform, expected["reset"].as_bool().unwrap());
        for (name, actual) in [
            ("local", scene.nodes[id].transform),
            ("world", world_transform(&scene, id).unwrap()),
        ] {
            for (r, row) in actual.iter().enumerate() {
                for (c, value) in row.iter().enumerate() {
                    assert!(
                        (value - expected[name][r][c].as_f64().unwrap()).abs() < 1e-10,
                        "{id} {name} [{r}][{c}]"
                    );
                }
            }
        }
    }
    let package = export_scene(&scene, &Default::default()).unwrap();
    assert_eq!(
        scene,
        import_scene(Input::Bytes(&package.bytes), &Default::default())
            .unwrap()
            .scene
    );
}

#[test]
fn preview_surface_defaults_match_reference_and_types_are_checked() {
    let bytes = include_str!("../../../fixtures/models/panels.usda")
        .replace("color3f inputs:diffuseColor = (0.2, 0.04, 0.01)", "")
        .replace("float inputs:roughness = 0.3", "");
    let files = [
        InputFile {
            name: "panels.usda",
            bytes: bytes.as_bytes(),
        },
        input()[1],
    ];
    let scene = import_scene(Input::Bundle(&files), &Default::default()).unwrap().scene;
    let trim = scene.materials.iter().find(|m| m.name == "Trim").unwrap();
    // Explicit roughness remains authored; missing inputs use UsdPreviewSurface defaults.
    assert_eq!(trim.surface.specular_ior, Some(usd_toolbox_core::Value::from(1.5)));
    assert_eq!(
        trim.surface.base_color,
        usd_toolbox_core::Value::from([0.18, 0.18, 0.18, 1.])
    );
    assert_eq!(trim.surface.specular_roughness, usd_toolbox_core::Value::from(0.5));
    let bad = bytes.replace(
        "uniform token info:id = \"UsdPreviewSurface\"",
        "uniform token info:id = \"UsdPreviewSurface\"\nstring inputs:metallic = \"invalid\"",
    );
    let files = [
        InputFile {
            name: "panels.usda",
            bytes: bad.as_bytes(),
        },
        input()[1],
    ];
    assert_eq!(
        import_scene(Input::Bundle(&files), &Default::default())
            .unwrap_err()
            .code,
        "usd.shader_type"
    );
}

#[test]
fn furniture_fixture_retains_geometry_and_physical_dimensions() {
    let scene = import_scene(
        Input::Bytes(include_bytes!("../../../fixtures/models/desk.usda")),
        &Default::default(),
    )
    .unwrap()
    .scene;
    let inspection = inspect_scene(&scene, &Default::default()).unwrap();
    assert_eq!(inspection.mesh_occurrences, 7);
    assert_eq!(inspection.placed_faces, 42);
    let bounds = inspection.bounds.unwrap();
    assert!((bounds.max[0] - bounds.min[0] - 1.2).abs() < 1e-6);
    assert!((bounds.max[1] - bounds.min[1] - 0.75).abs() < 1e-6);
    let package = export_scene(&scene, &Default::default()).unwrap();
    assert_eq!(
        scene,
        import_scene(Input::Bytes(&package.bytes), &Default::default())
            .unwrap()
            .scene
    );
}

#[test]
fn nested_layer_dependencies_are_anchored_without_ambiguous_fallbacks() {
    let files = [
        InputFile {
            name: "nested/panels.usda",
            bytes: input()[0].bytes,
        },
        InputFile {
            name: "nested/checker.png",
            bytes: input()[1].bytes,
        },
    ];
    assert_eq!(
        import_scene(Input::Bundle(&files), &Default::default())
            .unwrap()
            .scene
            .materials
            .len(),
        2
    );
    let ambiguous = [
        files[0],
        files[1],
        InputFile {
            name: "checker.png",
            bytes: input()[1].bytes,
        },
    ];
    assert_eq!(
        import_scene(Input::Bundle(&ambiguous), &Default::default())
            .unwrap_err()
            .code,
        "usd.dependency_path"
    );
    let mut archive = openusd::usdz::ArchiveWriter::new(std::io::Cursor::new(Vec::new()));
    archive.add_layer(files[0].name, files[0].bytes).unwrap();
    archive.add_layer(files[1].name, files[1].bytes).unwrap();
    let bytes = archive.finish().unwrap().into_inner();
    assert_eq!(
        import_scene(Input::Bytes(&bytes), &Default::default())
            .unwrap()
            .scene
            .materials
            .len(),
        2
    );
}
