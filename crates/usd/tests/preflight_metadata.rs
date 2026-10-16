use usd_toolbox_core::scene::*;
use usd_toolbox_core::{Input, InputFile};
use usd_toolbox_usd::{UsdFormat, scene::*};

const SOURCE: &[u8] = include_bytes!("../../../fixtures/models/metadata-semantics.usda");

#[test]
fn metadata_wide_integers_nested_types_and_unicode_survive_usd_roundtrips() {
    let imported = import_scene(Input::Bytes(SOURCE), &Default::default()).unwrap();
    let scene = imported.scene;
    let metadata = &scene.metadata.nodes["/Product"].original;
    assert_eq!(
        metadata["usd:attribute:product:serial"].value.to_json().as_u64(),
        Some(9_007_199_254_740_993)
    );
    let json = metadata["usd:customData"].value.to_json();
    assert_eq!(json["productId"].as_u64(), Some(u64::MAX));
    assert_eq!(json["negativeId"].as_i64(), Some(i64::MIN));
    assert_eq!(json["catalogue"]["tags"][1], "木材");
    assert!(
        scene.metadata.materials["/Looks/Strong"]
            .original
            .contains_key("usd:source_graph")
    );
    for format in [UsdFormat::Usda, UsdFormat::Usdc, UsdFormat::Usdz] {
        let output = export_scene(
            &scene,
            &SceneExportOptions {
                format,
                ..Default::default()
            },
        )
        .unwrap();
        let result = import_scene(Input::Bytes(&output.bytes), &Default::default()).unwrap();
        assert_eq!(scene.metadata, result.scene.metadata);
        if format == UsdFormat::Usda {
            let text = String::from_utf8(output.bytes).unwrap();
            assert!(text.contains("custom uint64 product:serial = 9007199254740993"));
            assert!(text.contains("uint64 productId = 18446744073709551615"));
            assert!(text.contains("kind = \"component\""));
        }
    }
}

#[test]
fn inactive_pruning_inherited_constant_primvars_strength_and_interfaces_are_projected() {
    let result = import_scene(Input::Bytes(SOURCE), &Default::default()).unwrap();
    assert!(!result.scene.nodes.keys().any(|p| p.contains("Disabled")));
    assert!(result.diagnostics.iter().any(|d| d.code == "usd.inactive_omitted"));
    assert_eq!(
        result.scene.nodes["/Product/Surface"].material.as_ref().unwrap().0,
        "/Looks/Strong"
    );
    let colors = result.scene.meshes["/Product/Surface"].colors.as_ref().unwrap();
    assert_eq!(colors.interpolation, Interpolation::Constant);
    assert_eq!(colors.values, vec![[0.25, 0.5, 0.75, 1.]]);
    let material = result
        .scene
        .materials
        .iter()
        .find(|m| m.id.0 == "/Looks/Strong")
        .unwrap();
    assert_eq!(
        material.surface.base_color,
        usd_toolbox_core::Value::from([0.8, 0.2, 0.1, 1.])
    );
    assert_eq!(material.surface.specular_roughness, usd_toolbox_core::Value::from(0.4));
}

#[test]
fn preflight_accumulates_blockers_and_inventory_without_resolving_dependencies() {
    let source = String::from_utf8(SOURCE.to_vec())
        .unwrap()
        .replace("metersPerUnit = 1", "subLayers = [@missing.usda@]\nmetersPerUnit = 1")
        .replace(
            "uniform token subdivisionScheme = \"none\"",
            "uniform token subdivisionScheme = \"catmullClark\"",
        );
    let report = preflight(Input::Bytes(source.as_bytes()), &Default::default());
    assert!(report.complete, "{:?}", report.issues);
    assert!(report.features.contains("sublayers"));
    assert!(report.features.contains("subdivision"));
    assert!(report.issues.iter().any(|d| d.code == "usd.unsupported_composition"));
    assert!(report.issues.iter().any(|d| d.code == "usd.subdivision_unsupported"));
    assert!(
        report
            .dependencies
            .iter()
            .any(|d| d.authored_path == "missing.usda" && d.status == "missing")
    );
    assert!(
        report
            .metadata
            .iter()
            .any(|m| m.path == "/Product" && m.field == "customData")
    );
    assert_eq!(
        report.prim_types["Camera"], 1,
        "unsupported inactive types remain visible in inventory"
    );
    assert_eq!(
        report.estimates.points, 0,
        "metadata scan does not materialize neutral geometry"
    );
}

