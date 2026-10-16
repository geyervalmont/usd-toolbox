//! Synthetic, repeatable geometry stage timings with honest LOD error semantics.
use std::time::Instant;
use usd_toolbox_core::scene::*;
fn main() {
    let n = std::env::args()
        .nth(1)
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(32);
    assert!((2..=256).contains(&n));
    let mut mesh = Mesh::default();
    for y in 0..=n {
        for x in 0..=n {
            mesh.positions.push([x as f64, y as f64, 0.]);
        }
    }
    for y in 0..n {
        for x in 0..n {
            let a = (y * (n + 1) + x) as u32;
            let b = a + 1;
            let c = a + (n + 1) as u32;
            let d = c + 1;
            mesh.face_counts.extend([3, 3]);
            mesh.face_indices.extend([a, b, d, a, d, c]);
        }
    }
    let mut scene = Scene::new("geometry-benchmark");
    scene.meshes.insert("grid".into(), mesh);
    let mut node = SceneNode::new("Grid");
    node.mesh = Some("grid".into());
    scene.nodes.insert("grid".into(), node);
    let mut rows = Vec::new();
    for edit in [
        SceneEdit::GenerateNormals {
            mesh: "grid".into(),
            crease_angle_degrees: 60.,
            area_weighted: true,
        },
        SceneEdit::SimplifyMesh {
            mesh: "grid".into(),
            target_faces: n * n,
            max_vertex_displacement: 2.,
        },
        SceneEdit::WeldVertices {
            mesh: "grid".into(),
            tolerance: 0.001,
        },
    ] {
        let started = Instant::now();
        let changed = edit_scene(&scene, &[edit], &Default::default()).unwrap();
        let milliseconds = started.elapsed().as_secs_f64() * 1000.;
        let report: GeometryReport =
            serde_json::from_str(&changed.provenance.operations.last().unwrap().detail).unwrap();
        rows.push(serde_json::json!({"milliseconds":milliseconds,"report":report}));
    }
    println!("{}",serde_json::to_string_pretty(&serde_json::json!({"schema":"usd-toolbox.geometry-benchmark.v1","profile":if cfg!(debug_assertions){"debug"}else{"release"},"platform":std::env::consts::OS,"arch":std::env::consts::ARCH,"grid_size":n,"source_faces":2*n*n,"stages":rows,"notes":["Synthetic planar grid; timings include atomic copy-returning edit_scene validation and payload copy.","LOD maximum original-vertex displacement is not a surface Hausdorff bound; open edges are locked."]})).unwrap());
}
