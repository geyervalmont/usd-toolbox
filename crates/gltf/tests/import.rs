use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use usd_toolbox_core::scene::*;
use usd_toolbox_gltf::{GltfFormat, scene::*};

fn triangle() -> (Value, Vec<u8>) {
    let positions = [[0f32, 0., 0.], [1., 0., 0.], [0., 1., 0.]];
    let mut bytes = positions
        .into_iter()
        .flatten()
        .flat_map(f32::to_le_bytes)
        .collect::<Vec<_>>();
    bytes.extend([0u8, 1, 2, 0]);
    (
        json!({"asset":{"version":"2.0"},"buffers":[{"uri":"mesh.bin","byteLength":40}],
        "bufferViews":[{"buffer":0,"byteLength":36},{"buffer":0,"byteOffset":36,"byteLength":3}],
        "accessors":[{"bufferView":0,"componentType":5126,"count":3,"type":"VEC3","min":[0,0,0],"max":[1,1,0]},
        {"bufferView":1,"componentType":5121,"count":3,"type":"SCALAR"}],
        "meshes":[{"primitives":[{"attributes":{"POSITION":0},"indices":1}]}],
        "nodes":[{"name":"Triangle","mesh":0,"translation":[2,0,0],"extras":{"product":{"id":"chair-1"},"wide":18446744073709551615u64}}],
        "scenes":[{"nodes":[0]}],"scene":0}),
        bytes,
    )
}
fn load(doc: &Value, bytes: &[u8]) -> Result<SceneImport, SceneError> {
    import_scene_bundle(
        "models/root.gltf",
        &BTreeMap::from([
            ("models/root.gltf".into(), serde_json::to_vec(doc).unwrap()),
            ("models/mesh.bin".into(), bytes.to_vec()),
        ]),
        &Default::default(),
    )
}
fn embedded(doc: &Value, bytes: &[u8]) -> Vec<u8> {
    let mut doc = doc.clone();
    doc["buffers"][0]["uri"] = json!(format!("data:application/octet-stream;base64,{}", BASE64.encode(bytes)));
    serde_json::to_vec(&doc).unwrap()
}
#[test]
fn imports_external_embedded_and_glb_preserving_bounds_metadata_and_identity() {
    let (doc, bytes) = triangle();
    assert_eq!(
        discover_dependencies(
            &serde_json::to_vec(&doc).unwrap(),
            "models/root.gltf",
            &Default::default()
        )
        .unwrap(),
        vec!["models/mesh.bin"]
    );
    let imported = load(&doc, &bytes).unwrap();
    let mesh = imported.scene.meshes.values().next().unwrap();
    assert_eq!(mesh.face_indices, vec![0, 1, 2]);
    assert_eq!(imported.scene.dependencies[0].bytes, 40);
    assert_eq!(imported.scene.nodes["gltf/nodes/0"].transform[0][3], 2.);
    let metadata = &imported.scene.metadata.nodes["gltf/nodes/0"].original["gltf:extras"].value;
    assert_eq!(metadata.to_json()["wide"].as_u64(), Some(u64::MAX));
    let inline = import_scene(&embedded(&doc, &bytes), "inline.gltf", &Default::default()).unwrap();
    assert_eq!(inline.scene.meshes, imported.scene.meshes);
    let output = export_scene(&imported.scene, &Default::default()).unwrap();
    let roundtrip = import_scene(&output.bytes, "roundtrip.glb", &Default::default()).unwrap();
    assert_eq!(roundtrip.scene.id, imported.scene.id);
    assert_eq!(roundtrip.scene.nodes["gltf/nodes/0"].name, "Triangle");
    assert_eq!(
        &roundtrip.scene.metadata.nodes["gltf/nodes/0"].original["gltf:extras"].value,
        metadata
    );
}
#[test]
fn rejects_missing_escaping_dependencies_and_unsupported_features() {
    let (mut doc, bytes) = triangle();
    assert_eq!(
        import_scene(&serde_json::to_vec(&doc).unwrap(), "test.gltf", &Default::default())
            .unwrap_err()
            .code,
        "gltf.dependency_missing"
    );
    for path in [
        "../mesh.bin",
        "https://host/file",
        "%2e%2e/mesh.bin",
        "/mesh.bin",
        "a\\mesh.bin",
        "mesh.bin?x",
    ] {
        doc["buffers"][0]["uri"] = json!(path);
        assert_eq!(load(&doc, &bytes).unwrap_err().code, "gltf.dependency_path");
    }
    let (mut doc, bytes) = triangle();
    doc["extensionsRequired"] = json!(["KHR_draco_mesh_compression"]);
    assert_eq!(load(&doc, &bytes).unwrap_err().code, "gltf.unsupported");
    let (mut doc, bytes) = triangle();
    doc["animations"] = json!([{}]);
    assert_eq!(load(&doc, &bytes).unwrap_err().code, "gltf.unsupported");
    let (mut doc, bytes) = triangle();
    doc["nodes"][0]["children"] = json!([0]);
    assert_eq!(load(&doc, &bytes).unwrap_err().code, "gltf.invalid");
}
#[test]
fn interleaved_and_sparse_accessors_and_normalized_uvs() {
    let (mut doc, _) = triangle();
    let mut bytes = Vec::new();
    for p in [[0f32, 0., 0.], [1., 0., 0.], [0., 1., 0.]] {
        bytes.extend(p.into_iter().chain([99.]).flat_map(f32::to_le_bytes));
    }
    bytes.extend([0u8, 1, 2, 0]);
    bytes.extend([0u16, 0, 65535, 0, 0, 65535].into_iter().flat_map(u16::to_le_bytes));
    doc["buffers"][0]["byteLength"] = json!(bytes.len());
    doc["bufferViews"] = json!([{"buffer":0,"byteLength":48,"byteStride":16},{"buffer":0,"byteOffset":48,"byteLength":3},{"buffer":0,"byteOffset":52,"byteLength":12}]);
    doc["accessors"]
        .as_array_mut()
        .unwrap()
        .push(json!({"bufferView":2,"componentType":5123,"normalized":true,"count":3,"type":"VEC2"}));
    doc["meshes"][0]["primitives"][0]["attributes"]["TEXCOORD_0"] = json!(2);
    let result = load(&doc, &bytes).unwrap();
    assert_eq!(
        result.scene.meshes.values().next().unwrap().uv_sets["st"].values,
        vec![[0., 1.], [1., 1.], [0., 0.]]
    );
    // Replace tightly packed POSITION with a zero-base accessor plus sparse writes.
    let (mut doc, bytes) = triangle();
    doc["accessors"][0].as_object_mut().unwrap().remove("bufferView");
    doc["accessors"][0]["sparse"] =
        json!({"count":3,"indices":{"bufferView":1,"componentType":5121},"values":{"bufferView":0}});
    let result = load(&doc, &bytes).unwrap();
    assert_eq!(result.scene.meshes.values().next().unwrap().positions[1], [1., 0., 0.]);
    let mut malformed = bytes.clone();
    malformed[37] = 0;
    assert!(
        load(&doc, &malformed)
            .unwrap_err()
            .detail
            .contains("strictly increasing")
    );
}
#[test]
fn checks_accessor_bounds_strides_sizes_and_resource_budgets() {
    let (mut doc, bytes) = triangle();
    doc["accessors"][0]["count"] = json!(u64::MAX);
    assert!(load(&doc, &bytes).is_err());
    let (mut doc, bytes) = triangle();
    doc["accessors"][0]["max"] = json!([0, 0, 0]);
    assert!(load(&doc, &bytes).unwrap_err().detail.contains("bounds"));
    let (mut doc, bytes) = triangle();
    doc["bufferViews"][0]["byteStride"] = json!(8);
    assert!(load(&doc, &bytes).is_err());
    let (mut doc, bytes) = triangle();
    doc["accessors"][1]["normalized"] = json!(true);
    assert!(load(&doc, &bytes).is_err());
    let (doc, bytes) = triangle();
    let mut options = SceneGltfImportOptions::default();
    options.limits.max_points = 2;
    assert_eq!(
        import_scene(&embedded(&doc, &bytes), "input.gltf", &options)
            .unwrap_err()
            .code,
        "scene.limit"
    );
    let (mut doc, bytes) = triangle();
    doc["bufferViews"][0]["byteOffset"] = json!(u64::MAX);
    assert!(load(&doc, &bytes).is_err());
}
#[test]
fn shared_mesh_placements_and_strip_winding_are_preserved() {
    let (mut doc, bytes) = triangle();
    doc["nodes"]
        .as_array_mut()
        .unwrap()
        .push(json!({"name":"Copy","mesh":0,"scale":[-1,2,1]}));
    doc["scenes"][0]["nodes"] = json!([0, 1]);
    doc["meshes"][0]["primitives"][0]["mode"] = json!(5);
    let result = load(&doc, &bytes).unwrap();
    assert_eq!(result.scene.meshes.len(), 1);
    assert_eq!(result.scene.nodes.len(), 2);
    assert_eq!(
        result.scene.nodes["gltf/nodes/0"].mesh,
        result.scene.nodes["gltf/nodes/1"].mesh
    );
    let output = export_scene(
        &result.scene,
        &SceneGltfOptions {
            format: GltfFormat::Gltf,
            ..Default::default()
        },
    )
    .unwrap();
    let doc: Value = serde_json::from_slice(&output.bytes).unwrap();
    assert_eq!(doc["meshes"].as_array().unwrap().len(), 1);
    assert!(doc["meshes"][0]["primitives"][0]["indices"].is_number());
}
#[test]
fn malformed_glb_chunks_fail_without_panicking() {
    let (doc, bytes) = triangle();
    let imported = load(&doc, &bytes).unwrap();
    let glb = export_scene(&imported.scene, &Default::default()).unwrap().bytes;
    for length in [0, 1, 4, 8, 12, 16, glb.len() - 1] {
        assert!(import_scene(&glb[..length], "x.glb", &Default::default()).is_err());
    }
    let mut bad = glb.clone();
    bad[12..16].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(import_scene(&bad, "x.glb", &Default::default()).is_err());
}