#[test]
fn preflight_limits_and_malformed_input_have_explicit_incomplete_reports() {
    for options in [
        PreflightOptions {
            max_specs: 2,
            ..Default::default()
        },
        PreflightOptions {
            max_issues: 1,
            ..Default::default()
        },
        PreflightOptions {
            max_metadata_bytes: 16,
            ..Default::default()
        },
    ] {
        let report = preflight(Input::Bytes(SOURCE), &options);
        assert!(!report.complete, "{:?}", report);
        assert!(report.issues.len() <= options.max_issues.max(1));
    }
    let report = preflight(Input::Bytes(b"#usda 1.0\ndef Mesh {"), &Default::default());
    assert!(!report.complete);
    assert_eq!(report.issues[0].code, "usd.decode");
    let source = format!("#usda 1.0\n{}", "def Xform \"A\" {".repeat(50));
    let options = PreflightOptions {
        limits: SceneLimits {
            max_depth: 16,
            ..Default::default()
        },
        ..Default::default()
    };
    let report = preflight(Input::Bytes(source.as_bytes()), &options);
    assert_eq!(report.issues[0].code, "usd.nesting_limit");
}

#[test]
fn preflight_target_geometry_checks_and_tangent_roundtrip() {
    let files = [
        InputFile {
            name: "panels.usda",
            bytes: include_bytes!("../../../fixtures/models/panels.usda"),
        },
        InputFile {
            name: "checker.png",
            bytes: include_bytes!("../../../fixtures/models/checker.png"),
        },
    ];
    let report = preflight(
        Input::Bundle(&files),
        &PreflightOptions {
            target: PreflightTarget::Glb,
            check_geometry: true,
            ..Default::default()
        },
    );
    assert!(report.complete, "{:?}", report.issues);
    assert_eq!(report.estimates.faces, 2);
    assert!(report.performed.iter().any(|p| p.contains("triangulation")));
    let mut scene = import_scene(Input::Bytes(SOURCE), &Default::default()).unwrap().scene;
    scene.meshes.get_mut("/Product/Surface").unwrap().tangents = Some(Attribute {
        interpolation: Interpolation::Constant,
        values: vec![[1., 0., 0., -1.]],
        indices: None,
    });
    let bytes = export_scene(&scene, &Default::default()).unwrap().bytes;
    let result = import_scene(Input::Bytes(&bytes), &Default::default()).unwrap();
    assert_eq!(scene.meshes, result.scene.meshes);
}

#[test]
fn cyclic_shader_interfaces_report_source_path() {
    let source = String::from_utf8(SOURCE.to_vec()).unwrap().replace(
        "color3f inputs:color = (0.8,0.2,0.1)",
        "color3f inputs:color.connect = </Looks/Strong/Interface.inputs:color>",
    );
    let error = import_scene(Input::Bytes(source.as_bytes()), &Default::default()).unwrap_err();
    assert_eq!(error.code, "usd.shader_cycle");
    assert!(error.path.contains("inputs:color"));
}

#[test]
fn archived_graph_roundtrips_do_not_recursively_grow_and_large_strings_fail_safely() {
    let mut scene = import_scene(Input::Bytes(SOURCE), &Default::default()).unwrap().scene;
    let mut first_size = None;
    for _ in 0..4 {
        let output = export_scene(&scene, &Default::default()).unwrap();
        if let Some(size) = first_size {
            assert_eq!(output.bytes.len(), size);
        } else {
            first_size = Some(output.bytes.len());
        }
        let next = import_scene(Input::Bytes(&output.bytes), &Default::default())
            .unwrap()
            .scene;
        assert_eq!(next.metadata, scene.metadata);
        scene = next;
    }
    let source = format!("#usda 1.0\n( documentation = \"{}\" )", "\\\"".repeat(600));
    let report = preflight(Input::Bytes(source.as_bytes()), &Default::default());
    assert!(!report.complete);
    assert_eq!(report.issues[0].code, "usd.text_token_limit");
}

