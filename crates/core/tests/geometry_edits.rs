use usd_toolbox_core::scene::*;
use usd_toolbox_core::{Material, MaterialId};
fn scene(mesh: Mesh) -> Scene {
    let mut s = Scene::new("geometry");
    s.meshes.insert("mesh".into(), mesh);
    let mut node = SceneNode::new("Node");
    node.mesh = Some("mesh".into());
    s.nodes.insert("node".into(), node);
    s
}
fn triangle() -> Mesh {
    Mesh {
        positions: vec![[0., 0., 0.], [1., 0., 0.], [0., 1., 0.]],
        face_counts: vec![3],
        face_indices: vec![0, 1, 2],
        normals: Some(Attribute {
            interpolation: Interpolation::Vertex,
            values: vec![[0., 0., 1.]; 3],
            indices: None,
        }),
        tangents: Some(Attribute {
            interpolation: Interpolation::Vertex,
            values: vec![[1., 0., 0., 1.]; 3],
            indices: None,
        }),
        ..Default::default()
    }
}
#[test]
fn affine_baking_preserves_world_points_shared_meshes_children_and_mirror_basis() {
    let mut s = scene(triangle());
    let mut transform = IDENTITY;
    transform[0] = [-2., 0.5, 0., 5.];
    transform[1][1] = 3.;
    s.nodes.get_mut("node").unwrap().transform = transform;
    let mut shared = s.nodes["node"].clone();
    shared.transform = IDENTITY;
    s.nodes.insert("shared".into(), shared);
    let mut child = SceneNode::new("Child");
    child.parent = Some("node".into());
    child.transform[2][3] = 2.;
    s.nodes.insert("child".into(), child);
    let before = scene_bounds(&s).unwrap();
    let child_world = world_transform(&s, "child").unwrap();
    let baked = edit_scene(
        &s,
        &[SceneEdit::BakeTransform { node: "node".into() }],
        &Default::default(),
    )
    .unwrap();
    assert_eq!(scene_bounds(&baked).unwrap(), before);
    assert_eq!(world_transform(&baked, "child").unwrap(), child_world);
    assert_eq!(baked.nodes["node"].transform, IDENTITY);
    assert_eq!(baked.meshes.len(), 2);
    assert_eq!(baked.meshes["mesh"], s.meshes["mesh"]);
    let mesh = &baked.meshes[baked.nodes["node"].mesh.as_ref().unwrap()];
    assert_eq!(mesh.orientation, Orientation::LeftHanded);
    assert_eq!(mesh.tangents.as_ref().unwrap().values[0], [-1., 0., 0., -1.]);
    assert_eq!(mesh.normals.as_ref().unwrap().values[0], [0., 0., 1.]);
}
#[test]
fn singular_baking_rolls_back_and_noop_callbacks_run() {
    let mut s = scene(triangle());
    s.nodes.get_mut("node").unwrap().transform[0][0] = 0.;
    let mut doc = SceneDocument::new(s, Default::default()).unwrap();
    let before = doc.scene().clone();
    assert_eq!(
        doc.apply(&[SceneEdit::BakeTransform { node: "node".into() }])
            .unwrap_err()
            .code,
        "geometry.singular"
    );
    assert_eq!(doc.scene(), &before);
    let mut called = false;
    doc.apply_checked(&[], |report| {
        called = true;
        assert_eq!(report.generation, 0);
        Ok(())
    })
    .unwrap();
    assert!(called);
}
#[test]
fn normal_smoothing_obeys_angles_and_material_boundaries() {
    let mesh = Mesh {
        positions: vec![[0., 0., 0.], [1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
        face_counts: vec![3, 3],
        face_indices: vec![0, 1, 2, 0, 3, 1],
        ..Default::default()
    };
    let base = scene(mesh);
    let smooth = edit_scene(
        &base,
        &[SceneEdit::GenerateNormals {
            mesh: "mesh".into(),
            crease_angle_degrees: 100.,
            area_weighted: true,
        }],
        &Default::default(),
    )
    .unwrap();
    let normal = smooth.meshes["mesh"].normals.as_ref().unwrap().values[0];
    assert!((normal[1] - std::f64::consts::FRAC_1_SQRT_2).abs() < 1e-12);
    assert!((normal[2] - normal[1]).abs() < 1e-12);
    let hard = edit_scene(
        &base,
        &[SceneEdit::GenerateNormals {
            mesh: "mesh".into(),
            crease_angle_degrees: 30.,
            area_weighted: true,
        }],
        &Default::default(),
    )
    .unwrap();
    assert_eq!(hard.meshes["mesh"].normals.as_ref().unwrap().values[0], [0., 0., 1.]);
    let mut boundary = base;
    boundary.materials = vec![Material::new("a", "A"), Material::new("b", "B")];
    boundary.nodes.get_mut("node").unwrap().material = Some(MaterialId::new("a"));
    boundary.nodes.get_mut("node").unwrap().subsets = vec![FaceBinding {
        name: "other".into(),
        faces: vec![1],
        material: MaterialId::new("b"),
    }];
    let protected = edit_scene(
        &boundary,
        &[SceneEdit::GenerateNormals {
            mesh: "mesh".into(),
            crease_angle_degrees: 180.,
            area_weighted: true,
        }],
        &Default::default(),
    )
    .unwrap();
    assert_eq!(
        protected.meshes["mesh"].normals.as_ref().unwrap().values[0],
        [0., 0., 1.]
    );
}
#[test]
fn explicit_degenerate_cleanup_remaps_corner_attributes_and_bindings() {
    let mut mesh = triangle();
    mesh.face_counts.push(3);
    mesh.face_indices.extend([0, 0, 1]);
    mesh.uv_sets.insert(
        "st".into(),
        Attribute {
            interpolation: Interpolation::FaceVarying,
            values: vec![[0., 0.], [1., 0.], [0., 1.], [9., 9.], [8., 8.], [7., 7.]],
            indices: None,
        },
    );
    let mut s = scene(mesh);
    s.materials.push(Material::new("mat", "Material"));
    s.nodes.get_mut("node").unwrap().subsets = vec![FaceBinding {
        name: "removed".into(),
        faces: vec![1],
        material: MaterialId::new("mat"),
    }];
    let repaired = edit_scene(
        &s,
        &[SceneEdit::RemoveDegenerateFaces {
            mesh: "mesh".into(),
            area_tolerance: 0.,
        }],
        &Default::default(),
    )
    .unwrap();
    assert_eq!(repaired.meshes["mesh"].face_counts, [3]);
    assert_eq!(
        repaired.meshes["mesh"].uv_sets["st"].values,
        [[0., 0.], [1., 0.], [0., 1.]]
    );
    assert!(repaired.nodes["node"].subsets.is_empty());
}
#[test]
fn standalone_repair_requires_permission_for_invalid_indices_and_rejects_bowties() {
    let mut mesh = triangle();
    mesh.face_counts.push(3);
    mesh.face_indices.extend([0, 1, 99]);
    assert_eq!(repair_mesh(&mesh, &Default::default()).unwrap_err().code, "mesh.index");
    let repaired = repair_mesh(
        &mesh,
        &MeshRepairOptions {
            remove_invalid_index_faces: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(repaired.removed_faces, [1]);
    assert_eq!(repaired.old_to_new_faces, [Some(0), None]);
    let bowtie = Mesh {
        positions: vec![[0., 0., 0.], [1., 1., 0.], [0., 1., 0.], [1., 0., 0.]],
        face_counts: vec![4],
        face_indices: vec![0, 1, 2, 3],
        ..Default::default()
    };
    assert!(repair_mesh(&bowtie, &Default::default()).is_err());
}
#[test]
fn welding_preserves_vertex_attribute_seams_and_corner_uvs() {
    let mut mesh = Mesh {
        positions: vec![[0., 0., 0.], [1., 0., 0.], [0., 1., 0.], [0., 0., 0.]],
        face_counts: vec![3, 3],
        face_indices: vec![0, 1, 2, 3, 1, 2],
        ..Default::default()
    };
    mesh.uv_sets.insert(
        "st".into(),
        Attribute {
            interpolation: Interpolation::FaceVarying,
            values: vec![[0., 0.], [1., 0.], [0., 1.], [0.5, 0.5], [1., 0.], [0., 1.]],
            indices: None,
        },
    );
    let s = scene(mesh);
    let welded = edit_scene(
        &s,
        &[SceneEdit::WeldVertices {
            mesh: "mesh".into(),
            tolerance: 0.001,
        }],
        &Default::default(),
    )
    .unwrap();
    assert_eq!(welded.meshes["mesh"].positions.len(), 3);
    assert_eq!(welded.meshes["mesh"].uv_sets, s.meshes["mesh"].uv_sets);
    let mut s = s;
    s.meshes.get_mut("mesh").unwrap().uv_sets.insert(
        "st".into(),
        Attribute {
            interpolation: Interpolation::Vertex,
            values: vec![[0., 0.], [1., 0.], [0., 1.], [0.5, 0.5]],
            indices: None,
        },
    );
    let seams = edit_scene(
        &s,
        &[SceneEdit::WeldVertices {
            mesh: "mesh".into(),
            tolerance: 0.001,
        }],
        &Default::default(),
    )
    .unwrap();
    assert_eq!(seams.meshes["mesh"].positions.len(), 4);
}
fn grid(n: usize) -> Mesh {
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
    mesh
}
#[test]
fn conservative_lod_reduces_interior_grid_and_bounds_original_vertex_displacement() {
    let base = scene(grid(8));
    let edits = [SceneEdit::SimplifyMesh {
        mesh: "mesh".into(),
        target_faces: 80,
        max_vertex_displacement: 2.,
    }];
    let simplified = edit_scene(&base, &edits, &Default::default()).unwrap();
    assert!(simplified.meshes["mesh"].face_counts.len() < 128);
    assert!(simplified.meshes["mesh"].positions.len() < 81);
    assert_eq!(scene_bounds(&simplified).unwrap(), scene_bounds(&base).unwrap());
    for p in &base.meshes["mesh"].positions {
        if p[0] == 0. || p[1] == 0. || p[0] == 8. || p[1] == 8. {
            assert!(simplified.meshes["mesh"].positions.contains(p));
        }
    }
    let report: GeometryReport =
        serde_json::from_str(&simplified.provenance.operations.last().unwrap().detail).unwrap();
    assert!(report.max_vertex_displacement <= 2.);
    assert_eq!(simplified, edit_scene(&base, &edits, &Default::default()).unwrap());
}
#[test]
fn zero_error_lod_preserves_uv_seams_and_triangulation_preserves_bindings() {
    let mut mesh = grid(1);
    mesh.uv_sets.insert(
        "st".into(),
        Attribute {
            interpolation: Interpolation::FaceVarying,
            values: vec![[0., 0.], [1., 0.], [1., 1.], [0.5, 0.5], [0.8, 0.8], [0., 1.]],
            indices: None,
        },
    );
    let s = scene(mesh);
    let lod = edit_scene(
        &s,
        &[SceneEdit::SimplifyMesh {
            mesh: "mesh".into(),
            target_faces: 1,
            max_vertex_displacement: 0.,
        }],
        &Default::default(),
    )
    .unwrap();
    assert_eq!(lod.meshes["mesh"].uv_sets, s.meshes["mesh"].uv_sets);
    let mut quad = scene(Mesh {
        positions: vec![[0., 0., 0.], [1., 0., 0.], [1., 1., 0.], [0., 1., 0.]],
        face_counts: vec![4],
        face_indices: vec![0, 1, 2, 3],
        ..Default::default()
    });
    quad.materials.push(Material::new("mat", "Material"));
    quad.nodes.get_mut("node").unwrap().subsets = vec![FaceBinding {
        name: "all".into(),
        faces: vec![0],
        material: MaterialId::new("mat"),
    }];
    let triangulated = edit_scene(
        &quad,
        &[SceneEdit::TriangulateMesh { mesh: "mesh".into() }],
        &Default::default(),
    )
    .unwrap();
    assert_eq!(triangulated.meshes["mesh"].face_counts, [3, 3]);
    assert_eq!(triangulated.nodes["node"].subsets[0].faces, [0, 1]);
}
