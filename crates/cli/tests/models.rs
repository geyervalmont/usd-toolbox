use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{fs, process::Command};
use tempfile::tempdir;
use usd_toolbox_core::{Exporter, Importer, Input, Material};
use usd_toolbox_usd::{UsdExporter, UsdImporter};

fn run(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_usd-toolbox"))
        .arg("model")
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn model_pipeline_pins_materials_exports_sidecars_and_rejects_overwrites() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    let source = root.join("panels.usda");
    fs::write(&source, include_bytes!("../../../fixtures/models/panels.usda")).unwrap();
    fs::write(
        root.join("checker.png"),
        include_bytes!("../../../fixtures/models/checker.png"),
    )
    .unwrap();
    let package = root.join("model.usdz");
    let report = root.join("report.json");
    let result = run(&[
        "pack",
        "--input",
        source.to_str().unwrap(),
        "--output",
        package.to_str().unwrap(),
        "--report",
        report.to_str().unwrap(),
        "--id",
        "catalogue-model",
    ]);
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    let report: Value = serde_json::from_slice(&fs::read(report).unwrap()).unwrap();
    assert_eq!(report["inspection"]["id"], "catalogue-model");
    assert_eq!(report["inspection"]["placed_faces"], 2);
    let bytes = fs::read(&package).unwrap();
    assert_eq!(report["output"]["sha256"], format!("{:x}", Sha256::digest(&bytes)));
    assert!(
        !run(&[
            "pack",
            "--input",
            source.to_str().unwrap(),
            "--output",
            source.to_str().unwrap()
        ])
        .status
        .success()
    );
    assert_eq!(
        fs::read(&source).unwrap(),
        include_bytes!("../../../fixtures/models/panels.usda")
    );
    let layer = root.join("export/scene.usda");
    let report = root.join("sidecars.json");
    assert!(
        run(&[
            "export",
            "--input",
            package.to_str().unwrap(),
            "--target",
            "usda",
            "--output",
            layer.to_str().unwrap(),
            "--report",
            report.to_str().unwrap()
        ])
        .status
        .success()
    );
    let report: Value = serde_json::from_slice(&fs::read(report).unwrap()).unwrap();
    assert!(!report["assets"].as_array().unwrap().is_empty());
    for asset in report["assets"].as_array().unwrap() {
        assert!(root.join("export").join(asset["path"].as_str().unwrap()).is_file());
    }
    let material = Material::new("library-wood", "Published wood");
    let pinned = UsdExporter.export(&[material], &Default::default()).unwrap().bytes;
    fs::write(root.join("wood.usdz"), &pinned).unwrap();
    let manifest = root.join("bindings.json");
    let mut binding = json!([{"source_material":"/Looks/Fabric","package":"wood.usdz","sha256":format!("{:x}",Sha256::digest(&pinned))}]);
    fs::write(&manifest, serde_json::to_vec(&binding).unwrap()).unwrap();
    let output = root.join("bound.usdz");
    let result = run(&[
        "pack",
        "--input",
        source.to_str().unwrap(),
        "--output",
        output.to_str().unwrap(),
        "--material-bindings",
        manifest.to_str().unwrap(),
    ]);
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    let bytes = fs::read(output).unwrap();
    let materials = UsdImporter.import(Input::Bytes(&bytes), &Default::default()).unwrap();
    assert!(materials.iter().any(|m| m.id.0 == "library-wood"));
    let scene = usd_toolbox_usd::scene::import_scene(Input::Bytes(&bytes), &Default::default())
        .unwrap()
        .scene;
    assert_eq!(
        scene.nodes["/Panels/Surface"].material.as_ref().unwrap().0,
        "library-wood"
    );
    binding[0]["sha256"] = json!("0".repeat(64));
    fs::write(&manifest, serde_json::to_vec(&binding).unwrap()).unwrap();
    assert!(
        !run(&[
            "pack",
            "--input",
            source.to_str().unwrap(),
            "--output",
            root.join("bad.usdz").to_str().unwrap(),
            "--material-bindings",
            manifest.to_str().unwrap()
        ])
        .status
        .success()
    );
    assert!(!root.join("bad.usdz").exists());
}

