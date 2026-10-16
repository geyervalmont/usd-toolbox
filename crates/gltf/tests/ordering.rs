use serde_json::{Value, json};
use sha2::Digest;
use std::collections::BTreeMap;
use usd_toolbox_core::{Material, MaterialId, scene::*};
use usd_toolbox_gltf::scene::*;

fn grid(size: usize, shuffled: bool, attributes: bool) -> Scene {
    let mut scene = Scene::new("cache-grid");
    let mut mesh = Mesh::default();
    for y in 0..=size {
        for x in 0..=size {
            mesh.positions.push([x as f64, y as f64, 0.]);
        }
    }
    let mut normals = Vec::new();
    let mut uv = Vec::new();
    let mut uv2 = Vec::new();
    let mut colors = Vec::new();
    let mut tangents = Vec::new();
    for i in 0..size * size {
        let cell = if shuffled { i * 73 % (size * size) } else { i };
        let (x, y) = (cell % size, cell / size);
        let v = (y * (size + 1) + x) as u32;
        mesh.face_counts.push(4);
        mesh.face_indices
            .extend([v, v + 1, v + (size + 2) as u32, v + (size + 1) as u32]);
        if attributes {
            let right = x >= size / 2;
            for [u, v] in [[x, y], [x + 1, y], [x + 1, y + 1], [x, y + 1]] {
                uv.push([u as f32 / size as f32 + f32::from(right), v as f32 / size as f32]);
                uv2.push([v as f32 / size as f32, u as f32 / size as f32]);
                normals.push(if right { [0., 0.6, 0.8] } else { [0., 0., 1.] });
                colors.push([u as f32 / size as f32, 0.5, v as f32 / size as f32, 1.]);
                tangents.push([1., 0., 0., if right { -1. } else { 1. }]);
            }
        }
    }
    if attributes {
        mesh.normals = Some(Attribute {
            interpolation: Interpolation::FaceVarying,
            values: normals,
            indices: None,
        });
        mesh.tangents = Some(Attribute {
            interpolation: Interpolation::FaceVarying,
            values: tangents,
            indices: None,
        });
        mesh.colors = Some(Attribute {
            interpolation: Interpolation::FaceVarying,
            values: colors,
            indices: None,
        });
        mesh.uv_sets.insert(
            "st".into(),
            Attribute {
                interpolation: Interpolation::FaceVarying,
                values: uv,
                indices: None,
            },
        );
        mesh.uv_sets.insert(
            "st1".into(),
            Attribute {
                interpolation: Interpolation::FaceVarying,
                values: uv2,
                indices: None,
            },
        );
    }
    scene.meshes.insert("grid".into(), mesh);
    let mut node = SceneNode::new("Grid");
    node.mesh = Some("grid".into());
    node.material = Some(MaterialId::new("solid"));
    scene.nodes.insert("grid".into(), node);
    scene.materials.push(Material::new("solid", "Solid"));
    scene
}