#[test]
fn texture_uv_selection_wrap_transform_and_cutout_are_explicit_render_settings() {
    let source = include_str!("../../../fixtures/models/panels.usda")
        .replace("primvars:st", "primvars:st1")
        .replace("string inputs:varname = \"st\"", "string inputs:varname = \"st1\"")
        .replace("token inputs:wrapS = \"repeat\"", "token inputs:wrapS = \"clamp\"")
        .replace("token inputs:wrapT = \"repeat\"", "token inputs:wrapT = \"mirror\"")
        .replace(
            "float2 inputs:st.connect = </Looks/Fabric/UV.outputs:result>",
            "float2 inputs:st.connect = </Looks/Fabric/Transform.outputs:result>",
        )
        .replace(
            "def Shader \"UV\"",
            r#"def Shader "Transform" {
            uniform token info:id = "UsdTransform2d"
            float2 inputs:in.connect = </Looks/Fabric/UV.outputs:result>
            float2 inputs:scale = (2, 3)
            float2 inputs:translation = (0.25, 0.5)
            float inputs:rotation = 90
            float2 outputs:result
        }
        def Shader "UV""#,
        )
        .replace(
            "float inputs:roughness = 0.65",
            "float inputs:roughness = 0.65\nfloat inputs:opacity = 0.6\nfloat inputs:opacityThreshold = 0.3",
        );
    let files = [
        InputFile {
            name: "model.usda",
            bytes: source.as_bytes(),
        },
        InputFile {
            name: "checker.png",
            bytes: include_bytes!("../../../fixtures/models/checker.png"),
        },
    ];
    let scene = import_scene(Input::Bundle(&files), &Default::default()).unwrap().scene;
    let settings = scene.metadata.materials["/Looks/Fabric"].original["gltf:render_settings"]
        .value
        .to_json();
    assert_eq!(settings["alpha_mode"], "MASK");
    assert!((settings["alpha_cutoff"].as_f64().unwrap() - 0.3).abs() < 1e-6);
    let texture = &settings["textures"]["base_color"];
    assert_eq!(texture["tex_coord"], 1);
    assert_eq!(texture["sampler"]["wrapS"], 33071);
    assert_eq!(texture["sampler"]["wrapT"], 33648);
    assert!((texture["transform"]["offset"][0].as_f64().unwrap() + 2.75).abs() < 1e-12);
    assert!((texture["transform"]["offset"][1].as_f64().unwrap() - 0.5).abs() < 1e-12);
    assert!((texture["transform"]["rotation"].as_f64().unwrap() + std::f64::consts::FRAC_PI_2).abs() < 1e-12);
    let invalid = source.replace("token inputs:wrapS = \"clamp\"", "");
    let invalid_files = [
        InputFile {
            name: "model.usda",
            bytes: invalid.as_bytes(),
        },
        files[1],
    ];
    assert_eq!(
        import_scene(Input::Bundle(&invalid_files), &Default::default())
            .unwrap_err()
            .code,
        "usd.texture_wrap"
    );
}

#[test]
fn corrupted_usdc_token_list_rejected_before_untrusted_allocation() {
    for bytes in [
        include_bytes!("../../../fuzz/regressions/usdc-oversized-token-list.seed").as_slice(),
        include_bytes!("../../../fuzz/regressions/usdc-token-list-original.seed").as_slice(),
    ] {
        let limits = SceneLimits {
            max_bytes: 4 * 1024 * 1024,
            ..Default::default()
        };
        let report = preflight(
            Input::Bytes(bytes),
            &PreflightOptions {
                limits: limits.clone(),
                ..Default::default()
            },
        );
        assert!(!report.complete, "{report:?}");
        assert!(
            report
                .issues
                .iter()
                .any(|issue| issue.detail.contains("budget") || issue.detail.contains("file extent")),
            "{:?}",
            report.issues
        );
        let error = import_scene(
            Input::Bytes(bytes),
            &usd_toolbox_usd::scene::SceneImportOptions { limits },
        )
        .unwrap_err();
        assert!(
            error.to_string().contains("budget") || error.to_string().contains("file extent"),
            "{error}"
        );
    }
}
