use serde_json::Value;
use usd_toolbox_core::{Material, MaterialId, scene::*};
use usd_toolbox_gltf::{GltfFormat, scene::*};

fn scene() -> Scene {
    let mut scene = Scene::new("test");
    scene.meters_per_unit = 0.01;
    scene.up_axis = UpAxis::Z;
    let mut mesh = Mesh {
        positions: vec![[0., 0., 0.], [2., 0., 0.], [2., 2., 0.], [1., 1., 0.], [0., 2., 0.]],
        face_counts: vec![5],
        face_indices: vec![0, 1, 2, 3, 4],
        double_sided: true,
        ..Default::default()
    };
    mesh.uv_sets.insert(
        "st".into(),
        Attribute {
            interpolation: Interpolation::FaceVarying,
            values: vec![[0., 0.], [1., 0.], [1., 1.], [0.5, 0.5], [0., 1.]],
            indices: None,
        },
    );
    scene.meshes.insert("shared".into(), mesh);
    scene.materials.push(Material::new("wood", "Wood"));
    let mut node = SceneNode::new("Part");
    node.mesh = Some("shared".into());
    node.material = Some(MaterialId::new("wood"));
    node.transform[0][0] = -2.;
    node.transform[1][1] = 3.;
    scene.nodes.insert("part".into(), node.clone());
    node.transform[0][3] = 10.;
    scene.nodes.insert("copy".into(), node);
    scene
}

#[test]
fn exports_concave_geometry_shared_meshes_seams_materials_and_unit_root() {
    let scene = scene();
    let options = SceneGltfOptions {
        format: GltfFormat::Gltf,
        ..Default::default()
    };
    let output = export_scene(&scene, &options).unwrap();
    let document: Value = serde_json::from_slice(&output.bytes).unwrap();
    assert_eq!(document["meshes"].as_array().unwrap().len(), 1);
    assert_eq!(document["nodes"][0]["mesh"], document["nodes"][1]["mesh"]);
    let primitive = &document["meshes"][0]["primitives"][0];
    let accessor = primitive["attributes"]["POSITION"].as_u64().unwrap() as usize;
    assert_eq!(document["accessors"][accessor]["count"], 5);
    let indices = primitive["indices"].as_u64().unwrap() as usize;
    assert_eq!(document["accessors"][indices]["count"], 9);
    assert_eq!(document["accessors"][indices]["componentType"], 5123);
    assert!(primitive["attributes"].get("TEXCOORD_0").is_some());
    assert_eq!(
        document["materials"][primitive["material"].as_u64().unwrap() as usize]["doubleSided"],
        true
    );
    assert_eq!(document["nodes"][2]["matrix"][0], 0.01);
    assert_eq!(document["nodes"][2]["matrix"][6], -0.01);
    assert!(output.diagnostics.iter().any(|v| v.code == "gltf.normals_generated"));
    assert_eq!(output.bytes, export_scene(&scene, &options).unwrap().bytes);
}

#[test]
fn rejects_shear_degenerate_polygons_and_expanded_buffer_overflow() {
    let mut s = scene();
    s.nodes.get_mut("part").unwrap().transform[0][1] = 1.;
    assert_eq!(export_scene(&s, &Default::default()).unwrap_err().code, "gltf.shear");
    let mut s = scene();
    s.meshes.get_mut("shared").unwrap().positions[1] = [0., 0., 0.];
    assert_eq!(export_scene(&s, &Default::default()).unwrap_err().code, "mesh.polygon");
    let options = SceneGltfOptions {
        limits: SceneLimits {
            max_bytes: 300,
            ..Default::default()
        },
        ..Default::default()
    };
    assert_eq!(export_scene(&scene(), &options).unwrap_err().code, "scene.limit");
}

#[test]
fn compressed_images_cannot_bypass_the_decoded_pixel_budget() {
    use sha2::Digest;
    use usd_toolbox_core::{Channel, ColorSpace, MapRole, Sha256, TextureRef, TextureSource, Tier};
    let mut bytes = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(32, 32, image::Rgba([255, 0, 0, 255])))
        .write_to(&mut bytes, image::ImageFormat::Png)
        .unwrap();
    let bytes = bytes.into_inner();
    let mut s = scene();
    s.materials[0].surface.base_color = usd_toolbox_core::Value::Texture {
        texture: TextureRef {
            role: MapRole::BaseColor,
            color_space: ColorSpace::Srgb,
            channel: Channel::Rgb,
            normal_convention: None,
            tiers: std::collections::BTreeMap::from([(
                Tier::Preview,
                TextureSource {
                    hash: Sha256(format!("{:x}", sha2::Sha256::digest(&bytes))),
                    extension: "png".into(),
                    media_type: "image/png".into(),
                    width: Some(32),
                    height: Some(32),
                    bytes,
                },
            )]),
        },
    };
    let options = SceneGltfOptions {
        limits: SceneLimits {
            max_bytes: 1024,
            ..Default::default()
        },
        ..Default::default()
    };
    let error = export_scene(&s, &options).unwrap_err();
    assert_eq!(error.code, "scene.limit");
    assert!(error.detail.contains("decoded texture"));
}

fn grid(size: usize) -> Scene {
    let mut s = Scene::new("grid");
    let mut mesh = Mesh::default();
    for y in 0..=size {
        for x in 0..=size {
            mesh.positions.push([x as f64, y as f64, 0.]);
        }
    }
    for y in 0..size {
        for x in 0..size {
            let v = (y * (size + 1) + x) as u32;
            mesh.face_counts.push(4);
            mesh.face_indices
                .extend([v, v + 1, v + (size + 2) as u32, v + (size + 1) as u32]);
        }
    }
    s.meshes.insert("grid".into(), mesh);
    let mut node = SceneNode::new("Grid");
    node.mesh = Some("grid".into());
    s.nodes.insert("grid".into(), node);
    s
}
#[test]
fn indexed_output_uses_32_bits_when_reserved_short_index_would_be_needed() {
    let s = grid(255); // 65,536 unique vertices; 65,535 is forbidden as a u16 index.
    let out = export_scene(&s, &Default::default()).unwrap();
    let length = u32::from_le_bytes(out.bytes[12..16].try_into().unwrap()) as usize;
    let document: Value = serde_json::from_slice(&out.bytes[20..20 + length]).unwrap();
    let primitive = &document["meshes"][0]["primitives"][0];
    let indices = primitive["indices"].as_u64().unwrap() as usize;
    assert_eq!(document["accessors"][indices]["componentType"], 5125);
    let positions = primitive["attributes"]["POSITION"].as_u64().unwrap() as usize;
    assert_eq!(document["accessors"][positions]["count"], 65536);
}
#[test]
#[ignore = "repeatable release-build export performance measurement"]
fn indexed_grid_benchmark() {
    let s = grid(256);
    let start = std::time::Instant::now();
    let out = export_scene(&s, &Default::default()).unwrap();
    let elapsed = start.elapsed();
    let expanded_bytes = 256 * 256 * 6 * 6 * 4; // Prior exporter: six unindexed corners, position + normal.
    assert!(out.bytes.len() < expanded_bytes / 2);
    eprintln!(
        "grid=256x256 points=66049 quads=65536 indexed_glb_bytes={} old_expanded_geometry_bytes={} reduction_percent={:.2} export_ms={:.3}",
        out.bytes.len(),
        expanded_bytes,
        100. * (1. - out.bytes.len() as f64 / expanded_bytes as f64),
        elapsed.as_secs_f64() * 1000.
    );
}