#[test]
fn missing_and_escaping_dependencies_never_produce_output() {
    let dir = tempdir().unwrap();
    let source = dir.path().join("model.usda");
    let output = dir.path().join("model.usdz");
    for path in ["missing.png", "../escape.png", "/etc/passwd"] {
        let text = include_str!("../../../fixtures/models/panels.usda").replace("checker.png", path);
        fs::write(&source, text).unwrap();
        assert!(
            !run(&[
                "pack",
                "--input",
                source.to_str().unwrap(),
                "--output",
                output.to_str().unwrap()
            ])
            .status
            .success()
        );
        assert!(!output.exists());
    }
}

#[test]
fn query_pages_survive_processes_and_metadata_remains_exact() {
    let dir = tempdir().unwrap();
    let source = dir.path().join("日本語-model.usda");
    fs::write(
        &source,
        include_bytes!("../../../fixtures/models/metadata-semantics.usda"),
    )
    .unwrap();
    let request_path = dir.path().join("query.json");
    let mut request = json!({"fields":["name","source_path"],"limit":1});
    fs::write(&request_path, serde_json::to_vec(&request).unwrap()).unwrap();
    let query = || {
        run(&[
            "query",
            "--input",
            source.to_str().unwrap(),
            "--request",
            request_path.to_str().unwrap(),
        ])
    };
    let first = query();
    assert!(first.status.success(), "{}", String::from_utf8_lossy(&first.stderr));
    let first: Value = serde_json::from_slice(&first.stdout).unwrap();
    assert!(first["next_cursor"].is_string());
    assert!(!String::from_utf8_lossy(&serde_json::to_vec(&first).unwrap()).contains("positions"));
    request["cursor"] = first["next_cursor"].clone();
    fs::write(&request_path, serde_json::to_vec(&request).unwrap()).unwrap();
    let second = query();
    assert!(second.status.success(), "{}", String::from_utf8_lossy(&second.stderr));
    let second: Value = serde_json::from_slice(&second.stdout).unwrap();
    assert_ne!(first["rows"][0]["id"], second["rows"][0]["id"]);
    let metadata = run(&[
        "metadata",
        "--input",
        source.to_str().unwrap(),
        "--node",
        "/Product",
        "--namespace",
        "original",
        "--key",
        "usd:attribute:product:serial",
    ]);
    assert!(
        metadata.status.success(),
        "{}",
        String::from_utf8_lossy(&metadata.stderr)
    );
    let value: Value = serde_json::from_slice(&metadata.stdout).unwrap();
    assert!(serde_json::to_string(&value).unwrap().contains("9007199254740993"));
    let changed = fs::read_to_string(&source)
        .unwrap()
        .replace("9007199254740993", "9007199254740994");
    fs::write(&source, changed).unwrap();
    let stale = query();
    assert!(!stale.status.success());
    assert!(String::from_utf8_lossy(&stale.stderr).contains("stale_cursor"));
}

#[test]
fn preflight_resolves_confined_assets_and_preserves_multiple_findings() {
    let dir = tempdir().unwrap();
    let source = dir.path().join("panel.usda");
    fs::write(&source, include_bytes!("../../../fixtures/models/panels.usda")).unwrap();
    fs::write(
        dir.path().join("checker.png"),
        include_bytes!("../../../fixtures/models/checker.png"),
    )
    .unwrap();
    let output = run(&[
        "preflight",
        "--input",
        source.to_str().unwrap(),
        "--target",
        "glb",
        "--geometry",
    ]);
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(
        report["dependencies"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v["status"] == "supplied")
    );
    let invalid = include_str!("../../../fixtures/models/metadata-semantics.usda")
        .replace("metersPerUnit = 1", "subLayers = [@missing.usda@]\nmetersPerUnit = 1")
        .replace("subdivisionScheme = \"none\"", "subdivisionScheme = \"catmullClark\"");
    fs::write(&source, invalid).unwrap();
    let output = run(&["preflight", "--input", source.to_str().unwrap()]);
    assert!(!output.status.success());
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(report["issues"].as_array().unwrap().len() > 1);
    assert!(report["features"].as_array().unwrap().contains(&json!("sublayers")));
}

