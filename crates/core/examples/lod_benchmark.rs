//! Release smoke/measurement for independent LOD generation, with no format I/O.
use std::time::Instant;
use usd_toolbox_core::scene::*;
fn main() {
    let n = std::env::args()
        .nth(1)
        .map(|v| v.parse::<usize>().expect("grid side"))
        .unwrap_or(32);
    assert!((2..=256).contains(&n), "grid side must be 2..=256");
    let mut mesh = Mesh::default();
    for y in 0..=n {
        for x in 0..=n {
            mesh.positions.push([x as f64, y as f64, 0.]);
        }
    }
    for y in 0..n {
        for x in 0..n {
            let a = (y * (n + 1) + x) as u32;
            let c = a + (n + 1) as u32;
            mesh.face_counts.extend([3, 3]);
            mesh.face_indices.extend([a, a + 1, c + 1, a, c + 1, c]);
        }
    }
    let mut source = Scene::new("lod-benchmark");
    source.meshes.insert("grid".into(), mesh);
    let mut node = SceneNode::new("Grid");
    node.mesh = Some("grid".into());
    source.nodes.insert("grid-placement".into(), node);
    let options = SceneLodOptions {
        levels: [0.75, 0.5, 0.25]
            .into_iter()
            .enumerate()
            .map(|(i, face_ratio)| SceneLodRequest {
                name: format!("level_{i}"),
                face_ratio,
                max_vertex_displacement: 2.,
            })
            .collect(),
        ..Default::default()
    };
    let start = Instant::now();
    let levels = generate_scene_lods(&source, &options, &SceneLimits::default()).expect("generate levels");
    let generation_ms = start.elapsed().as_secs_f64() * 1000.;
    let manifest = levels.manifest().clone();
    let start = Instant::now();
    for level in levels.into_scenes() {
        drop(level.expect("materialize level"));
    }
    let materialization_ms = start.elapsed().as_secs_f64() * 1000.;
    println!(
        "{}",
        serde_json::json!({ "grid_side": n, "source_faces": n*n*2, "generation_ms": generation_ms, "materialization_ms": materialization_ms, "manifest": manifest })
    );
}
