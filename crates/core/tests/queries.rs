use usd_toolbox_core::scene::*;
use usd_toolbox_core::{Material, MaterialId};

fn fixture() -> Scene {
    let mut scene = Scene::new("catalogue");
    scene.meters_per_unit = 0.01;
    scene.meshes.insert(
        "triangle".into(),
        Mesh {
            positions: vec![[0., 0., 0.], [100., 0., 0.], [0., 100., 0.]],
            face_counts: vec![3],
            face_indices: vec![0, 1, 2],
            ..Default::default()
        },
    );
    scene.materials.push(Material::new("red", "Red"));
    let mut root = SceneNode::new("Assembly");
    root.source_path = Some("/Assembly".into());
    root.material = Some(MaterialId::new("red"));
    root.transform[0][3] = 100.;
    scene.nodes.insert("root".into(), root);
    for (id, name, parent, x) in [
        ("a", "Chair A", Some("root"), 10.),
        ("b", "Chair B", Some("root"), 20.),
        ("c", "Light", None, 30.),
    ] {
        let mut node = SceneNode::new(name);
        node.source_path = Some(format!("/Assembly/{id}"));
        node.parent = parent.map(str::to_owned);
        node.mesh = Some("triangle".into());
        node.transform[0][3] = x;
        scene.nodes.insert(id.into(), node);
    }
    let metadata = scene.metadata.nodes.entry("a".into()).or_default();
    metadata.normalized.insert(
        "product_id".into(),
        MetadataProperty::authored(MetadataValue::U64("18446744073709551615".into())),
    );
    metadata.normalized.insert(
        "label".into(),
        MetadataProperty::authored(MetadataValue::String("椅子 🪑".into())),
    );
    metadata
        .normalized
        .insert("nullable".into(), MetadataProperty::authored(MetadataValue::Null));
    metadata.original.insert(
        "deferred".into(),
        MetadataProperty {
            value: MetadataValue::String("raw".into()),
            state: MetadataState::Unresolved,
            source: None,
            unit: None,
        },
    );
    scene
}
fn document() -> SceneDocument {
    SceneDocument::with_options(
        fixture(),
        DocumentOptions {
            metadata_indexes: vec![MetadataIndex {
                namespace: MetadataNamespace::Normalized,
                key: "product_id".into(),
            }],
            ..Default::default()
        },
    )
    .unwrap()
}

