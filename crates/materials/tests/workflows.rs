use std::collections::BTreeMap;
use std::io::Cursor;

use image::{DynamicImage, ImageFormat, Rgba, RgbaImage};
use time::OffsetDateTime;
use usd_toolbox_core::{Channel, ColorSpace, MapRole, Material, Operation, Tier};
use usd_toolbox_materials::{
    AttachmentProvenance, AuditProfile, EmbeddingOptions, EmbeddingRecord, Fixability, TextureAttachment,
    TierJobAction, TierPolicy, apply_merge_patch, attach_texture, audit_material, cluster_embeddings, inspect,
    migrate_document_json, plan_tiers, prepare_embedding_input,
};

fn png(size: u32) -> Vec<u8> {
    let mut cursor = Cursor::new(Vec::new());
    DynamicImage::ImageRgba8(RgbaImage::from_pixel(size, size, Rgba([128, 64, 32, 255])))
        .write_to(&mut cursor, ImageFormat::Png)
        .unwrap();
    cursor.into_inner()
}

fn provenance(operation: Operation) -> AttachmentProvenance {
    AttachmentProvenance {
        operation,
        tool: "test".into(),
        tool_version: "1".into(),
        model: None,
        parameters: serde_json::json!({}),
        created: OffsetDateTime::UNIX_EPOCH,
    }
}

#[test]
fn attachment_inspection_audit_and_planning_share_one_contract() {
    let mut material = Material::new("stone", "Stone");
    material.provenance.builder = Some("test".into());
    material.provenance.built_at = Some(OffsetDateTime::UNIX_EPOCH);
    attach_texture(
        &mut material,
        TextureAttachment {
            role: MapRole::BaseColor,
            tier: Tier::K1,
            bytes: png(1024),
            color_space: ColorSpace::Srgb,
            channel: Channel::Rgb,
            normal_convention: None,
            parent: None,
            provenance: provenance(Operation::Generated),
        },
    )
    .unwrap();

    assert_eq!(inspect(&[material.clone()]).materials[0].textures.len(), 1);
    let report = audit_material(&material, &AuditProfile::default());
    assert!(report.issues.iter().any(|issue| {
        issue.code == "completeness.missing_role" && issue.fixability == Some(Fixability::ExternalGeneration)
    }));
    let policy = TierPolicy {
        required_roles: vec![MapRole::BaseColor],
        required_tiers: vec![Tier::Preview, Tier::K1, Tier::K2],
    };
    let jobs = plan_tiers(&material, &policy);
    assert_eq!(jobs.len(), 2);
    assert_eq!(jobs[0].action, TierJobAction::Downscale);
    assert_eq!(jobs[1].action, TierJobAction::ExternalGeneration);
}

#[test]
fn merge_patch_edits_metadata_without_exposing_an_untyped_mutator() {
    let material = Material::new("paint", "Old name");
    let updated = apply_merge_patch(&material, &serde_json::json!({ "name": "New name" })).unwrap();
    assert_eq!(updated.name, "New name");
}

#[test]
fn embedding_input_is_model_agnostic_and_repeatable() {
    let mut material = Material::new("paint", "Warm White");
    material.provenance.metadata = BTreeMap::from([("category".into(), serde_json::json!("paint"))]);
    let first = prepare_embedding_input(&material, &EmbeddingOptions::default());
    let second = prepare_embedding_input(&material, &EmbeddingOptions::default());
    assert_eq!(first.input_digest, second.input_digest);
    assert!(first.text.contains("category"));
}

#[test]
fn normal_attachment_requires_an_explicit_convention() {
    let mut material = Material::new("normal", "Normal");
    let error = attach_texture(
        &mut material,
        TextureAttachment {
            role: MapRole::Normal,
            tier: Tier::Preview,
            bytes: png(512),
            color_space: ColorSpace::Raw,
            channel: Channel::Rgb,
            normal_convention: None,
            parent: None,
            provenance: provenance(Operation::Generated),
        },
    )
    .unwrap_err();
    assert!(error.to_string().contains("explicit convention"));
}

#[test]
fn legacy_material_arrays_migrate_to_the_versioned_document() {
    let bytes = serde_json::to_vec(&vec![Material::new("legacy", "Legacy")]).unwrap();
    let document = migrate_document_json(&bytes).unwrap();
    assert_eq!(document.schema, "usd-toolbox.material.v1");
    assert_eq!(document.materials[0].id.0, "legacy");
}

#[test]
fn clustering_keeps_model_versions_and_outliers_explicit() {
    let record = |id: &str, vector| EmbeddingRecord {
        entity_id: id.into(),
        modality: "visual".into(),
        model: "clip".into(),
        model_version: "1".into(),
        input_digest: usd_toolbox_core::Sha256("a".repeat(64)),
        vector,
    };
    let result = cluster_embeddings(
        &[
            record("a", vec![1.0, 0.0]),
            record("b", vec![0.99, 0.01]),
            record("outlier", vec![0.0, 1.0]),
        ],
        0.95,
        2,
    )
    .unwrap();
    assert_eq!(result.assignments.iter().filter(|item| item.outlier).count(), 1);
}