fn export(scene: &Scene, optimize: bool) -> SceneExport {
    export_scene(
        scene,
        &SceneGltfOptions {
            optimize_vertex_cache: optimize,
            ..Default::default()
        },
    )
    .unwrap()
}
fn document(bytes: &[u8]) -> (Value, &[u8]) {
    let length = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
    (
        serde_json::from_slice(&bytes[20..20 + length]).unwrap(),
        &bytes[28 + length..],
    )
}
fn accessor<'a>(doc: &Value, binary: &'a [u8], index: usize) -> (&'a [u8], usize) {
    let acc = &doc["accessors"][index];
    let view = &doc["bufferViews"][acc["bufferView"].as_u64().unwrap() as usize];
    let components = match acc["type"].as_str().unwrap() {
        "SCALAR" => 1,
        "VEC2" => 2,
        "VEC3" => 3,
        "VEC4" => 4,
        _ => panic!("unsupported test accessor"),
    };
    let width = match acc["componentType"].as_u64().unwrap() {
        5123 => 2,
        5125 | 5126 => 4,
        _ => panic!("unsupported test component"),
    };
    let start = view["byteOffset"].as_u64().unwrap_or(0) as usize + acc["byteOffset"].as_u64().unwrap_or(0) as usize;
    let stride = components * width;
    let length = acc["count"].as_u64().unwrap() as usize * stride;
    (&binary[start..start + length], stride)
}
fn indices(doc: &Value, binary: &[u8], primitive: &Value) -> Vec<usize> {
    let (bytes, stride) = accessor(doc, binary, primitive["indices"].as_u64().unwrap() as usize);
    bytes
        .chunks_exact(stride)
        .map(|b| {
            if stride == 2 {
                u16::from_le_bytes(b.try_into().unwrap()) as usize
            } else {
                u32::from_le_bytes(b.try_into().unwrap()) as usize
            }
        })
        .collect()
}
fn primitives(bytes: &[u8]) -> BTreeMap<String, Vec<Vec<u8>>> {
    let (doc, binary) = document(bytes);
    let mut result: BTreeMap<String, Vec<Vec<u8>>> = BTreeMap::new();
    for mesh in doc["meshes"].as_array().unwrap() {
        for primitive in mesh["primitives"].as_array().unwrap() {
            let attributes = primitive["attributes"]
                .as_object()
                .unwrap()
                .iter()
                .map(|(name, index)| (name, accessor(&doc, binary, index.as_u64().unwrap() as usize)))
                .collect::<Vec<_>>();
            let records = indices(&doc, binary, primitive)
                .as_chunks::<3>()
                .0
                .iter()
                .map(|tri| {
                    let mut record = Vec::new();
                    for &vertex in tri {
                        for (_, (bytes, stride)) in &attributes {
                            record.extend_from_slice(&bytes[vertex * stride..(vertex + 1) * stride]);
                        }
                    }
                    record
                })
                .collect::<Vec<_>>();
            let material = primitive["material"].as_u64().unwrap() as usize;
            result
                .entry(format!(
                    "{material}:{}",
                    doc["materials"][material]["name"].as_str().unwrap()
                ))
                .or_default()
                .extend(records);
        }
    }
    result
}
fn misses(indices: &[usize], capacity: usize) -> usize {
    let mut cache = std::collections::VecDeque::new();
    let mut misses = 0;
    for &index in indices {
        if !cache.contains(&index) {
            misses += 1;
            cache.push_back(index);
            if cache.len() > capacity {
                cache.pop_front();
            }
        }
    }
    misses
}
fn statistics(bytes: &[u8]) -> (usize, usize, usize) {
    let (doc, binary) = document(bytes);
    let mut n = 0;
    let mut cache16 = 0;
    let mut cache32 = 0;
    for mesh in doc["meshes"].as_array().unwrap() {
        for primitive in mesh["primitives"].as_array().unwrap() {
            let indices = indices(&doc, binary, primitive);
            n += indices.len() / 3;
            cache16 += misses(&indices, 16);
            cache32 += misses(&indices, 32);
        }
    }
    (n, cache16, cache32)
}

#[test]
fn opaque_ordering_reduces_cache_misses_and_preserves_all_attribute_tuples_and_winding() {
    let scene = grid(32, true, true);
    let baseline = export(&scene, false);
    let optimized = export(&scene, true);
    assert_eq!(optimized.bytes, export(&scene, true).bytes);
    assert_eq!(export(&scene, false).bytes, baseline.bytes);
    assert!(!SceneGltfOptions::default().optimize_vertex_cache);
    let mut original = primitives(&baseline.bytes);
    let mut reordered = primitives(&optimized.bytes);
    assert_ne!(original, reordered, "fixture must exercise a real reorder");
    for triangles in original.values_mut() {
        triangles.sort();
    }
    for triangles in reordered.values_mut() {
        triangles.sort();
    }
    assert_eq!(
        original, reordered,
        "every corner's position, normal, both UVs, colour and tangent must survive bit-for-bit"
    );
    let before = statistics(&baseline.bytes);
    let after = statistics(&optimized.bytes);
    assert_eq!(before.0, after.0);
    assert!(after.1 < before.1 / 2);
    assert!(after.2 < before.2 / 2);
    assert_eq!(baseline.bytes.len(), optimized.bytes.len());
    assert!(
        optimized
            .diagnostics
            .iter()
            .any(|d| d.code == "gltf.vertex_cache_optimized")
    );
    let (doc, binary) = document(&optimized.bytes);
    let primitive = &doc["meshes"][0]["primitives"][0];
    let mut seen = std::collections::BTreeSet::new();
    let mut next = 0;
    for index in indices(&doc, binary, primitive) {
        if seen.insert(index) {
            assert_eq!(index, next);
            next += 1;
        }
    }
    assert!(next > (32 + 1) * (32 + 1), "render seam vertices stay split");
}