#[test]
fn indexed_lookup_hierarchy_and_payload_free_projection() {
    let doc = document();
    assert_eq!(doc.ids_for_path("/Assembly/a"), ["a"]);
    assert_eq!(doc.node("a").unwrap().name, "Chair A");
    assert_eq!(doc.children(Some("root")).unwrap(), ["a", "b"]);
    assert_eq!(doc.ancestors("a").unwrap(), ["root"]);
    assert_eq!(doc.descendants("root").unwrap(), ["a", "b"]);
    let row = doc
        .get_node(
            "a",
            &[
                QueryField::Name,
                QueryField::Bounds,
                QueryField::EffectiveMaterial,
                QueryField::Metadata,
            ],
        )
        .unwrap()
        .unwrap();
    assert_eq!(row.fields["bounds"]["value"]["min"], serde_json::json!([1.1, 0., 0.]));
    assert_eq!(row.fields["effective_material"]["material"], "red");
    let json = serde_json::to_string(&row).unwrap();
    assert!(!json.contains("face_indices"));
    assert!(!json.contains("\"positions\":"));
    assert!(json.contains("18446744073709551615"));
    assert!(doc.get_node("absent", &[]).unwrap().is_none());
}
#[test]
fn structured_filter_deserialization_missing_unresolved_and_exact_integer_semantics() {
    let doc = document();
    let request:QueryRequest=serde_json::from_value(serde_json::json!({"filter":{"metadata":[{"namespace":"normalized","key":"product_id","operator":"equals","value":{"type":"u64","value":"18446744073709551615"}}]},"fields":["name"]})).unwrap();
    assert_eq!(doc.query(&request).unwrap().rows[0].id, "a");
    for (filter, count) in [
        (
            serde_json::json!({"metadata":[{"key":"nullable","operator":"equals","value":{"type":"null"}}]}),
            1,
        ),
        (
            serde_json::json!({"metadata":[{"key":"nullable","operator":"exists","exists":false}]}),
            3,
        ),
        (
            serde_json::json!({"metadata":[{"namespace":"original","key":"deferred","operator":"exists","exists":true}]}),
            1,
        ),
        (
            serde_json::json!({"metadata":[{"namespace":"original","key":"deferred","operator":"equals","value":{"type":"string","value":"raw"}}]}),
            0,
        ),
        (
            serde_json::json!({"metadata":[{"key":"product_id","operator":"numeric","comparison":"greater","value":{"type":"u64","value":"18446744073709551614"}}]}),
            1,
        ),
        (
            serde_json::json!({"metadata":[{"key":"product_id","operator":"numeric","comparison":"greater","value":{"type":"f64","value":1.0}}]}),
            0,
        ),
        (
            serde_json::json!({"name":{"value":"chair","mode":"prefix","ignore_case":true},"material":"red","subtree":"root"}),
            2,
        ),
    ] {
        let filter: NodeFilter = serde_json::from_value(filter).unwrap();
        assert_eq!(doc.count(&filter).unwrap(), count);
    }
    let invalid:NodeFilter=serde_json::from_value(serde_json::json!({"metadata":[{"key":"x","operator":"numeric","comparison":"less","value":{"type":"string","value":"1"}}]})).unwrap();
    assert_eq!(doc.count(&invalid).unwrap_err().code, "query.numeric");
}
#[test]
fn pagination_is_stable_content_bound_and_invalidated_by_edits() {
    let mut doc = document();
    let request = QueryRequest {
        limit: 1,
        sort: QuerySort::Name,
        ..Default::default()
    };
    let first = doc.query(&request).unwrap();
    assert_eq!(first.rows[0].id, "root");
    let next = QueryRequest {
        cursor: first.next_cursor.clone(),
        ..request.clone()
    };
    assert_eq!(doc.query(&next).unwrap().rows[0].id, "a");
    // A new handle/process with exactly the same content may continue the page.
    assert_eq!(document().query(&next).unwrap().rows[0].id, "a");
    let mismatch = QueryRequest {
        fields: vec![QueryField::Metadata],
        ..next.clone()
    };
    assert_eq!(doc.query(&mismatch).unwrap_err().code, "query.cursor_mismatch");
    let mut different = fixture();
    different.meshes.get_mut("triangle").unwrap().positions[0][0] = 99.;
    let other = SceneDocument::new(different, Default::default()).unwrap();
    assert_eq!(other.query(&next).unwrap_err().code, "query.stale_cursor");
    doc.apply(&[SceneEdit::Rename {
        node: "b".into(),
        name: "Other".into(),
    }])
    .unwrap();
    assert_eq!(doc.query(&next).unwrap_err().code, "query.stale_cursor");
    let mut request = QueryRequest {
        limit: 1,
        descending: true,
        ..Default::default()
    };
    let mut found = Vec::new();
    loop {
        let page = doc.query(&request).unwrap();
        found.extend(page.rows.into_iter().map(|row| row.id));
        request.cursor = page.next_cursor;
        if request.cursor.is_none() {
            break;
        }
    }
    assert_eq!(found, ["root", "c", "b", "a"]);
}
#[test]
fn metadata_edit_is_atomic_and_keeps_geometry_and_material_allocations() {
    let mut doc = document();
    let positions = doc.scene().meshes["triangle"].positions.as_ptr();
    let material = doc.scene().materials.as_ptr();
    let bounds = doc.bounds("a").unwrap();
    let report = doc
        .apply(&[SceneEdit::SetMetadata {
            target: MetadataTarget::Node("a".into()),
            namespace: MetadataNamespace::Normalized,
            key: "label".into(),
            value: Some(MetadataProperty::authored(MetadataValue::String("New".into()))),
        }])
        .unwrap();
    assert_eq!(report.generation, 1);
    assert_eq!(doc.scene().meshes["triangle"].positions.as_ptr(), positions);
    assert_eq!(doc.scene().materials.as_ptr(), material);
    assert_eq!(doc.bounds("a").unwrap(), bounds);
    assert!(report.affected.iter().any(|c| c.entity == "metadata:node:a"));
    let before = serde_json::to_value(doc.scene()).unwrap();
    let error = doc
        .apply(&[
            SceneEdit::Rename {
                node: "a".into(),
                name: "Must rollback".into(),
            },
            SceneEdit::Reparent {
                node: "root".into(),
                parent: Some("a".into()),
                preserve_world: true,
                preserve_inherited: false,
            },
        ])
        .unwrap_err();
    assert_eq!(error.code, "scene.cycle");
    assert_eq!(doc.generation(), 1);
    assert_eq!(serde_json::to_value(doc.scene()).unwrap(), before);
    assert_eq!(doc.scene().meshes["triangle"].positions.as_ptr(), positions);
    assert_eq!(doc.bounds("a").unwrap(), bounds);
}
#[test]
fn transform_cache_and_metadata_indexes_refresh_after_successful_edits() {
    let mut doc = document();
    assert_eq!(doc.world_transform("a").unwrap()[0][3], 110.);
    let mut transform = IDENTITY;
    transform[0][3] = 200.;
    doc.apply(&[
        SceneEdit::SetTransform {
            node: "root".into(),
            transform,
        },
        SceneEdit::SetMetadata {
            target: MetadataTarget::Node("a".into()),
            namespace: MetadataNamespace::Normalized,
            key: "product_id".into(),
            value: None,
        },
    ])
    .unwrap();
    assert_eq!(doc.world_transform("a").unwrap()[0][3], 210.);
    assert_eq!(doc.bounds("a").unwrap().unwrap().min[0], 2.1);
    let filter = NodeFilter {
        metadata: vec![MetadataFilter {
            namespace: MetadataNamespace::Normalized,
            key: "product_id".into(),
            predicate: MetadataPredicate::Equals {
                value: MetadataValue::U64("18446744073709551615".into()),
            },
        }],
        ..Default::default()
    };
    assert_eq!(doc.count(&filter).unwrap(), 0);
}
#[test]
fn metadata_types_and_independent_budgets_are_enforced() {
    let mut s = fixture();
    s.metadata.scene.original.insert(
        "typed".into(),
        MetadataProperty::authored(MetadataValue::Dictionary(
            [
                (
                    "token".into(),
                    MetadataValue::Typed {
                        value_type: "token".into(),
                        value: Box::new(MetadataValue::String("空".into())),
                    },
                ),
                (
                    "values".into(),
                    MetadataValue::TypedArray {
                        element_type: "int64".into(),
                        values: vec![
                            MetadataValue::I64(i64::MIN.to_string()),
                            MetadataValue::I64(i64::MAX.to_string()),
                        ],
                    },
                ),
            ]
            .into(),
        )),
    );
    let serialized = serde_json::to_string(&s).unwrap();
    let decoded: Scene = serde_json::from_str(&serialized).unwrap();
    assert_eq!(s, decoded);
    validate_metadata(&decoded.metadata, &Default::default()).unwrap();
    let mut doc = SceneDocument::with_options(
        s,
        DocumentOptions {
            metadata_limits: MetadataLimits {
                max_property_bytes: 1024,
                ..Default::default()
            },
            ..Default::default()
        },
    )
    .unwrap();
    let bad = SceneEdit::SetMetadata {
        target: MetadataTarget::Scene,
        namespace: MetadataNamespace::Original,
        key: "large".into(),
        value: Some(MetadataProperty::authored(MetadataValue::String("x".repeat(2000)))),
    };
    assert_eq!(doc.apply(&[bad]).unwrap_err().code, "metadata.limit");
    assert_eq!(doc.generation(), 0);
    let tiny = SceneDocument::with_options(
        fixture(),
        DocumentOptions {
            query_limits: QueryLimits {
                max_response_bytes: 100,
                ..Default::default()
            },
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        tiny.get_metadata(&MetadataTarget::Node("a".into())).unwrap_err().code,
        "query.response_limit"
    );
    let deep = MetadataValue::Array(vec![MetadataValue::Array(vec![MetadataValue::Null])]);
    let mut metadata = SceneMetadata::default();
    metadata
        .scene
        .original
        .insert("deep".into(), MetadataProperty::authored(deep));
    assert_eq!(
        validate_metadata(
            &metadata,
            &MetadataLimits {
                max_depth: 2,
                ..Default::default()
            }
        )
        .unwrap_err()
        .code,
        "metadata.depth"
    );
    metadata.scene.original.insert(
        "deep".into(),
        MetadataProperty::authored(MetadataValue::U64("01".into())),
    );
    assert_eq!(
        validate_metadata(&metadata, &Default::default()).unwrap_err().code,
        "metadata.integer"
    );
}
#[test]
fn bounded_index_budget_rejects_before_document_creation() {
    assert_eq!(
        SceneDocument::with_options(
            fixture(),
            DocumentOptions {
                query_limits: QueryLimits {
                    max_index_bytes: 32,
                    ..Default::default()
                },
                ..Default::default()
            }
        )
        .err()
        .unwrap()
        .code,
        "query.index_limit"
    );
}

#[test]
fn small_metadata_fast_path_updates_only_needed_state_and_rolls_back_reserved_output() {
    let mut doc = document();
    let node = doc.node("a").unwrap() as *const SceneNode;
    let geometry = doc.scene().meshes["triangle"].positions.as_ptr();
    let before = doc.scene().clone();
    let edit = SceneEdit::SetMetadata {
        target: MetadataTarget::Node("a".into()),
        namespace: MetadataNamespace::Normalized,
        key: "product_id".into(),
        value: Some(MetadataProperty::authored(MetadataValue::U64("7".into()))),
    };
    assert!(
        doc.apply_checked(std::slice::from_ref(&edit), |_| Err(SceneError::new(
            "host.full",
            "/",
            "result budget"
        )))
        .is_err()
    );
    assert_eq!(doc.scene(), &before);
    assert_eq!(doc.generation(), 0);
    doc.apply(&[edit]).unwrap();
    assert_eq!(doc.node("a").unwrap() as *const SceneNode, node);
    assert_eq!(doc.scene().meshes["triangle"].positions.as_ptr(), geometry);
    let filter = NodeFilter {
        metadata: vec![MetadataFilter {
            namespace: MetadataNamespace::Normalized,
            key: "product_id".into(),
            predicate: MetadataPredicate::Equals {
                value: MetadataValue::U64("7".into()),
            },
        }],
        ..Default::default()
    };
    assert_eq!(
        doc.query(&QueryRequest {
            filter,
            ..Default::default()
        })
        .unwrap()
        .rows[0]
            .id,
        "a"
    );
    let rename = SceneEdit::Rename {
        node: "a".into(),
        name: "ZZZ".into(),
    };
    doc.apply(&[rename]).unwrap();
    assert_eq!(doc.node("a").unwrap() as *const SceneNode, node);
    let page = doc
        .query(&QueryRequest {
            sort: QuerySort::Name,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(page.rows.last().unwrap().id, "a");
}
#[test]
fn incremental_metadata_accounting_enforces_global_budget_and_recovers_after_removal() {
    let mut s = Scene::new("budget");
    s.nodes.insert("a".into(), SceneNode::new("A"));
    s.nodes.insert("b".into(), SceneNode::new("B"));
    let mut doc = SceneDocument::with_options(
        s,
        DocumentOptions {
            metadata_limits: MetadataLimits {
                max_bytes: 220,
                max_property_bytes: 200,
                ..Default::default()
            },
            ..Default::default()
        },
    )
    .unwrap();
    let set = |node: &str, value: Option<MetadataProperty>| SceneEdit::SetMetadata {
        target: MetadataTarget::Node(node.into()),
        namespace: MetadataNamespace::Normalized,
        key: "value".into(),
        value,
    };
    let property = MetadataProperty::authored(MetadataValue::String("x".repeat(100)));
    doc.apply(&[set("a", Some(property.clone()))]).unwrap();
    assert_eq!(
        doc.apply(&[set("b", Some(property.clone()))]).unwrap_err().code,
        "metadata.limit"
    );
    assert_eq!(doc.generation(), 1);
    doc.apply(&[set("a", None), set("b", Some(property))]).unwrap();
    assert_eq!(doc.generation(), 2);
    assert!(
        doc.get_property(
            &MetadataTarget::Node("a".into()),
            MetadataNamespace::Normalized,
            "value"
        )
        .unwrap()
        .is_none()
    );
}

#[test]
fn operation_history_budget_applies_to_incremental_edit_paths() {
    let mut doc = SceneDocument::new(
        fixture(),
        SceneLimits {
            max_operations: 1,
            ..Default::default()
        },
    )
    .unwrap();
    doc.apply(&[SceneEdit::Rename {
        node: "a".into(),
        name: "Renamed".into(),
    }])
    .unwrap();
    let before = doc.scene().clone();
    assert_eq!(
        doc.apply(&[SceneEdit::SetMetadata {
            target: MetadataTarget::Scene,
            namespace: MetadataNamespace::Normalized,
            key: "label".into(),
            value: None
        }])
        .unwrap_err()
        .code,
        "scene.operation_limit"
    );
    assert_eq!(doc.scene(), &before);
}

#[test]
fn metadata_filters_reject_unknown_fields_and_invalid_typed_values() {
    assert!(
        serde_json::from_value::<NodeFilter>(
            serde_json::json!({"metadata":[{"key":"x","operator":"exists","exists":true,"arbitrary_code":"x"}]})
        )
        .is_err()
    );
    let doc = document();
    let invalid = NodeFilter {
        metadata: vec![MetadataFilter {
            namespace: MetadataNamespace::Normalized,
            key: "x".into(),
            predicate: MetadataPredicate::Equals {
                value: MetadataValue::U64("01".into()),
            },
        }],
        ..Default::default()
    };
    assert_eq!(doc.count(&invalid).unwrap_err().code, "query.metadata_value");
}