#[test]
fn textured_normal_tangents_cutout_sampler_and_uv_transform_roundtrip() {
    let (mut doc, mut bytes) = triangle();
    let offset = bytes.len();
    bytes.extend([0f32, 0., 1.].repeat(3).into_iter().flat_map(f32::to_le_bytes));
    bytes.extend([1f32, 0., 0., -1.].repeat(3).into_iter().flat_map(f32::to_le_bytes));
    bytes.extend([0f32, 0., 1., 0., 0., 1.].into_iter().flat_map(f32::to_le_bytes));
    doc["bufferViews"].as_array_mut().unwrap().extend([
        json!({"buffer":0,"byteOffset":offset,"byteLength":36}),
        json!({"buffer":0,"byteOffset":offset+36,"byteLength":48}),
        json!({"buffer":0,"byteOffset":offset+84,"byteLength":24}),
    ]);
    doc["accessors"].as_array_mut().unwrap().extend([
        json!({"bufferView":2,"componentType":5126,"count":3,"type":"VEC3"}),
        json!({"bufferView":3,"componentType":5126,"count":3,"type":"VEC4"}),
        json!({"bufferView":4,"componentType":5126,"count":3,"type":"VEC2"}),
    ]);
    let attrs = &mut doc["meshes"][0]["primitives"][0]["attributes"];
    attrs["NORMAL"] = json!(2);
    attrs["TANGENT"] = json!(3);
    attrs["TEXCOORD_0"] = json!(4);
    doc["buffers"][0]["byteLength"] = json!(bytes.len());
    let mut png = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(2, 2, image::Rgba([128, 128, 255, 128])))
        .write_to(&mut png, image::ImageFormat::Png)
        .unwrap();
    doc["images"] = json!([{"uri":format!("data:image/png;base64,{}",BASE64.encode(png.into_inner()))}]);
    doc["textures"] = json!([{"source":0,"sampler":0}]);
    doc["samplers"] = json!([{"wrapS":33071,"wrapT":33648,"magFilter":9728}]);
    doc["materials"] = json!([{"alphaMode":"MASK","alphaCutoff":0.3,"pbrMetallicRoughness":{"baseColorTexture":{"index":0,"extensions":{"KHR_texture_transform":{"offset":[0.2,0.3],"scale":[2,2],"rotation":0.5}}}},"normalTexture":{"index":0,"scale":0.4}}]);
    doc["extensionsUsed"] = json!(["KHR_texture_transform"]);
    doc["meshes"][0]["primitives"][0]["material"] = json!(0);
    let imported = load(&doc, &bytes).unwrap();
    let m = imported.scene.meshes.values().next().unwrap();
    assert_eq!(m.tangents.as_ref().unwrap().values[0], [1., 0., 0., 1.]);
    let output = export_scene(
        &imported.scene,
        &SceneGltfOptions {
            format: GltfFormat::Gltf,
            ..Default::default()
        },
    )
    .unwrap();
    let encoded: Value = serde_json::from_slice(&output.bytes).unwrap();
    assert_eq!(encoded["materials"][0]["alphaMode"], "MASK");
    assert!((encoded["materials"][0]["alphaCutoff"].as_f64().unwrap() - 0.3).abs() < 1e-6);
    assert_eq!(
        encoded["materials"][0]["pbrMetallicRoughness"]["baseColorTexture"]["extensions"]["KHR_texture_transform"]["rotation"],
        0.5
    );
    let texture = encoded["materials"][0]["normalTexture"]["index"].as_u64().unwrap() as usize;
    let sampler = encoded["textures"][texture]["sampler"].as_u64().unwrap() as usize;
    assert_eq!(encoded["samplers"][sampler]["wrapS"], 33071);
    let reimport = import_scene(&output.bytes, "roundtrip.gltf", &Default::default()).unwrap();
    assert_eq!(reimport.scene.meshes.values().next().unwrap().tangents, m.tangents);
}

#[test]
fn preflight_accumulates_features_without_resolving_or_decoding_resources() {
    let (mut doc, _) = triangle();
    doc["animations"] = json!([{}]);
    doc["skins"] = json!([{}]);
    doc["extensionsRequired"] = json!(["KHR_draco_mesh_compression"]);
    doc["meshes"][0]["primitives"][0]["targets"] = json!([{}]);
    let report = preflight_scene(
        &serde_json::to_vec(&doc).unwrap(),
        "model/root.gltf",
        &Default::default(),
    )
    .unwrap();
    assert_eq!(report["complete"], true);
    assert_eq!(report["compatible"], false);
    assert_eq!(report["dependencies"], json!(["model/mesh.bin"]));
    assert_eq!(report["validation"]["geometry"], "deferred");
    for code in [
        "gltf.animations",
        "gltf.skins",
        "gltf.morph_targets",
        "gltf.unsupported_extension",
    ] {
        assert!(
            report["diagnostics"]
                .as_array()
                .unwrap()
                .iter()
                .any(|d| d["code"] == code),
            "{code}"
        );
    }
    assert_eq!(gltf_scene_capabilities()["portable"], true);
}
