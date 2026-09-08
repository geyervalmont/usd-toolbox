//! Neutral-model validation, including provenance-chain integrity.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::{Material, Sha256, Value};

/// Severity of a model validation issue.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ValidationIssueKind {
    /// The material cannot be safely exported.
    Error,
    /// The material is valid but likely incomplete.
    Warning,
}

/// One deterministic model-validation diagnostic.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ValidationIssue {
    /// Severity.
    pub kind: ValidationIssueKind,
    /// Stable dotted field path.
    pub path: String,
    /// Human-readable detail.
    pub detail: String,
}

/// Validates one material, including all referenced derivation parents.
#[must_use]
pub fn validate_material(material: &Material) -> Vec<ValidationIssue> {
    let mut issues = Vec::new();
    if material.id.0.trim().is_empty() {
        error(&mut issues, "id", "material identity cannot be empty");
    }
    if material.name.trim().is_empty() {
        error(&mut issues, "name", "material name cannot be empty");
    }
    validate_color4_unit("surface.base_color", &material.surface.base_color, &mut issues);
    validate_scalar_unit("surface.base_metalness", &material.surface.base_metalness, &mut issues);
    validate_scalar_unit(
        "surface.specular_roughness",
        &material.surface.specular_roughness,
        &mut issues,
    );
    validate_optional_scalar_min("surface.specular_ior", &material.surface.specular_ior, 1.0, &mut issues);
    for (path, value) in [
        ("surface.specular_weight", &material.surface.specular_weight),
        ("surface.specular_anisotropy", &material.surface.specular_anisotropy),
        ("surface.transmission_weight", &material.surface.transmission_weight),
        ("surface.coat_weight", &material.surface.coat_weight),
        ("surface.coat_roughness", &material.surface.coat_roughness),
        ("surface.fuzz_weight", &material.surface.fuzz_weight),
        ("surface.fuzz_roughness", &material.surface.fuzz_roughness),
        ("surface.subsurface_weight", &material.surface.subsurface_weight),
        ("geometry.opacity", &material.geometry.opacity),
        ("geometry.ambient_occlusion", &material.geometry.ambient_occlusion),
    ] {
        if let Some(value) = value {
            validate_scalar_unit(path, value, &mut issues);
        }
    }
    validate_optional_scalar_min(
        "surface.transmission_thickness",
        &material.surface.transmission_thickness,
        0.0,
        &mut issues,
    );
    if let Some(emission) = &material.surface.emission_color {
        validate_color3_min("surface.emission_color", emission, 0.0, &mut issues);
    }
    if let Some(tiling) = &material.tiling {
        for (path, dimension) in [
            ("tiling.width_mm", tiling.width_mm),
            ("tiling.height_mm", tiling.height_mm),
        ] {
            if dimension.is_some_and(|value| !value.is_finite() || value <= 0.0) {
                error(&mut issues, path, "physical repeat must be a finite positive number");
            }
        }
    }

    let mut content_hashes = BTreeSet::new();
    material.visit_textures(|parameter, texture| {
        if texture.tiers.is_empty() {
            error(
                &mut issues,
                &format!("{parameter}.tiers"),
                "texture reference must contain at least one tier",
            );
        }
        if texture.role == crate::MapRole::Normal && texture.normal_convention.is_none() {
            error(
                &mut issues,
                &format!("{parameter}.normal_convention"),
                "normal maps must declare OpenGL or DirectX convention",
            );
        }
        if parameter == "geometry_normal" && texture.role != crate::MapRole::Normal {
            error(
                &mut issues,
                &format!("{parameter}.role"),
                "geometry_normal must reference a normal-role texture",
            );
        }
        if texture.role == crate::MapRole::Glossiness {
            error(
                &mut issues,
                &format!("{parameter}.role"),
                "glossiness must be converted to neutral roughness before storage",
            );
        }
        if texture.role != crate::MapRole::Normal && texture.normal_convention.is_some() {
            error(
                &mut issues,
                &format!("{parameter}.normal_convention"),
                "normal convention is only valid for normal-role textures",
            );
        }
        for (tier, source) in &texture.tiers {
            validate_hash(
                &source.hash,
                &format!("{parameter}.tiers.{}.hash", tier.as_str()),
                &mut issues,
            );
            if source.bytes.is_empty() {
                error(
                    &mut issues,
                    &format!("{parameter}.tiers.{}.bytes", tier.as_str()),
                    "texture bytes cannot be empty",
                );
            }
            if !valid_extension(&source.extension) {
                error(
                    &mut issues,
                    &format!("{parameter}.tiers.{}.extension", tier.as_str()),
                    "extension must be non-empty lowercase ASCII letters or digits",
                );
            }
            if source.width == Some(0) || source.height == Some(0) {
                error(
                    &mut issues,
                    &format!("{parameter}.tiers.{}.dimensions", tier.as_str()),
                    "texture dimensions must be positive when supplied",
                );
            }
            content_hashes.insert(source.hash.clone());
        }
    });
    for asset in &material.auxiliary {
        validate_hash(&asset.hash, "auxiliary.hash", &mut issues);
        if asset.bytes.is_empty() {
            error(
                &mut issues,
                "auxiliary.bytes",
                &format!("auxiliary asset `{}` has no bytes", asset.name),
            );
        }
        content_hashes.insert(asset.hash.clone());
    }
    for (hash, asset) in &material.provenance.source_assets {
        validate_hash(hash, "provenance.source_assets.hash", &mut issues);
        if asset.bytes.is_empty() {
            error(
                &mut issues,
                "provenance.source_assets.bytes",
                &format!("retained source artifact {hash} has no bytes"),
            );
        }
        if !valid_extension(&asset.extension) {
            error(
                &mut issues,
                "provenance.source_assets.extension",
                "extension must be non-empty lowercase ASCII letters or digits",
            );
        }
        content_hashes.insert(hash.clone());
    }

    let derivation_hashes: BTreeSet<_> = material.provenance.derivations.keys().cloned().collect();
    for (hash, derivation) in &material.provenance.derivations {
        validate_hash(hash, "provenance.derivations.result", &mut issues);
        if let Some(parent) = &derivation.parent {
            validate_hash(parent, "provenance.derivations.parent", &mut issues);
            if parent == hash {
                error(
                    &mut issues,
                    "provenance.derivations.parent",
                    &format!("derivation {hash} cannot name itself as its parent"),
                );
            } else if !content_hashes.contains(parent) && !derivation_hashes.contains(parent) {
                error(
                    &mut issues,
                    "provenance.derivations.parent",
                    &format!("parent hash {parent} does not resolve inside the material"),
                );
            }
        }
    }
    for start in material.provenance.derivations.keys() {
        let mut visited = BTreeSet::new();
        let mut current = start;
        while let Some(derivation) = material.provenance.derivations.get(current) {
            if !visited.insert(current) {
                error(
                    &mut issues,
                    "provenance.derivations.parent",
                    &format!("derivation chain starting at {start} contains a cycle"),
                );
                break;
            }
            let Some(parent) = derivation.parent.as_ref() else {
                break;
            };
            current = parent;
        }
    }

    for (index, set) in material.variants.iter().enumerate() {
        if !set.variants.iter().any(|variant| variant.name == set.default) {
            error(
                &mut issues,
                &format!("variants.{index}.default"),
                "default selection does not name a variant",
            );
        }
        let mut names = BTreeSet::new();
        for variant in &set.variants {
            if !names.insert(&variant.name) {
                error(
                    &mut issues,
                    &format!("variants.{index}.variants"),
                    &format!("duplicate variant name `{}`", variant.name),
                );
            }
        }
    }

    issues.sort_by(|left, right| {
        left.path
            .cmp(&right.path)
            .then_with(|| left.detail.cmp(&right.detail))
            .then_with(|| severity_rank(left.kind).cmp(&severity_rank(right.kind)))
    });
    issues.dedup();
    issues
}

