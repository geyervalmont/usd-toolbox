use std::ptr::{null, null_mut};
use usd_toolbox_ffi::*;

const SOURCE: &[u8] = br#"#usda 1.0
def Xform "Asset" {
    def Mesh "Triangle" {
        uniform token subdivisionScheme = "none"
        point3f[] points = [(0,0,0),(1,0,0),(0,1,0)]
        int[] faceVertexCounts = [3]
        int[] faceVertexIndices = [0,1,2]
    }
}
"#;

fn open() -> u64 {
    let mut handle = 0;
    assert_eq!(
        unsafe { usd_toolbox_document_open_v1(SOURCE.as_ptr(), SOURCE.len(), null(), 0, &mut handle) },
        USD_TOOLBOX_STATUS_OK
    );
    assert_ne!(handle, 0);
    handle
}
fn request(handle: u64, request: serde_json::Value) -> u64 {
    let bytes = serde_json::to_vec(&request).unwrap();
    let mut result = 0;
    assert_eq!(
        unsafe { usd_toolbox_document_request_v1(handle, bytes.as_ptr(), bytes.len(), &mut result) },
        USD_TOOLBOX_STATUS_OK
    );
    assert_ne!(result, 0);
    result
}
fn copy(handle: u64) -> (Vec<u8>, Vec<u8>) {
    let (mut len, mut report_len) = (0, 0);
    assert_eq!(
        unsafe { usd_toolbox_result_copy_v1(handle, null_mut(), 0, &mut len, null_mut(), 0, &mut report_len) },
        USD_TOOLBOX_STATUS_BUFFER_TOO_SMALL
    );
    let (mut bytes, mut report) = (vec![0; len], vec![0; report_len]);
    assert_eq!(
        unsafe {
            usd_toolbox_result_copy_v1(
                handle,
                bytes.as_mut_ptr(),
                bytes.len(),
                &mut len,
                report.as_mut_ptr(),
                report.len(),
                &mut report_len,
            )
        },
        USD_TOOLBOX_STATUS_OK
    );
    (bytes, report)
}

#[test]
fn retained_document_queries_edits_and_results_have_independent_lifetimes() {
    let handle = open();
    let query = request(
        handle,
        serde_json::json!({"operation":"query","request":{"filter":{"node_type":"mesh"},"fields":["name","metadata"],"limit":1}}),
    );
    let value: serde_json::Value = serde_json::from_slice(&copy(query).0).unwrap();
    assert_eq!(value["rows"][0]["id"], "/Asset/Triangle");
    assert!(value["rows"][0]["fields"].get("positions").is_none());
    assert_eq!(usd_toolbox_result_release_v1(query), USD_TOOLBOX_STATUS_OK);

    let update = request(
        handle,
        serde_json::json!({"operation":"apply","edits":[{"operation":"rename","node":"/Asset/Triangle","name":"日本語 chair"}]}),
    );
    let first = copy(update).0;
    assert_eq!(
        first,
        copy(update).0,
        "buffer sizing and repeated copying do not repeat edits"
    );
    let value: serde_json::Value = serde_json::from_slice(&first).unwrap();
    assert_eq!(value["generation"], 1);
    assert_eq!(usd_toolbox_result_release_v1(update), USD_TOOLBOX_STATUS_OK);
    let noop = request(handle, serde_json::json!({"operation":"apply","edits":[]}));
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&copy(noop).0).unwrap()["generation"],
        1
    );
    assert_eq!(usd_toolbox_result_release_v1(noop), USD_TOOLBOX_STATUS_OK);

    let exported = request(handle, serde_json::json!({"operation":"export","target":"glb"}));
    assert_eq!(usd_toolbox_document_release_v1(handle), USD_TOOLBOX_STATUS_OK);
    assert_eq!(
        usd_toolbox_document_release_v1(handle),
        USD_TOOLBOX_STATUS_INVALID_ARGUMENT
    );
    let (bytes, report) = copy(exported);
    assert!(bytes.starts_with(b"glTF"));
    assert!(serde_json::from_slice::<serde_json::Value>(&report).unwrap().is_array());
    assert_eq!(usd_toolbox_result_release_v1(exported), USD_TOOLBOX_STATUS_OK);
    assert_eq!(
        usd_toolbox_result_release_v1(exported),
        USD_TOOLBOX_STATUS_INVALID_ARGUMENT
    );

    let mut output = 42;
    assert_eq!(
        unsafe { usd_toolbox_document_request_v1(handle, b"{}".as_ptr(), 2, &mut output) },
        USD_TOOLBOX_STATUS_INVALID_ARGUMENT
    );
    assert_eq!(output, 0);
}