#[test]
fn lod_packages_are_new_deterministic_directories_with_verified_files() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    let source = root.join("source.usda");
    fs::write(&source, include_bytes!("../../../fixtures/models/panels.usda")).unwrap();
    fs::write(
        root.join("checker.png"),
        include_bytes!("../../../fixtures/models/checker.png"),
    )
    .unwrap();
    let original = fs::read(&source).unwrap();
    let options = root.join("lod.json");
    fs::write(
        &options,
        serde_json::to_vec(&json!({"levels":[
            {"name":"medium","face_ratio":0.75,"max_vertex_displacement":0.1},
            {"name":"low","face_ratio":0.25,"max_vertex_displacement":0.2}
        ]}))
        .unwrap(),
    )
    .unwrap();
    for target in ["glb", "usdz"] {
        let output = root.join(target);
        let invoke = || {
            run(&[
                "lod",
                "--input",
                source.to_str().unwrap(),
                "--options",
                options.to_str().unwrap(),
                "--output-dir",
                output.to_str().unwrap(),
                "--target",
                target,
            ])
        };
        let result = invoke();
        assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
        let manifest_bytes = fs::read(output.join("manifest.json")).unwrap();
        let manifest: Value = serde_json::from_slice(&manifest_bytes).unwrap();
        assert_eq!(manifest["schema"], "usd-toolbox.geometry-lods.v1");
        assert_eq!(manifest["files"][0]["name"], format!("lod-0.{target}"));
        assert_eq!(manifest["files"][1]["level"], "low");
        assert_eq!(manifest["source_scene_sha256"].as_str().unwrap().len(), 64);
        for file in manifest["files"].as_array().unwrap() {
            let bytes = fs::read(output.join(file["name"].as_str().unwrap())).unwrap();
            assert_eq!(file["bytes"].as_u64().unwrap(), bytes.len() as u64);
            assert_eq!(file["sha256"], format!("{:x}", Sha256::digest(&bytes)));
            let scene = if target == "glb" {
                usd_toolbox_gltf::scene::import_scene(&bytes, "level.glb", &Default::default())
                    .unwrap()
                    .scene
            } else {
                usd_toolbox_usd::scene::import_scene(Input::Bytes(&bytes), &Default::default())
                    .unwrap()
                    .scene
            };
            assert!(!scene.meshes.is_empty());
        }
        assert!(!invoke().status.success(), "existing package must not be overwritten");
        assert_eq!(fs::read(output.join("manifest.json")).unwrap(), manifest_bytes);
        let second = root.join(format!("{target}-repeated"));
        let repeated = run(&[
            "lod",
            "--input",
            source.to_str().unwrap(),
            "--options",
            options.to_str().unwrap(),
            "--output-dir",
            second.to_str().unwrap(),
            "--target",
            target,
        ]);
        assert!(
            repeated.status.success(),
            "{}",
            String::from_utf8_lossy(&repeated.stderr)
        );
        assert_eq!(fs::read(second.join("manifest.json")).unwrap(), manifest_bytes);
    }
    assert_eq!(fs::read(source).unwrap(), original);
    assert!(fs::read_dir(root).unwrap().all(|entry| {
        !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".usd-toolbox-lods-")
    }));
}
#[test]
fn lod_failure_never_publishes_or_overwrites_an_existing_empty_directory() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    let source = root.join("source.usda");
    fs::write(&source, b"#usda 1.0\ndef Mesh \"Triangle\" {\n uniform token subdivisionScheme = \"none\"\n point3f[] points = [(0,0,0),(1,0,0),(0,1,0)]\n int[] faceVertexCounts = [3]\n int[] faceVertexIndices = [0,1,2]\n}\n").unwrap();
    let options = root.join("lod.json");
    let config = json!({"levels":[{"name":"low","face_ratio":0.5,"max_vertex_displacement":1.0}],"max_output_bytes":1});
    fs::write(&options, serde_json::to_vec(&config).unwrap()).unwrap();
    let output = root.join("failed");
    let invoke = || {
        run(&[
            "lod",
            "--input",
            source.to_str().unwrap(),
            "--options",
            options.to_str().unwrap(),
            "--output-dir",
            output.to_str().unwrap(),
        ])
    };
    assert!(!invoke().status.success());
    assert!(!output.exists());
    fs::create_dir(&output).unwrap();
    assert!(!invoke().status.success());
    assert_eq!(fs::read_dir(&output).unwrap().count(), 0);
    let result = run(&[
        "lod",
        "--input",
        source.to_str().unwrap(),
        "--options",
        options.to_str().unwrap(),
        "--output-dir",
        source.to_str().unwrap(),
    ]);
    assert!(!result.status.success());
    assert!(source.is_file());
}