/// Validates multiple materials and prefixes each path with its identity.
#[must_use]
pub fn validate_materials(materials: &[Material]) -> Vec<ValidationIssue> {
    let mut issues = Vec::new();
    let mut ids = BTreeSet::new();
    for material in materials {
        if !ids.insert(&material.id) {
            error(
                &mut issues,
                &format!("materials.{}.id", material.id),
                "duplicate material identity",
            );
        }
        for mut issue in validate_material(material) {
            issue.path = format!("materials.{}.{}", material.id, issue.path);
            issues.push(issue);
        }
    }
    issues.sort_by(|left, right| left.path.cmp(&right.path).then_with(|| left.detail.cmp(&right.detail)));
    issues
}

fn validate_hash(hash: &Sha256, path: &str, issues: &mut Vec<ValidationIssue>) {
    if !hash.is_canonical() {
        error(
            issues,
            path,
            &format!("`{hash}` is not a canonical lowercase SHA-256 digest"),
        );
    }
}

fn valid_extension(extension: &str) -> bool {
    !extension.is_empty()
        && extension
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
}

fn validate_scalar_unit(path: &str, value: &Value<f32>, issues: &mut Vec<ValidationIssue>) {
    if value_factor(value).is_some_and(|value| !value.is_finite() || !(0.0..=1.0).contains(&value)) {
        error(issues, path, "constant/factor must be finite and between 0 and 1");
    }
}

