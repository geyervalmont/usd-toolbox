use usd_toolbox_core::scene::*;
use usd_toolbox_core::{Material, MaterialId};
fn fixture() -> Scene {
    let mut s = Scene::new("editing");
    s.meshes.insert(
        "mesh".into(),
        Mesh {
            positions: vec![[0., 0., 0.], [1., 0., 0.], [0., 1., 0.]],
            face_counts: vec![3],
            face_indices: vec![0, 1, 2],
            ..Default::default()
        },
    );
    s.materials.push(Material::new("mat", "Material"));
    let mut root = SceneNode::new("Root");
    root.transform[0][3] = 10.;
    root.material = Some(MaterialId::new("mat"));
    s.nodes.insert("root".into(), root);
    let mut mesh = SceneNode::new("Mesh");
    mesh.parent = Some("root".into());
    mesh.source_path = Some("/Root/Mesh".into());
    mesh.mesh = Some("mesh".into());
    mesh.transform[1][3] = 2.;
    s.nodes.insert("child".into(), mesh);
    let mut second = SceneNode::new("Second");
    second.transform[0][3] = -20.;
    s.nodes.insert("second".into(), second);
    s.metadata.nodes.entry("child".into()).or_default().normalized.insert(
        "id".into(),
        MetadataProperty::authored(MetadataValue::String("vendor-42".into())),
    );
    s
}
#[test]
fn hierarchy_operations_preserve_identity_placement_inheritance_and_shared_meshes() {
    let mut doc = SceneDocument::new(fixture(), Default::default()).unwrap();
    let world = doc.world_transform("child").unwrap();
    let positions = doc.scene().meshes["mesh"].positions.as_ptr();
    doc.apply(&[
        SceneEdit::Rename {
            node: "child".into(),
            name: "Chair".into(),
        },
        SceneEdit::Reparent {
            node: "child".into(),
            parent: Some("second".into()),
            preserve_world: true,
            preserve_inherited: true,
        },
        SceneEdit::DuplicateSubtree {
            node: "child".into(),
            prefix: "copy:".into(),
            parent: None,
            preserve_world: true,
        },
    ])
    .unwrap();
    assert_eq!(doc.world_transform("child").unwrap(), world);
    assert_eq!(doc.world_transform("copy:child").unwrap(), world);
    assert_eq!(doc.node("child").unwrap().material, Some(MaterialId::new("mat")));
    assert_eq!(doc.scene().meshes.len(), 1);
    assert_eq!(doc.scene().meshes["mesh"].positions.as_ptr(), positions);
    assert_eq!(doc.ids_for_path("/Root/Mesh"), ["child", "copy:child"]);
    assert_eq!(
        doc.scene().metadata.nodes["child"],
        doc.scene().metadata.nodes["copy:child"]
    );
    doc.apply(&[SceneEdit::RemoveSubtree { node: "second".into() }])
        .unwrap();
    assert!(doc.node("child").is_none());
    assert!(doc.node("copy:child").is_some());
    assert!(!doc.scene().metadata.nodes.contains_key("child"));
    assert_eq!(doc.scene().meshes["mesh"].positions.as_ptr(), positions);
}
#[test]
fn added_nodes_and_cycles_validate_atomically() {
    let mut doc = SceneDocument::new(fixture(), Default::default()).unwrap();
    let mut node = SceneNode::new("Added");
    node.parent = Some("root".into());
    doc.apply(&[SceneEdit::AddNode {
        id: "added".into(),
        node: Box::new(node),
    }])
    .unwrap();
    assert_eq!(doc.children(Some("root")).unwrap(), ["added", "child"]);
    let before = doc.scene().clone();
    let invalid = SceneNode {
        parent: Some("not-there".into()),
        ..SceneNode::new("Invalid")
    };
    assert!(
        doc.apply(&[SceneEdit::AddNode {
            id: "invalid".into(),
            node: Box::new(invalid)
        }])
        .is_err()
    );
    assert_eq!(doc.scene(), &before);
    assert!(
        doc.apply(&[SceneEdit::DuplicateSubtree {
            node: "root".into(),
            prefix: "".into(),
            parent: None,
            preserve_world: true
        }])
        .is_err()
    );
    assert_eq!(doc.scene(), &before);
}
#[test]
fn pivot_duplicates_only_shared_geometry_and_keeps_all_placements() {
    let mut scene = fixture();
    let mut other = scene.nodes["child"].clone();
    other.parent = None;
    scene.nodes.insert("other".into(), other);
    let mut grandchild = SceneNode::new("Grandchild");
    grandchild.parent = Some("child".into());
    grandchild.transform[2][3] = 3.;
    scene.nodes.insert("grandchild".into(), grandchild);
    let before = scene_bounds(&scene).unwrap();
    let grandchild_world = world_transform(&scene, "grandchild").unwrap();
    let updated = edit_scene(
        &scene,
        &[SceneEdit::SetPivot {
            node: "child".into(),
            pivot: [0.5, 0.25, 0.],
        }],
        &Default::default(),
    )
    .unwrap();
    assert_eq!(scene_bounds(&updated).unwrap(), before);
    assert_eq!(world_transform(&updated, "grandchild").unwrap(), grandchild_world);
    assert_eq!(updated.meshes.len(), 2);
    assert_eq!(updated.meshes["mesh"], scene.meshes["mesh"]);
    assert_ne!(updated.nodes["child"].mesh, updated.nodes["other"].mesh);
}
#[test]
fn axis_conversion_includes_reset_roots_and_round_trips() {
    let mut scene = fixture();
    let node = scene.nodes.get_mut("child").unwrap();
    node.reset_transform = true;
    node.transform[2][3] = 4.;
    let before = world_transform(&scene, "child").unwrap();
    let z = edit_scene(
        &scene,
        &[SceneEdit::ConvertAxis { up_axis: UpAxis::Z }],
        &Default::default(),
    )
    .unwrap();
    let world = world_transform(&z, "child").unwrap();
    assert_eq!(world[1][3], -before[2][3]);
    assert_eq!(world[2][3], before[1][3]);
    let back = edit_scene(
        &z,
        &[SceneEdit::ConvertAxis { up_axis: UpAxis::Y }],
        &Default::default(),
    )
    .unwrap();
    assert_eq!(back.nodes, scene.nodes);
    assert_eq!(back.meshes, scene.meshes);
}
#[test]
fn merge_normalizes_units_and_namespaces_all_identities_and_metadata() {
    let scene = fixture();
    let mut other = fixture();
    other.meters_per_unit = 0.001;
    let merged = edit_scene(
        &scene,
        &[SceneEdit::MergeScene {
            scene: Box::new(other),
            prefix: "incoming:".into(),
            parent: None,
        }],
        &Default::default(),
    )
    .unwrap();
    assert_eq!(merged.nodes.len(), 6);
    assert_eq!(merged.meshes.len(), 2);
    assert_eq!(merged.materials.len(), 2);
    assert_eq!(merged.nodes["incoming:child"].mesh.as_deref(), Some("incoming:mesh"));
    assert_eq!(
        effective_material(&merged, "incoming:child").unwrap(),
        Some(MaterialId::new("incoming:mat"))
    );
    assert_eq!(merged.meshes["incoming:mesh"].positions[1], [0.001, 0., 0.]);
    assert!(merged.metadata.nodes.contains_key("incoming:child"));
    assert!(merged.metadata.scene.original.contains_key("merged_scene:incoming:"));
    assert!(
        edit_scene(
            &merged,
            &[SceneEdit::MergeScene {
                scene: Box::new(scene),
                prefix: "incoming:".into(),
                parent: None
            }],
            &Default::default()
        )
        .is_err()
    );
}
#[test]
fn outgoing_report_failure_rolls_back_without_payload_copy() {
    let mut doc = SceneDocument::new(fixture(), Default::default()).unwrap();
    let geometry = doc.scene().meshes["mesh"].positions.as_ptr();
    let before = doc.scene().clone();
    let error = doc
        .apply_checked(
            &[SceneEdit::Rename {
                node: "child".into(),
                name: "No capacity".into(),
            }],
            |report| {
                assert_eq!(report.generation, 1);
                Err(SceneError::new("host.capacity", "/", "no result slots"))
            },
        )
        .unwrap_err();
    assert_eq!(error.code, "host.capacity");
    assert_eq!(doc.scene(), &before);
    assert_eq!(doc.scene().meshes["mesh"].positions.as_ptr(), geometry);
    assert_eq!(doc.generation(), 0);
}
#[test]
fn compact_diff_reports_metadata_and_mesh_fields_without_payloads() {
    let before = fixture();
    let mut after = before.clone();
    after.meshes.get_mut("mesh").unwrap().positions[0][0] = 1.;
    after.metadata.nodes.get_mut("child").unwrap().normalized.insert(
        "label".into(),
        MetadataProperty::authored(MetadataValue::String("Changed".into())),
    );
    let diff = diff_scenes(&before, &after);
    assert!(
        diff.iter()
            .any(|c| c.entity == "mesh:mesh" && c.fields == ["positions"])
    );
    assert!(diff.iter().any(|c| c.entity == "metadata:node:child"));
    let encoded = serde_json::to_string(&diff).unwrap();
    assert!(!encoded.contains("vendor-42"));
    assert!(!encoded.contains("Changed"));
}
