use usd_toolbox_core::scene::*;
use usd_toolbox_core::{Material, MaterialId};

fn scene() -> Scene {
    let mut s = Scene::new("test");
    s.meshes.insert(
        "mesh".into(),
        Mesh {
            positions: vec![[0., 0., 0.], [1., 0., 0.], [0., 1., 0.]],
            face_counts: vec![3],
            face_indices: vec![0, 1, 2],
            ..Default::default()
        },
    );
    let mut n = SceneNode::new("node");
    n.mesh = Some("mesh".into());
    s.nodes.insert("node".into(), n);
    s
}

#[test]
fn validates_cycles_topology_attributes_bindings_and_budgets() {
    let base = scene();
    assert!(validate_scene(&base, &Default::default()).is_empty());
    let mut s = base.clone();
    s.nodes.get_mut("node").unwrap().parent = Some("node".into());
    assert_eq!(
        require_valid_scene(&s, &Default::default()).unwrap_err().code,
        "scene.cycle"
    );
    let mut s = base.clone();
    s.meshes.get_mut("mesh").unwrap().face_indices[0] = 99;
    assert!(
        validate_scene(&s, &Default::default())
            .iter()
            .any(|d| d.code == "mesh.index")
    );
    let mut s = base.clone();
    s.meshes.get_mut("mesh").unwrap().uv_sets.insert(
        "st".into(),
        Attribute {
            interpolation: Interpolation::FaceVarying,
            values: vec![[0., 0.]],
            indices: Some(vec![0, 0, 1]),
        },
    );
    assert!(
        validate_scene(&s, &Default::default())
            .iter()
            .any(|d| d.code == "mesh.uv")
    );
    let mut s = base.clone();
    s.materials.push(Material::new("m", "m"));
    s.nodes.get_mut("node").unwrap().subsets = vec![
        FaceBinding {
            name: "a".into(),
            faces: vec![0],
            material: MaterialId::new("m"),
        },
        FaceBinding {
            name: "b".into(),
            faces: vec![0],
            material: MaterialId::new("m"),
        },
    ];
    assert!(
        validate_scene(&s, &Default::default())
            .iter()
            .any(|d| d.code == "binding.overlap")
    );
    assert_eq!(
        require_valid_scene(
            &base,
            &SceneLimits {
                max_points: 2,
                ..Default::default()
            }
        )
        .unwrap_err()
        .code,
        "scene.limit"
    );
}

#[test]
fn triangulates_concave_faces_and_rejects_bowties_and_nonplanarity() {
    let p = [[0., 0., 0.], [2., 0., 0.], [2., 2., 0.], [1., 1., 0.], [0., 2., 0.]];
    let triangles = triangulate(&p).unwrap();
    assert_eq!(triangles.len(), 3);
    let area: f64 = triangles
        .iter()
        .map(|t| {
            let [a, b, c] = t.map(|i| p[i]);
            ((b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])) / 2.
        })
        .sum();
    assert_eq!(area, 3.);
    assert!(triangulate(&[[0., 0., 0.], [1., 1., 0.], [0., 1., 0.], [1., 0., 0.]]).is_err());
    assert!(triangulate(&[[0., 0., 0.], [1., 0., 0.], [1., 1., 1.], [0., 1., 0.]]).is_err());
}

#[test]
fn inherited_binding_and_purpose_survive_detachment_and_explicit_overrides() {
    let mut s = scene();
    s.materials.push(Material::new("parent-material", "Parent material"));
    let mut parent = SceneNode::new("parent");
    parent.material = Some(MaterialId::new("parent-material"));
    parent.purpose = Purpose::Proxy;
    s.nodes.insert("parent".into(), parent);
    s.nodes.get_mut("node").unwrap().parent = Some("parent".into());
    assert_eq!(effective_visibility(&s, "node").unwrap(), (true, Purpose::Proxy));
    s.nodes.get_mut("node").unwrap().purpose = Purpose::Default;
    assert_eq!(effective_visibility(&s, "node").unwrap(), (true, Purpose::Default));
    let extracted = extract_subtree(&s, "node", &Default::default()).unwrap();
    assert_eq!(
        extracted.nodes["node"].material,
        Some(MaterialId::new("parent-material"))
    );
    assert_eq!(extracted.materials.len(), 1);
}

#[test]
fn reset_transform_and_unit_conversion_preserve_placement() {
    let mut s = scene();
    let mut parent = SceneNode::new("parent");
    parent.transform[0][3] = 10.;
    s.nodes.insert("parent".into(), parent);
    let node = s.nodes.get_mut("node").unwrap();
    node.parent = Some("parent".into());
    node.transform[0][3] = 2.;
    assert_eq!(scene_bounds(&s).unwrap().unwrap().min, [12., 0., 0.]);
    s.nodes.get_mut("node").unwrap().reset_transform = true;
    assert_eq!(scene_bounds(&s).unwrap().unwrap().min, [2., 0., 0.]);
    let changed = edit_scene(
        &s,
        &[SceneEdit::ConvertUnits { meters_per_unit: 0.001 }],
        &Default::default(),
    )
    .unwrap();
    assert_eq!(scene_bounds(&changed).unwrap().unwrap().min, [2000., 0., 0.]);
}