#[test]
fn mask_blend_opacity_and_transmission_preserve_triangle_and_vertex_order() {
    for profile in ["MASK", "BLEND", "opacity", "transmission"] {
        let mut scene = grid(16, true, true);
        if matches!(profile, "MASK" | "BLEND") {
            scene
                .metadata
                .materials
                .entry("solid".into())
                .or_default()
                .original
                .insert(
                    "gltf:render_settings".into(),
                    MetadataProperty::authored(MetadataValue::from_json(
                        json!({"alpha_mode":profile,"alpha_cutoff":0.5,"textures":{}}),
                    )),
                );
        } else if profile == "opacity" {
            scene.materials[0].geometry.opacity = Some(0.5.into());
        } else {
            scene.materials[0].surface.transmission_weight = Some(0.7.into());
        }
        let a = export(&scene, false);
        let b = export(&scene, true);
        assert_eq!(a.bytes, b.bytes, "{profile}");
        assert!(!b.diagnostics.iter().any(|d| d.code == "gltf.vertex_cache_optimized"));
    }
}

#[test]
fn insufficient_optional_scratch_budget_keeps_valid_original_order() {
    let scene = grid(32, true, false);
    let options = SceneGltfOptions {
        optimize_vertex_cache: true,
        limits: SceneLimits {
            max_bytes: 150_000,
            ..Default::default()
        },
        ..Default::default()
    };
    let optimized = export_scene(&scene, &options).unwrap();
    let baseline = export_scene(
        &scene,
        &SceneGltfOptions {
            optimize_vertex_cache: false,
            ..options
        },
    )
    .unwrap();
    assert_eq!(baseline.bytes, optimized.bytes);
    assert!(
        optimized
            .diagnostics
            .iter()
            .any(|d| d.code == "gltf.vertex_cache_skipped")
    );
}

#[test]
fn candidate_without_measurable_cache_benefit_preserves_original_bytes() {
    let scene = grid(1, false, false);
    let original = export(&scene, false);
    let attempted = export(&scene, true);
    assert_eq!(original.bytes, attempted.bytes);
    assert!(
        !attempted
            .diagnostics
            .iter()
            .any(|d| d.code == "gltf.vertex_cache_optimized")
    );
}

#[test]
fn mixed_material_primitive_boundaries_and_transparent_order_survive() {
    let mut scene = grid(32, true, true);
    let mut transparent = Material::new("blend", "Blend");
    transparent.geometry.opacity = Some(0.5.into());
    scene.materials.push(transparent);
    scene.nodes.get_mut("grid").unwrap().subsets.push(FaceBinding {
        name: "Blend faces".into(),
        faces: (0..1024).filter(|f| f % 2 == 1).collect(),
        material: MaterialId::new("blend"),
    });
    let original = export(&scene, false);
    let optimized = export(&scene, true);
    let mut a = primitives(&original.bytes);
    let mut b = primitives(&optimized.bytes);
    assert_eq!(a["1:Blend"], b["1:Blend"]);
    for triangles in a.values_mut() {
        triangles.sort();
    }
    for triangles in b.values_mut() {
        triangles.sort();
    }
    assert_eq!(a, b);
}

