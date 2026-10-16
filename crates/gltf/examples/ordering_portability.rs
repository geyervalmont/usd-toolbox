//! Standalone native/bare-WASM probe exercising an accepted cache reorder.
use sha2::Digest;
use usd_toolbox_core::{Material, MaterialId, scene::*};
use usd_toolbox_gltf::scene::*;

#[unsafe(no_mangle)]
pub extern "C" fn ordering_checksum() -> u64 {
    let mut scene = Scene::new("portable-ordering");
    let mut mesh = Mesh::default();
    for y in 0..=16 {
        for x in 0..=16 {
            mesh.positions.push([f64::from(x), f64::from(y), 0.]);
        }
    }
    let uv = mesh
        .positions
        .iter()
        .map(|p| [p[0] as f32 / 16., p[1] as f32 / 16.])
        .collect();
    let colors = mesh
        .positions
        .iter()
        .map(|p| [p[0] as f32 / 16., 0.5, p[1] as f32 / 16., 1.])
        .collect();
    let tangents = mesh
        .positions
        .iter()
        .map(|p| [1., 0., 0., if p[0] < 8. { 1. } else { -1. }])
        .collect();
    mesh.uv_sets.insert(
        "st".into(),
        Attribute {
            interpolation: Interpolation::Vertex,
            values: uv,
            indices: None,
        },
    );
    mesh.colors = Some(Attribute {
        interpolation: Interpolation::Vertex,
        values: colors,
        indices: None,
    });
    mesh.tangents = Some(Attribute {
        interpolation: Interpolation::Vertex,
        values: tangents,
        indices: None,
    });
    for i in 0..256u32 {
        let cell = i * 73 % 256;
        let v = cell / 16 * 17 + cell % 16;
        mesh.face_counts.push(4);
        mesh.face_indices.extend([v, v + 1, v + 18, v + 17]);
    }
    scene.meshes.insert("grid".into(), mesh);
    scene.materials.push(Material::new("solid", "Solid"));
    let mut node = SceneNode::new("Grid");
    node.mesh = Some("grid".into());
    node.material = Some(MaterialId::new("solid"));
    scene.nodes.insert("grid".into(), node);
    let original = export_scene(&scene, &SceneGltfOptions::default()).unwrap();
    let optimized = export_scene(
        &scene,
        &SceneGltfOptions {
            optimize_vertex_cache: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(original.bytes.len(), optimized.bytes.len());
    assert_ne!(original.bytes, optimized.bytes);
    assert!(
        optimized
            .diagnostics
            .iter()
            .any(|d| d.code == "gltf.vertex_cache_optimized")
    );
    u64::from_le_bytes(sha2::Sha256::digest(&optimized.bytes)[..8].try_into().unwrap())
}
fn main() {
    println!("{}", ordering_checksum());
}