#[test]
fn document_handles_validate_arguments_and_scene_json_metadata_integers() {
    let mut handle = 99;
    assert_eq!(
        unsafe { usd_toolbox_document_open_v1(null(), 1, null(), 0, &mut handle) },
        USD_TOOLBOX_STATUS_INVALID_ARGUMENT
    );
    assert_eq!(handle, 0);
    assert_eq!(
        unsafe { usd_toolbox_document_open_v1(SOURCE.as_ptr(), SOURCE.len(), null(), 0, null_mut()) },
        USD_TOOLBOX_STATUS_INVALID_ARGUMENT
    );
    let handle = open();
    let update = request(
        handle,
        serde_json::json!({"operation":"apply","edits":[{"operation":"set_metadata","target":{"scope":"scene"},"namespace":"normalized","key":"product_id","value":{"value":{"type":"u64","value":"18446744073709551615"},"state":"authored"}}]}),
    );
    usd_toolbox_result_release_v1(update);
    let metadata = request(
        handle,
        serde_json::json!({"operation":"metadata","target":{"scope":"scene"}}),
    );
    let value: serde_json::Value = serde_json::from_slice(&copy(metadata).0).unwrap();
    assert_eq!(
        value["normalized"]["product_id"]["value"]["value"],
        "18446744073709551615"
    );
    usd_toolbox_result_release_v1(metadata);
    usd_toolbox_document_release_v1(handle);
}

#[test]
fn borrowed_bundles_preflight_and_capabilities_have_bounded_owned_results() {
    let source = include_bytes!("../../../fixtures/models/panels.usda");
    let checker = include_bytes!("../../../fixtures/models/checker.png");
    let files = [
        UsdToolboxInputFile {
            name: b"panel.usda".as_ptr(),
            name_len: 10,
            data: source.as_ptr(),
            data_len: source.len(),
        },
        UsdToolboxInputFile {
            name: b"checker.png".as_ptr(),
            name_len: 11,
            data: checker.as_ptr(),
            data_len: checker.len(),
        },
    ];
    let mut handle = 0;
    assert_eq!(
        unsafe { usd_toolbox_document_open_bundle_v1(files.as_ptr(), files.len(), null(), 0, &mut handle) },
        USD_TOOLBOX_STATUS_OK
    );
    let summary = request(handle, serde_json::json!({"operation":"summary"}));
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&copy(summary).0).unwrap()["meshes"],
        1
    );
    usd_toolbox_result_release_v1(summary);
    usd_toolbox_document_release_v1(handle);
    assert_eq!(
        unsafe { usd_toolbox_document_open_bundle_v1(null(), 1, null(), 0, &mut handle) },
        USD_TOOLBOX_STATUS_INVALID_ARGUMENT
    );
    assert_eq!(handle, 0);
    let mut size = 0;
    assert_eq!(
        unsafe { usd_toolbox_model_capabilities_v1(null_mut(), 0, &mut size) },
        USD_TOOLBOX_STATUS_BUFFER_TOO_SMALL
    );
    let mut bytes = vec![0; size];
    assert_eq!(
        unsafe { usd_toolbox_model_capabilities_v1(bytes.as_mut_ptr(), bytes.len(), &mut size) },
        USD_TOOLBOX_STATUS_OK
    );
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&bytes).unwrap()["schema"],
        "usd-toolbox.model-capabilities.v1"
    );
    let bad = b"#usda 1.0\ndef Mesh {";
    let mut result = 0;
    assert_eq!(
        unsafe { usd_toolbox_model_preflight_v1(bad.as_ptr(), bad.len(), null(), 0, &mut result) },
        USD_TOOLBOX_STATUS_OK
    );
    let report: serde_json::Value = serde_json::from_slice(&copy(result).0).unwrap();
    assert_eq!(report["complete"], false);
    usd_toolbox_result_release_v1(result);
}