fn validate_optional_scalar_min(
    path: &str,
    value: &Option<Value<f32>>,
    minimum: f32,
    issues: &mut Vec<ValidationIssue>,
) {
    if value
        .as_ref()
        .and_then(value_factor)
        .is_some_and(|value| !value.is_finite() || value < minimum)
    {
        error(
            issues,
            path,
            &format!("constant/factor must be finite and at least {minimum}"),
        );
    }
}

fn validate_color4_unit(path: &str, value: &Value<[f32; 4]>, issues: &mut Vec<ValidationIssue>) {
    if color_factor(value).is_some_and(|value| {
        value
            .iter()
            .any(|component| !component.is_finite() || !(0.0..=1.0).contains(component))
    }) {
        error(
            issues,
            path,
            "constant/factor components must be finite and between 0 and 1",
        );
    }
}

fn validate_color3_min(path: &str, value: &Value<[f32; 3]>, minimum: f32, issues: &mut Vec<ValidationIssue>) {
    if color_factor(value).is_some_and(|value| {
        value
            .iter()
            .any(|component| !component.is_finite() || *component < minimum)
    }) {
        error(
            issues,
            path,
            &format!("constant/factor components must be finite and at least {minimum}"),
        );
    }
}

const fn value_factor(value: &Value<f32>) -> Option<f32> {
    match value {
        Value::Constant { value } => Some(*value),
        Value::Texture { .. } => None,
        Value::Modulated { factor, .. } => Some(*factor),
    }
}

const fn color_factor<const N: usize>(value: &Value<[f32; N]>) -> Option<&[f32; N]> {
    match value {
        Value::Constant { value } => Some(value),
        Value::Texture { .. } => None,
        Value::Modulated { factor, .. } => Some(factor),
    }
}

fn error(issues: &mut Vec<ValidationIssue>, path: &str, detail: &str) {
    issues.push(ValidationIssue {
        kind: ValidationIssueKind::Error,
        path: path.to_owned(),
        detail: detail.to_owned(),
    });
}

const fn severity_rank(kind: ValidationIssueKind) -> u8 {
    match kind {
        ValidationIssueKind::Error => 0,
        ValidationIssueKind::Warning => 1,
    }
}
