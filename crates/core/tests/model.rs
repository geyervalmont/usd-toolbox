use std::collections::BTreeMap;

use serde_json::json;
use time::OffsetDateTime;
use usd_toolbox_core::{
    Derivation, LossKind, Material, Operation, ParameterValue, Sha256, Target, Variant, VariantSet, dry_run,
    validate_material,
};

#[test]
fn model_json_round_trip_preserves_variants() {
    let mut material = Material::new("paint-red", "Paint Red");
    material.variants.push(VariantSet {
        name: "colourway".into(),
        default: "red".into(),
        variants: vec![Variant {
            name: "red".into(),
            overrides: BTreeMap::from([(
                "base_color".into(),
                ParameterValue::Color4([0.8, 0.02, 0.01, 1.0].into()),
            )]),
        }],
    });

    let json = serde_json::to_string(&material).unwrap();
    let decoded: Material = serde_json::from_str(&json).unwrap();
    assert_eq!(decoded, material);
}

#[test]
fn gltf_dry_run_reports_unsupported_data_in_stable_order() {
    let mut material = Material::new("fabric", "Fabric");
    material.surface.subsurface_weight = Some(0.4.into());
    material.geometry.height = Some(0.5.into());

    let losses = dry_run(&[material], Target::Gltf);
    assert_eq!(losses.len(), 2);
    assert_eq!(losses[0].parameter, "geometry_height");
    assert_eq!(losses[0].kind, LossKind::Dropped);
    assert_eq!(losses[1].parameter, "subsurface_weight");
}

#[test]
fn broken_derivation_parent_is_an_error() {
    let missing = Sha256("00".repeat(32));
    let result = Sha256("11".repeat(32));
    let mut material = Material::new("broken", "Broken");
    material.provenance.derivations.insert(
        result,
        Derivation {
            parent: Some(missing.clone()),
            operation: Operation::Generated,
            tool: "test".into(),
            tool_version: "1".into(),
            model: Some("fixture".into()),
            parameters: json!({}),
            created: OffsetDateTime::UNIX_EPOCH,
        },
    );

    let issues = validate_material(&material);
    assert_eq!(issues.len(), 1);
    assert!(issues[0].detail.contains(&missing.0));
}

#[test]
fn cyclic_derivation_chain_is_an_error() {
    let first = Sha256("22".repeat(32));
    let second = Sha256("33".repeat(32));
    let derivation = |parent| Derivation {
        parent: Some(parent),
        operation: Operation::Converted,
        tool: "test".into(),
        tool_version: "1".into(),
        model: None,
        parameters: json!({}),
        created: OffsetDateTime::UNIX_EPOCH,
    };
    let mut material = Material::new("cycle", "Cycle");
    material
        .provenance
        .derivations
        .insert(first.clone(), derivation(second.clone()));
    material.provenance.derivations.insert(second, derivation(first));

    let issues = validate_material(&material);
    assert!(issues.iter().any(|issue| issue.detail.contains("cycle")));
}

#[test]
fn invalid_physical_parameter_ranges_are_rejected() {
    let mut material = Material::new("invalid", "Invalid");
    material.surface.base_metalness = 1.2.into();
    material.surface.specular_ior = Some(0.9.into());

    let issues = validate_material(&material);
    assert!(issues.iter().any(|issue| issue.path == "surface.base_metalness"));
    assert!(issues.iter().any(|issue| issue.path == "surface.specular_ior"));
}