#[test]
#[ignore = "repeatable release-build vertex-cache ordering measurement"]
fn cache_ordering_benchmark() {
    let mut all_reports = Vec::new();
    for shuffled in [false, true] {
        let scene = grid(256, shuffled, false);
        let mut report = Vec::new();
        for optimize in [false, true] {
            let (output, samples) = measure(&scene, optimize);
            let (triangles, fifo16, fifo32) = statistics(&output.bytes);
            report.push(json!({"shuffled":shuffled,"optimized":optimize,"triangles":triangles,"fifo16_misses":fifo16,"fifo32_misses":fifo32,"fifo16_acmr":fifo16 as f64/triangles as f64,"fifo32_acmr":fifo32 as f64/triangles as f64,"output_bytes":output.bytes.len(),"export_ms_p50":samples[2],"export_ms_samples":samples}));
        }
        eprintln!("{}", serde_json::to_string_pretty(&report).unwrap());
        all_reports.extend(report);
    }
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    std::fs::write(
        root.join("target/gltf-ordering-grid.json"),
        serde_json::to_vec_pretty(&all_reports).unwrap(),
    )
    .unwrap();
}

fn measure(scene: &Scene, optimize: bool) -> (SceneExport, Vec<f64>) {
    let mut output = export(scene, optimize);
    let mut samples = Vec::new();
    for _ in 0..5 {
        let started = std::time::Instant::now();
        output = export(scene, optimize);
        samples.push(started.elapsed().as_secs_f64() * 1000.);
    }
    samples.sort_by(f64::total_cmp);
    (output, samples)
}

#[test]
#[ignore = "requires pinned external corpus under target/corpus"]
fn cache_ordering_on_pinned_corpus() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let destination = root.join("target/corpus-optimized");
    std::fs::create_dir_all(&destination).unwrap();
    let mut report = Vec::new();
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(root.join("fixtures/corpus/manifest.json")).unwrap()).unwrap();
    for id in ["Box", "BoomBox", "NormalTangentMirrorTest"] {
        let path = format!("{id}/{id}.glb");
        let bytes = std::fs::read(root.join("target/corpus").join(&path)).unwrap();
        let declared = &manifest["assets"]
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["id"] == id)
            .unwrap()["files"][0]["sha256"];
        assert_eq!(format!("{:x}", sha2::Sha256::digest(&bytes)), *declared);
        let scene = import_scene(&bytes, &path, &Default::default()).unwrap().scene;
        let (baseline, baseline_samples) = measure(&scene, false);
        let (optimized, optimized_samples) = measure(&scene, true);
        let mut a = primitives(&baseline.bytes);
        let mut b = primitives(&optimized.bytes);
        for triangles in a.values_mut() {
            triangles.sort();
        }
        for triangles in b.values_mut() {
            triangles.sort();
        }
        assert_eq!(a, b, "{id}: geometry changed");
        let before = statistics(&baseline.bytes);
        let after = statistics(&optimized.bytes);
        assert!(after.1 <= before.1 && after.2 <= before.2);
        std::fs::write(destination.join(format!("{id}.glb")), &optimized.bytes).unwrap();
        report.push(json!({"id":id,"triangles":before.0,"before_fifo16":before.1,"after_fifo16":after.1,"before_fifo32":before.2,"after_fifo32":after.2,"baseline_bytes":baseline.bytes.len(),"optimized_bytes":optimized.bytes.len(),"baseline_export_ms_p50":baseline_samples[2],"optimized_export_ms_p50":optimized_samples[2],"baseline_export_ms_samples":baseline_samples,"optimized_export_ms_samples":optimized_samples,"diagnostics":optimized.diagnostics.iter().filter(|d|d.code.starts_with("gltf.vertex_cache")).collect::<Vec<_>>()}));
    }
    std::fs::write(
        destination.join("ordering-report.json"),
        serde_json::to_vec_pretty(&report).unwrap(),
    )
    .unwrap();
    eprintln!("{}", serde_json::to_string_pretty(&report).unwrap());
}