#[test]
fn gltf_preflight_reports_requested_checks_caps_and_failures() {
    let dir = tempdir().unwrap();
    let source = dir.path().join("model.glb");
    let scene = usd_toolbox_usd::scene::import_scene(
        Input::Bytes(include_bytes!("../../../fixtures/models/metadata-semantics.usda")),
        &Default::default(),
    )
    .unwrap()
    .scene;
    let output = usd_toolbox_gltf::scene::export_scene(&scene, &Default::default()).unwrap();
    fs::write(&source, output.bytes).unwrap();
    let scanned = run(&["preflight", "--input", source.to_str().unwrap()]);
    assert!(scanned.status.success(), "{}", String::from_utf8_lossy(&scanned.stderr));
    let scanned: Value = serde_json::from_slice(&scanned.stdout).unwrap();
    assert_eq!(scanned["validation"]["geometry"], "deferred");
    let checked = run(&[
        "preflight",
        "--input",
        source.to_str().unwrap(),
        "--geometry",
        "--target",
        "glb",
    ]);
    assert!(checked.status.success(), "{}", String::from_utf8_lossy(&checked.stdout));
    let checked: Value = serde_json::from_slice(&checked.stdout).unwrap();
    assert_eq!(checked["target"], "glb");
    assert_eq!(checked["validation"]["geometry"], "performed");
    assert_eq!(checked["validation"]["buffers"], "performed");
    assert_eq!(checked["validation"]["target"], "performed");
    assert_eq!(checked["validation"]["images"], "performed_preview_decode");
    assert!(checked["geometry_inspection"]["unique_points"].as_u64().unwrap() > 0);

    let source = dir.path().join("invalid.gltf");
    fs::write(
        &source,
        br#"{"asset":{"version":"2.0"},"nodes":[{"mesh":99}],"scenes":[{"nodes":[0]}],"scene":0}"#,
    )
    .unwrap();
    let failed = run(&["preflight", "--input", source.to_str().unwrap(), "--geometry"]);
    assert!(!failed.status.success());
    let failed: Value = serde_json::from_slice(&failed.stdout).unwrap();
    assert_eq!(failed["complete"], false);
    assert_eq!(failed["validation"]["geometry"], "failed");
    fs::write(
        &source,
        br#"{"asset":{"version":"2.0"},"cameras":[{}],"skins":[{}],"animations":[{}]}"#,
    )
    .unwrap();
    let capped = run(&["preflight", "--input", source.to_str().unwrap(), "--max-issues", "1"]);
    assert!(!capped.status.success());
    let capped: Value = serde_json::from_slice(&capped.stdout).unwrap();
    assert_eq!(capped["complete"], false);
    assert_eq!(capped["diagnostics_truncated"], true);
    assert_eq!(capped["diagnostics"].as_array().unwrap().len(), 1);
}