#[test]
fn lod_archives_are_prepared_once_bounded_and_independent_of_document_lifetime() {
    use sha2::{Digest, Sha256};
    use std::io::{Cursor, Read};
    let handle = open();
    let config = serde_json::json!({"operation":"export_lods","target":"glb","options":{"levels":[
        {"name":"medium","face_ratio":0.75,"max_vertex_displacement":0.1},
        {"name":"low","face_ratio":0.25,"max_vertex_displacement":0.2}
    ]}});
    let output = request(handle, config.clone());
    let (bytes, report) = copy(output);
    assert_eq!(
        copy(output),
        (bytes.clone(), report.clone()),
        "sizing and copying retain exact prepared output"
    );
    let manifest: serde_json::Value = serde_json::from_slice(&report).unwrap();
    assert_eq!(manifest["schema"], "usd-toolbox.geometry-lods.v1");
    let mut archive = zip::ZipArchive::new(Cursor::new(&bytes)).unwrap();
    assert_eq!(archive.len(), 3);
    let mut packed_report = Vec::new();
    archive
        .by_name("manifest.json")
        .unwrap()
        .read_to_end(&mut packed_report)
        .unwrap();
    assert_eq!(packed_report, report);
    for file in manifest["files"].as_array().unwrap() {
        let mut encoded = Vec::new();
        archive
            .by_name(file["name"].as_str().unwrap())
            .unwrap()
            .read_to_end(&mut encoded)
            .unwrap();
        assert!(encoded.starts_with(b"glTF"));
        assert_eq!(file["sha256"], format!("{:x}", Sha256::digest(&encoded)));
    }
    let repeated = request(handle, config.clone());
    assert_eq!(
        copy(repeated),
        (bytes.clone(), report.clone()),
        "archive output is deterministic"
    );
    usd_toolbox_result_release_v1(repeated);
    // Encoded levels fit this budget, but ZIP headers plus the retained report do not.
    let mut limited = config.clone();
    limited["options"]["max_output_bytes"] = serde_json::json!(bytes.len() + report.len() - 1);
    let request_bytes = serde_json::to_vec(&limited).unwrap();
    let mut failed = 99;
    assert_ne!(
        unsafe { usd_toolbox_document_request_v1(handle, request_bytes.as_ptr(), request_bytes.len(), &mut failed) },
        USD_TOOLBOX_STATUS_OK
    );
    assert_eq!(failed, 0);
    let summary = request(handle, serde_json::json!({"operation":"query"}));
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&copy(summary).0).unwrap()["generation"],
        0
    );
    usd_toolbox_result_release_v1(summary);
    let mut unsupported = config;
    unsupported["target"] = serde_json::json!("gltf");
    let request_bytes = serde_json::to_vec(&unsupported).unwrap();
    assert_eq!(
        unsafe { usd_toolbox_document_request_v1(handle, request_bytes.as_ptr(), request_bytes.len(), &mut failed) },
        USD_TOOLBOX_STATUS_INVALID_ARGUMENT
    );
    assert_eq!(usd_toolbox_document_release_v1(handle), USD_TOOLBOX_STATUS_OK);
    assert_eq!(copy(output), (bytes, report));
    assert_eq!(usd_toolbox_result_release_v1(output), USD_TOOLBOX_STATUS_OK);
}

#[test]
fn gltf_preflight_applies_profile_geometry_and_diagnostic_cap() {
    fn scan(bytes: &[u8], name: &str, profile: serde_json::Value) -> serde_json::Value {
        let options = serde_json::to_vec(&serde_json::json!({"name":name,"profile":profile})).unwrap();
        let mut result = 0;
        assert_eq!(
            unsafe {
                usd_toolbox_model_preflight_v1(
                    bytes.as_ptr(),
                    bytes.len(),
                    options.as_ptr(),
                    options.len(),
                    &mut result,
                )
            },
            USD_TOOLBOX_STATUS_OK
        );
        let report = serde_json::from_slice(&copy(result).0).unwrap();
        usd_toolbox_result_release_v1(result);
        report
    }
    let scene = usd_toolbox_usd::scene::import_scene(usd_toolbox_core::Input::Bytes(SOURCE), &Default::default())
        .unwrap()
        .scene;
    let glb = usd_toolbox_gltf::scene::export_scene(&scene, &Default::default())
        .unwrap()
        .bytes;
    assert_eq!(
        scan(&glb, "model.glb", serde_json::json!({}))["validation"]["geometry"],
        "deferred"
    );
    let report = scan(
        &glb,
        "model.glb",
        serde_json::json!({"check_geometry":true,"target":"browser_preview"}),
    );
    assert_eq!(report["complete"], true);
    assert_eq!(report["target"], "browser_preview");
    assert_eq!(report["validation"]["geometry"], "performed");
    assert_eq!(report["validation"]["target"], "performed");
    let failed = scan(
        br#"{"asset":{"version":"2.0"},"nodes":[{"mesh":99}],"scenes":[{"nodes":[0]}],"scene":0}"#,
        "invalid.gltf",
        serde_json::json!({"check_geometry":true}),
    );
    assert_eq!(failed["complete"], false);
    assert_eq!(failed["validation"]["geometry"], "failed");
    let capped = scan(
        br#"{"asset":{"version":"2.0"},"cameras":[{}],"skins":[{}],"animations":[{}]}"#,
        "many.gltf",
        serde_json::json!({"max_issues":1}),
    );
    assert_eq!(capped["diagnostics"].as_array().unwrap().len(), 1);
    assert_eq!(capped["diagnostics_truncated"], true);
    assert_eq!(capped["complete"], false);
}
