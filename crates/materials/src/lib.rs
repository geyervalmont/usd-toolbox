//! High-level material workflows shared by the CLI, native FFI, and WASM.
//!
//! Generation is deliberately outside this crate. The tier planner describes
//! missing work and [`attach_texture`] validates either a human-authored or
//! machine-generated result using the same provenance contract.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use image::GenericImageView;
use serde::{Deserialize, Serialize};
use serde_json::{Value as JsonValue, json};
use sha2::{Digest, Sha256 as Sha256Hasher};
use thiserror::Error;
use time::OffsetDateTime;
use usd_toolbox_core::{
    Channel, ColorSpace, Derivation, MapRole, Material, NormalConvention, Operation, Sha256, TextureRef, TextureSource,
    Tier, ValidationIssueKind, Value, validate_material,
};

/// Versioned neutral JSON envelope used at edit and service boundaries.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct MaterialDocument {
    /// Stable document schema identifier.
    pub schema: String,
    /// Neutral materials with their complete payloads.
    pub materials: Vec<Material>,
}

impl MaterialDocument {
    /// Current schema identifier.
    pub const SCHEMA: &'static str = "usd-toolbox.material.v1";

    /// Wraps neutral materials in the current schema.
    #[must_use]
    pub fn new(materials: Vec<Material>) -> Self {
        Self {
            schema: Self::SCHEMA.into(),
            materials,
        }
    }

    /// Parses and validates a versioned neutral document.
    pub fn from_json(bytes: &[u8]) -> Result<Self, MaterialWorkflowError> {
        let document: Self = serde_json::from_slice(bytes)?;
        if document.schema != Self::SCHEMA {
            return Err(MaterialWorkflowError::UnsupportedSchema(document.schema));
        }
        Ok(document)
    }
}

/// Migrates the legacy raw material-array boundary into the versioned v1
/// envelope. Unknown future schemas are rejected rather than guessed.
pub fn migrate_document_json(bytes: &[u8]) -> Result<MaterialDocument, MaterialWorkflowError> {
    let value: JsonValue = serde_json::from_slice(bytes)?;
    if value.is_array() {
        return Ok(MaterialDocument::new(serde_json::from_value(value)?));
    }
    let schema = value
        .get("schema")
        .and_then(JsonValue::as_str)
        .ok_or_else(|| MaterialWorkflowError::UnsupportedSchema("missing".into()))?;
    if schema != MaterialDocument::SCHEMA {
        return Err(MaterialWorkflowError::UnsupportedSchema(schema.into()));
    }
    Ok(serde_json::from_value(value)?)
}

/// Compact inspection of a material collection.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Inspection {
    pub schema: &'static str,
    pub material_count: usize,
    pub materials: Vec<MaterialInspection>,
}

/// Human- and machine-readable material summary without texture byte arrays.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct MaterialInspection {
    pub id: String,
    pub name: String,
    pub shading_model: String,
    pub textures: Vec<TextureInspection>,
    pub tiling: Option<usd_toolbox_core::Tiling>,
    pub variant_sets: Vec<String>,
    pub provenance_complete: bool,
}

/// One map and its available texture tiers.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct TextureInspection {
    pub parameter: String,
    pub role: MapRole,
    pub color_space: ColorSpace,
    pub channel: Channel,
    pub normal_convention: Option<NormalConvention>,
    pub tiers: Vec<TextureTierInspection>,
}

/// One encoded texture tier summary.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct TextureTierInspection {
    pub tier: Tier,
    pub hash: Sha256,
    pub media_type: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub bytes: usize,
}

/// Produces a byte-free inspection suitable for UI and batch JSONL output.
#[must_use]
pub fn inspect(materials: &[Material]) -> Inspection {
    let materials: Vec<MaterialInspection> = materials
        .iter()
        .map(|material| {
            let mut textures = Vec::new();
            material.visit_textures(|parameter, texture| {
                textures.push(TextureInspection {
                    parameter: parameter.into(),
                    role: texture.role,
                    color_space: texture.color_space,
                    channel: texture.channel,
                    normal_convention: texture.normal_convention,
                    tiers: texture
                        .tiers
                        .iter()
                        .map(|(tier, source)| TextureTierInspection {
                            tier: *tier,
                            hash: source.hash.clone(),
                            media_type: source.media_type.clone(),
                            width: source.width,
                            height: source.height,
                            bytes: source.bytes.len(),
                        })
                        .collect(),
                });
            });
            MaterialInspection {
                id: material.id.0.clone(),
                name: material.name.clone(),
                shading_model: format!("{:?}", material.model).to_ascii_lowercase(),
                textures,
                tiling: material.tiling.clone(),
                variant_sets: material.variants.iter().map(|set| set.name.clone()).collect(),
                provenance_complete: material.provenance.builder.is_some()
                    && material.provenance.built_at.is_some()
                    && (!material.provenance.derivations.is_empty() || material.provenance.source_identifier.is_some()),
            }
        })
        .collect();
    Inspection {
        schema: "usd-toolbox.inspection.v1",
        material_count: materials.len(),
        materials,
    }
}

/// Whether the toolbox can perform a suggested repair itself.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Fixability {
    Automatic,
    ExternalGeneration,
    Manual,
}

/// Audit diagnostic severity.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditSeverity {
    Error,
    Warning,
    Info,
}

/// One stable, actionable audit diagnostic.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AuditIssue {
    pub code: String,
    pub severity: AuditSeverity,
    pub path: String,
    pub message: String,
    pub suggested_fix: Option<String>,
    pub fixability: Option<Fixability>,
}

/// Configurable completeness and metadata policy.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default)]
pub struct AuditProfile {
    pub name: String,
    pub required_roles: Vec<MapRole>,
    pub required_tiers: Vec<Tier>,
    pub require_tiling: bool,
    pub require_provenance: bool,
    pub require_dimensions: bool,
    pub require_nominal_tier_size: bool,
    pub check_uniform_images: bool,
}

impl Default for AuditProfile {
    fn default() -> Self {
        Self {
            name: "portable_pbr".into(),
            required_roles: vec![MapRole::BaseColor, MapRole::Normal, MapRole::Roughness, MapRole::Ao],
            required_tiers: vec![Tier::Preview, Tier::K1, Tier::K2, Tier::K4],
            require_tiling: true,
            require_provenance: true,
            require_dimensions: true,
            require_nominal_tier_size: true,
            check_uniform_images: true,
        }
    }
}

/// Health facets remain visible even when a caller displays the overall score.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct HealthComponents {
    pub structural: u8,
    pub completeness: u8,
    pub metadata: u8,
    pub image_quality: u8,
}

/// Deterministic health report for one material.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct AuditReport {
    pub schema: &'static str,
    pub profile: String,
    pub material_id: String,
    pub valid: bool,
    pub score: u8,
    pub components: HealthComponents,
    pub issues: Vec<AuditIssue>,
}

/// Corpus-level health summary and duplicate candidates.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct LibraryAuditReport {
    pub schema: &'static str,
    pub profile: String,
    pub material_count: usize,
    pub valid_materials: usize,
    pub average_score: u8,
    pub reports: Vec<AuditReport>,
    pub exact_duplicate_images: Vec<DuplicateGroup>,
    pub perceptual_duplicate_candidates: Vec<DuplicateGroup>,
}

/// Repeated content locations sharing a cryptographic or perceptual digest.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct DuplicateGroup {
    pub fingerprint: String,
    pub occurrences: Vec<DuplicateOccurrence>,
}

/// One duplicate image location.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct DuplicateOccurrence {
    pub material_id: String,
    pub role: MapRole,
    pub tier: Tier,
}

/// Audits a complete corpus and adds exact/perceptual duplicate candidates.
#[must_use]
pub fn audit_library(materials: &[Material], profile: &AuditProfile) -> LibraryAuditReport {
    let reports = materials
        .iter()
        .map(|material| audit_material(material, profile))
        .collect::<Vec<_>>();
    let mut exact = BTreeMap::<String, Vec<DuplicateOccurrence>>::new();
    let mut perceptual = BTreeMap::<String, Vec<DuplicateOccurrence>>::new();
    for material in materials {
        material.visit_textures(|_, texture| {
            for (tier, source) in &texture.tiers {
                exact
                    .entry(source.hash.0.clone())
                    .or_default()
                    .push(DuplicateOccurrence {
                        material_id: material.id.0.clone(),
                        role: texture.role,
                        tier: *tier,
                    });
                if texture.role == MapRole::BaseColor
                    && let Some(hash) = difference_hash(&source.bytes)
                {
                    perceptual.entry(hash).or_default().push(DuplicateOccurrence {
                        material_id: material.id.0.clone(),
                        role: texture.role,
                        tier: *tier,
                    });
                }
            }
        });
    }
    let exact_duplicate_images = duplicate_groups(exact, false);
    let perceptual_duplicate_candidates = duplicate_groups(perceptual, true);
    let valid_materials = reports.iter().filter(|report| report.valid).count();
    let average_score = if reports.is_empty() {
        0
    } else {
        (reports.iter().map(|report| u64::from(report.score)).sum::<u64>() / reports.len() as u64) as u8
    };
    LibraryAuditReport {
        schema: "usd-toolbox.library-audit.v1",
        profile: profile.name.clone(),
        material_count: materials.len(),
        valid_materials,
        average_score,
        reports,
        exact_duplicate_images,
        perceptual_duplicate_candidates,
    }
}

fn duplicate_groups(
    groups: BTreeMap<String, Vec<DuplicateOccurrence>>,
    require_distinct_materials: bool,
) -> Vec<DuplicateGroup> {
    groups
        .into_iter()
        .filter(|(_, occurrences)| {
            occurrences.len() > 1
                && (!require_distinct_materials
                    || occurrences
                        .iter()
                        .map(|occurrence| &occurrence.material_id)
                        .collect::<BTreeSet<_>>()
                        .len()
                        > 1)
        })
        .map(|(fingerprint, mut occurrences)| {
            occurrences.sort_by_key(|occurrence| (occurrence.material_id.clone(), occurrence.role, occurrence.tier));
            DuplicateGroup {
                fingerprint,
                occurrences,
            }
        })
        .collect()
}

fn difference_hash(bytes: &[u8]) -> Option<String> {
    let grayscale = image::load_from_memory(bytes).ok()?.to_luma8();
    let resized = image::imageops::resize(&grayscale, 9, 8, image::imageops::FilterType::Triangle);
    let mut bits = 0_u64;
    for y in 0..8 {
        for x in 0..8 {
            if resized.get_pixel(x, y).0[0] > resized.get_pixel(x + 1, y).0[0] {
                bits |= 1 << (y * 8 + x);
            }
        }
    }
    Some(format!("{bits:016x}"))
}

/// Audits actual texture bytes as well as neutral-model completeness.
#[must_use]
pub fn audit_material(material: &Material, profile: &AuditProfile) -> AuditReport {
    let mut issues = Vec::new();
    for issue in validate_material(material) {
        issues.push(AuditIssue {
            code: "model.invalid".into(),
            severity: match issue.kind {
                ValidationIssueKind::Error => AuditSeverity::Error,
                ValidationIssueKind::Warning => AuditSeverity::Warning,
            },
            path: issue.path,
            message: issue.detail,
            suggested_fix: None,
            fixability: Some(Fixability::Manual),
        });
    }

    let mut present_roles = BTreeSet::new();
    material.visit_textures(|parameter, texture| {
        present_roles.insert(texture.role);
        audit_texture(parameter, texture, profile, &mut issues);
    });
    for role in &profile.required_roles {
        if !present_roles.contains(role) {
            push_issue(
                &mut issues,
                "completeness.missing_role",
                AuditSeverity::Warning,
                format!("textures.{role}"),
                format!("required `{role}` map is missing"),
                Some("author or generate the map, then attach it as a new material revision"),
                Some(Fixability::ExternalGeneration),
            );
        }
    }
    if profile.require_tiling && material.tiling.is_none() {
        push_issue(
            &mut issues,
            "metadata.missing_tiling",
            AuditSeverity::Warning,
            "tiling",
            "real-world texture repeat is missing",
            Some("supply width_mm and height_mm from the manufacturer or a measured sample"),
            Some(Fixability::Manual),
        );
    }
    if profile.require_provenance && (material.provenance.builder.is_none() || material.provenance.built_at.is_none()) {
        push_issue(
            &mut issues,
            "metadata.incomplete_provenance",
            AuditSeverity::Warning,
            "provenance",
            "builder and ingest timestamp are required by this profile",
            Some("record source and ingest metadata without changing the content digest"),
            Some(Fixability::Manual),
        );
    }
    issues.sort_by(|left, right| {
        left.path
            .cmp(&right.path)
            .then_with(|| left.code.cmp(&right.code))
            .then_with(|| left.message.cmp(&right.message))
    });
    issues.dedup();

    let components = HealthComponents {
        structural: component_score(&issues, &["model.", "image.decode", "image.hash"]),
        completeness: component_score(&issues, &["completeness."]),
        metadata: component_score(&issues, &["metadata.", "image.dimensions", "image.color_space"]),
        image_quality: component_score(&issues, &["image."]),
    };
    let score = ((u16::from(components.structural)
        + u16::from(components.completeness)
        + u16::from(components.metadata)
        + u16::from(components.image_quality))
        / 4) as u8;
    AuditReport {
        schema: "usd-toolbox.audit.v1",
        profile: profile.name.clone(),
        material_id: material.id.0.clone(),
        valid: !issues.iter().any(|issue| issue.severity == AuditSeverity::Error),
        score,
        components,
        issues,
    }
}

fn audit_texture(parameter: &str, texture: &TextureRef, profile: &AuditProfile, issues: &mut Vec<AuditIssue>) {
    let path = format!("textures.{}", texture.role);
    for tier in &profile.required_tiers {
        if !texture.tiers.contains_key(tier) {
            let larger_exists = texture
                .tiers
                .iter()
                .any(|(available, source)| *available > *tier && longest(source) >= tier.pixels());
            push_issue(
                issues,
                "completeness.missing_tier",
                AuditSeverity::Warning,
                format!("{path}.{}", tier.as_str()),
                format!("required {} texture tier is missing", tier.as_str()),
                Some(if larger_exists {
                    "generate a deterministic downscale and attach it"
                } else {
                    "supply a manual or externally generated result and attach it"
                }),
                Some(if larger_exists {
                    Fixability::Automatic
                } else {
                    Fixability::ExternalGeneration
                }),
            );
        }
    }
    let expected_space = if matches!(texture.role, MapRole::BaseColor | MapRole::Emissive) {
        ColorSpace::Srgb
    } else {
        ColorSpace::Raw
    };
    if texture.color_space != expected_space {
        push_issue(
            issues,
            "image.color_space_unexpected",
            AuditSeverity::Warning,
            format!("{path}.color_space"),
            format!(
                "`{}` normally uses {expected_space:?} rather than {:?}",
                texture.role, texture.color_space
            ),
            Some("confirm the source interpretation; do not infer it from the filename"),
            Some(Fixability::Manual),
        );
    }
    let mut aspect: Option<f64> = None;
    let mut baseline_mean: Option<[f64; 4]> = None;
    for (tier, source) in &texture.tiers {
        let source_path = format!("{path}.{}", tier.as_str());
        if format!("{:x}", Sha256Hasher::digest(&source.bytes)) != source.hash.0 {
            push_issue(
                issues,
                "image.hash_mismatch",
                AuditSeverity::Error,
                format!("{source_path}.hash"),
                "encoded bytes do not match the declared SHA-256",
                Some("quarantine the package and rebuild from a verified source"),
                Some(Fixability::Automatic),
            );
        }
        if let Ok(format) = image::guess_format(&source.bytes) {
            let (extension_matches, media_matches) = match format {
                image::ImageFormat::Png => (source.extension == "png", source.media_type == "image/png"),
                image::ImageFormat::Jpeg => (
                    matches!(source.extension.as_str(), "jpg" | "jpeg" | "jpe"),
                    source.media_type == "image/jpeg",
                ),
                _ => (true, true),
            };
            if !extension_matches || !media_matches {
                push_issue(
                    issues,
                    "image.encoding_metadata_mismatch",
                    AuditSeverity::Error,
                    format!("{source_path}.media_type"),
                    format!(
                        "encoded image is {format:?}; metadata declares .{} / {}",
                        source.extension, source.media_type
                    ),
                    Some("correct the declared extension and media type from the encoded bytes"),
                    Some(Fixability::Automatic),
                );
            }
        }
        match image::load_from_memory(&source.bytes) {
            Ok(decoded) => {
                let (width, height) = decoded.dimensions();
                let stats = image_stats(&decoded);
                if profile.require_dimensions && (source.width != Some(width) || source.height != Some(height)) {
                    push_issue(
                        issues,
                        "image.dimensions_mismatch",
                        AuditSeverity::Error,
                        format!("{source_path}.dimensions"),
                        format!(
                            "decoded image is {width}x{height}; metadata declares {}x{}",
                            display_dimension(source.width),
                            display_dimension(source.height)
                        ),
                        Some("recompute dimensions from the encoded image"),
                        Some(Fixability::Automatic),
                    );
                }
                if profile.require_nominal_tier_size && width.max(height) != tier.pixels() {
                    push_issue(
                        issues,
                        "image.tier_size_mismatch",
                        AuditSeverity::Warning,
                        format!("{source_path}.dimensions"),
                        format!(
                            "{} tier has a {}px longest edge; expected {}px",
                            tier.as_str(),
                            width.max(height),
                            tier.pixels()
                        ),
                        Some("move the image to the correct tier or resample it"),
                        Some(Fixability::Automatic),
                    );
                }
                let current_aspect = f64::from(width) / f64::from(height);
                if aspect.is_some_and(|expected| (expected - current_aspect).abs() > 0.001) {
                    push_issue(
                        issues,
                        "image.aspect_ratio_mismatch",
                        AuditSeverity::Error,
                        format!("{source_path}.dimensions"),
                        "aspect ratio differs from another tier of the same map",
                        Some("regenerate this tier from a verified parent"),
                        Some(Fixability::Automatic),
                    );
                } else {
                    aspect = Some(current_aspect);
                }
                if profile.check_uniform_images && approximately_uniform(&decoded) {
                    push_issue(
                        issues,
                        "image.nearly_uniform",
                        AuditSeverity::Warning,
                        source_path.clone(),
                        format!("`{parameter}` contains almost no pixel variation"),
                        Some("confirm that a constant value was not accidentally exported as a texture"),
                        Some(Fixability::Manual),
                    );
                }
                if let Some(expected) = baseline_mean {
                    let drift = expected
                        .iter()
                        .zip(stats.mean)
                        .take(3)
                        .map(|(left, right)| (left - right).abs())
                        .sum::<f64>()
                        / 3.0;
                    if drift > 0.08 {
                        push_issue(
                            issues,
                            "image.cross_tier_content_mismatch",
                            AuditSeverity::Warning,
                            source_path.clone(),
                            "average image content differs materially from another tier",
                            Some("regenerate every tier from the same verified parent"),
                            Some(Fixability::Automatic),
                        );
                    }
                } else {
                    baseline_mean = Some(stats.mean);
                }
                if texture.role == MapRole::Normal && stats.mean[2] < 0.45 {
                    push_issue(
                        issues,
                        "image.normal_map_implausible",
                        AuditSeverity::Warning,
                        source_path.clone(),
                        "normal map has unusually little positive Z (blue) content",
                        Some("confirm this is a tangent-space normal map and verify its convention"),
                        Some(Fixability::Manual),
                    );
                }
                if !matches!(texture.role, MapRole::BaseColor | MapRole::Opacity)
                    && texture.channel != Channel::A
                    && stats.minimum[3] < 255
                {
                    push_issue(
                        issues,
                        "image.unused_alpha",
                        AuditSeverity::Warning,
                        format!("{source_path}.alpha"),
                        "image contains transparency but this map does not consume alpha",
                        Some("remove the accidental alpha channel or explicitly select it"),
                        Some(Fixability::Automatic),
                    );
                }
            }
            Err(error) => push_issue(
                issues,
                "image.decode_failed",
                AuditSeverity::Error,
                source_path,
                format!("image cannot be decoded: {error}"),
                Some("replace it with a valid PNG or JPEG"),
                Some(Fixability::Manual),
            ),
        }
    }
}

struct ImageStats {
    mean: [f64; 4],
    minimum: [u8; 4],
}

fn image_stats(image: &image::DynamicImage) -> ImageStats {
    let rgba = image.to_rgba8();
    let step = (rgba.len() / 4096).max(1);
    let mut total = [0_u64; 4];
    let mut minimum = [u8::MAX; 4];
    let mut count = 0_u64;
    for pixel in rgba.pixels().step_by(step) {
        count += 1;
        for channel in 0..4 {
            total[channel] += u64::from(pixel.0[channel]);
            minimum[channel] = minimum[channel].min(pixel.0[channel]);
        }
    }
    ImageStats {
        mean: total.map(|value| value as f64 / count.max(1) as f64 / 255.0),
        minimum,
    }
}

fn approximately_uniform(image: &image::DynamicImage) -> bool {
    let rgba = image.to_rgba8();
    let step = (rgba.len() / 4096).max(1);
    let mut minimum = [u8::MAX; 4];
    let mut maximum = [u8::MIN; 4];
    for pixel in rgba.pixels().step_by(step) {
        for channel in 0..4 {
            minimum[channel] = minimum[channel].min(pixel.0[channel]);
            maximum[channel] = maximum[channel].max(pixel.0[channel]);
        }
    }
    (0..3).all(|channel| maximum[channel].saturating_sub(minimum[channel]) <= 2)
}

fn component_score(issues: &[AuditIssue], prefixes: &[&str]) -> u8 {
    let penalty: u16 = issues
        .iter()
        .filter(|issue| prefixes.iter().any(|prefix| issue.code.starts_with(prefix)))
        .map(|issue| match issue.severity {
            AuditSeverity::Error => 35,
            AuditSeverity::Warning => 12,
            AuditSeverity::Info => 2,
        })
        .sum();
    100_u16.saturating_sub(penalty).try_into().unwrap_or(0)
}

fn push_issue(
    issues: &mut Vec<AuditIssue>,
    code: &str,
    severity: AuditSeverity,
    path: impl Into<String>,
    message: impl Into<String>,
    suggested_fix: Option<&str>,
    fixability: Option<Fixability>,
) {
    issues.push(AuditIssue {
        code: code.into(),
        severity,
        path: path.into(),
        message: message.into(),
        suggested_fix: suggested_fix.map(Into::into),
        fixability,
    });
}

/// Tier requirements independent of any generation provider.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default)]
pub struct TierPolicy {
    pub required_roles: Vec<MapRole>,
    pub required_tiers: Vec<Tier>,
}

impl Default for TierPolicy {
    fn default() -> Self {
        let profile = AuditProfile::default();
        Self {
            required_roles: profile.required_roles,
            required_tiers: profile.required_tiers,
        }
    }
}

/// Work required to fill a missing texture tier.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TierJobAction {
    Downscale,
    ExternalGeneration,
    AuthorMissingRole,
}

/// One deterministic texture-tier work item.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct TierJob {
    pub material_id: String,
    pub role: MapRole,
    pub target_tier: Tier,
    pub target_longest_edge: u32,
    pub action: TierJobAction,
    pub parent_tier: Option<Tier>,
    pub parent_hash: Option<Sha256>,
    pub parent_width: Option<u32>,
    pub parent_height: Option<u32>,
}

/// Plans work but never invokes an AI service or overwrites source data.
#[must_use]
pub fn plan_tiers(material: &Material, policy: &TierPolicy) -> Vec<TierJob> {
    let mut jobs = Vec::new();
    for role in &policy.required_roles {
        let mut found = false;
        material.visit_textures(|_, texture| {
            if texture.role != *role || found {
                return;
            }
            found = true;
            for target in &policy.required_tiers {
                if texture.tiers.contains_key(target) {
                    continue;
                }
                let parent = texture.tiers.iter().max_by_key(|(_, source)| longest(source));
                let action = parent.map_or(TierJobAction::AuthorMissingRole, |(_, source)| {
                    if longest(source) >= target.pixels() {
                        TierJobAction::Downscale
                    } else {
                        TierJobAction::ExternalGeneration
                    }
                });
                jobs.push(TierJob {
                    material_id: material.id.0.clone(),
                    role: *role,
                    target_tier: *target,
                    target_longest_edge: target.pixels(),
                    action,
                    parent_tier: parent.map(|(tier, _)| *tier),
                    parent_hash: parent.map(|(_, source)| source.hash.clone()),
                    parent_width: parent.and_then(|(_, source)| source.width),
                    parent_height: parent.and_then(|(_, source)| source.height),
                });
            }
        });
        if !found {
            for target in &policy.required_tiers {
                jobs.push(TierJob {
                    material_id: material.id.0.clone(),
                    role: *role,
                    target_tier: *target,
                    target_longest_edge: target.pixels(),
                    action: TierJobAction::AuthorMissingRole,
                    parent_tier: None,
                    parent_hash: None,
                    parent_width: None,
                    parent_height: None,
                });
            }
        }
    }
    jobs.sort_by_key(|job| (job.role, job.target_tier));
    jobs
}

/// Performs every safe downscale in a tier plan and returns the attached
/// result hashes. Jobs requiring new source information remain in
/// [`plan_tiers`] for an external generator or human.
pub fn generate_missing_downscales(
    material: &mut Material,
    policy: &TierPolicy,
    created: OffsetDateTime,
) -> Result<Vec<Sha256>, MaterialWorkflowError> {
    let jobs = plan_tiers(material, policy);
    let mut attached = Vec::new();
    for job in jobs.into_iter().filter(|job| job.action == TierJobAction::Downscale) {
        let parent_hash = job
            .parent_hash
            .ok_or_else(|| MaterialWorkflowError::InvalidAttachment("downscale job has no parent hash".into()))?;
        let (source, color_space, channel, normal_convention) =
            texture_source_by_hash(material, job.role, &parent_hash)
                .ok_or_else(|| MaterialWorkflowError::TextureNotFound(parent_hash.to_string()))?;
        let decoded = image::load_from_memory(&source.bytes)
            .map_err(|error| MaterialWorkflowError::InvalidAttachment(error.to_string()))?
            .to_rgba8();
        let source_longest = decoded.width().max(decoded.height());
        let target = u64::from(job.target_longest_edge);
        let denominator = u64::from(source_longest);
        let width = u32::try_from((u64::from(decoded.width()) * target + denominator / 2) / denominator)
            .unwrap_or(u32::MAX)
            .max(1);
        let height = u32::try_from((u64::from(decoded.height()) * target + denominator / 2) / denominator)
            .unwrap_or(u32::MAX)
            .max(1);
        let resized = image::imageops::resize(&decoded, width, height, image::imageops::FilterType::Lanczos3);
        let mut cursor = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(resized)
            .write_to(&mut cursor, image::ImageFormat::Png)
            .map_err(|error| MaterialWorkflowError::InvalidAttachment(error.to_string()))?;
        let hash = attach_texture(
            material,
            TextureAttachment {
                role: job.role,
                tier: job.target_tier,
                bytes: cursor.into_inner(),
                color_space,
                channel,
                normal_convention,
                parent: Some(parent_hash),
                provenance: AttachmentProvenance {
                    operation: Operation::Downscale,
                    tool: "usd-toolbox".into(),
                    tool_version: env!("CARGO_PKG_VERSION").into(),
                    model: None,
                    parameters: json!({
                        "filter": "lanczos3",
                        "target_tier": job.target_tier.as_str(),
                        "width": width,
                        "height": height,
                    }),
                    created,
                },
            },
        )?;
        attached.push(hash);
    }
    Ok(attached)
}

/// Caller-declared lineage for a generated or manually authored attachment.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct AttachmentProvenance {
    pub operation: Operation,
    pub tool: String,
    pub tool_version: String,
    pub model: Option<String>,
    #[serde(default)]
    pub parameters: JsonValue,
    #[serde(with = "time::serde::rfc3339")]
    pub created: OffsetDateTime,
}

/// Validated texture attachment. AI and manual results share this exact type.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct TextureAttachment {
    pub role: MapRole,
    pub tier: Tier,
    pub bytes: Vec<u8>,
    pub color_space: ColorSpace,
    pub channel: Channel,
    pub normal_convention: Option<NormalConvention>,
    pub parent: Option<Sha256>,
    pub provenance: AttachmentProvenance,
}

/// Attaches a verified image and records its complete derivation.
pub fn attach_texture(material: &mut Material, attachment: TextureAttachment) -> Result<Sha256, MaterialWorkflowError> {
    if attachment.role == MapRole::Glossiness {
        return Err(MaterialWorkflowError::InvalidAttachment(
            "neutral materials store roughness, not glossiness".into(),
        ));
    }
    if attachment.role == MapRole::Normal && attachment.normal_convention.is_none() {
        return Err(MaterialWorkflowError::InvalidAttachment(
            "normal maps require an explicit convention".into(),
        ));
    }
    if attachment.role != MapRole::Normal && attachment.normal_convention.is_some() {
        return Err(MaterialWorkflowError::InvalidAttachment(
            "normal convention is only valid for a normal map".into(),
        ));
    }
    if matches!(
        attachment.provenance.operation,
        Operation::Downscale | Operation::Upscale | Operation::Converted | Operation::Retouched
    ) && attachment.parent.is_none()
    {
        return Err(MaterialWorkflowError::InvalidAttachment(format!(
            "{:?} attachments require a parent hash",
            attachment.provenance.operation
        )));
    }
    let format = image::guess_format(&attachment.bytes)
        .map_err(|error| MaterialWorkflowError::InvalidAttachment(error.to_string()))?;
    let decoded = image::load_from_memory_with_format(&attachment.bytes, format)
        .map_err(|error| MaterialWorkflowError::InvalidAttachment(error.to_string()))?;
    let (width, height) = decoded.dimensions();
    if width.max(height) != attachment.tier.pixels() {
        return Err(MaterialWorkflowError::InvalidAttachment(format!(
            "{} attachment must have a {}px longest edge; got {width}x{height}",
            attachment.tier.as_str(),
            attachment.tier.pixels()
        )));
    }
    if let Some(parent) = &attachment.parent
        && !hash_resolves(material, parent)
    {
        return Err(MaterialWorkflowError::InvalidAttachment(format!(
            "derivation parent {parent} does not resolve inside the material"
        )));
    }
    let (extension, media_type) = match format {
        image::ImageFormat::Png => ("png", "image/png"),
        image::ImageFormat::Jpeg => ("jpg", "image/jpeg"),
        other => {
            return Err(MaterialWorkflowError::InvalidAttachment(format!(
                "unsupported image format {other:?}; use PNG or JPEG"
            )));
        }
    };
    let hash = Sha256(format!("{:x}", Sha256Hasher::digest(&attachment.bytes)));
    let source = TextureSource {
        hash: hash.clone(),
        extension: extension.into(),
        media_type: media_type.into(),
        width: Some(width),
        height: Some(height),
        bytes: attachment.bytes,
    };
    let texture = ensure_texture(
        material,
        attachment.role,
        attachment.color_space,
        attachment.channel,
        attachment.normal_convention,
    )?;
    texture.tiers.insert(attachment.tier, source);
    material.provenance.derivations.insert(
        hash.clone(),
        Derivation {
            parent: attachment.parent,
            operation: attachment.provenance.operation,
            tool: attachment.provenance.tool,
            tool_version: attachment.provenance.tool_version,
            model: attachment.provenance.model,
            parameters: attachment.provenance.parameters,
            created: attachment.provenance.created,
        },
    );
    Ok(hash)
}

/// Removes one tier. If it was the last tier, the authored texture input is
/// reset to a conservative constant/absence rather than leaving an invalid ref.
pub fn remove_texture(material: &mut Material, role: MapRole, tier: Tier) -> Result<(), MaterialWorkflowError> {
    let Some(texture) = texture_mut(material, role) else {
        return Err(MaterialWorkflowError::TextureNotFound(role.to_string()));
    };
    if texture.tiers.remove(&tier).is_none() {
        return Err(MaterialWorkflowError::TextureNotFound(format!(
            "{} {}",
            role,
            tier.as_str()
        )));
    }
    if texture.tiers.is_empty() {
        clear_texture_input(material, role);
    }
    Ok(())
}

/// Returns one exact encoded role/tier payload without nearest-tier fallback.
#[must_use]
pub fn extract_texture(material: &Material, role: MapRole, tier: Tier) -> Option<Vec<u8>> {
    let mut bytes = None;
    material.visit_textures(|_, texture| {
        if bytes.is_none() && texture.role == role {
            bytes = texture.tiers.get(&tier).map(|source| source.bytes.clone());
        }
    });
    bytes
}

/// Applies RFC 7396-style merge semantics to one material and validates the result.
pub fn apply_merge_patch(material: &Material, patch: &JsonValue) -> Result<Material, MaterialWorkflowError> {
    let mut value = serde_json::to_value(material)?;
    merge_patch(&mut value, patch);
    let updated: Material = serde_json::from_value(value)?;
    let issues = validate_material(&updated);
    if !issues.is_empty() {
        return Err(MaterialWorkflowError::InvalidPatch(
            issues
                .into_iter()
                .map(|issue| format!("{}: {}", issue.path, issue.detail))
                .collect::<Vec<_>>()
                .join("; "),
        ));
    }
    Ok(updated)
}

fn merge_patch(target: &mut JsonValue, patch: &JsonValue) {
    if let JsonValue::Object(patch_object) = patch {
        if !target.is_object() {
            *target = json!({});
        }
        let target_object = target.as_object_mut().expect("target was replaced with an object");
        for (key, value) in patch_object {
            if value.is_null() {
                target_object.remove(key);
            } else {
                merge_patch(target_object.entry(key.clone()).or_insert(JsonValue::Null), value);
            }
        }
    } else {
        target.clone_from(patch);
    }
}

/// One stable semantic JSON difference. Byte arrays are summarized by length.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct MaterialChange {
    pub path: String,
    pub before: Option<JsonValue>,
    pub after: Option<JsonValue>,
}

/// Computes a deterministic field-level material difference.
pub fn diff_materials(before: &Material, after: &Material) -> Result<Vec<MaterialChange>, MaterialWorkflowError> {
    let mut before = serde_json::to_value(before)?;
    let mut after = serde_json::to_value(after)?;
    summarize_bytes(&mut before);
    summarize_bytes(&mut after);
    let mut changes = Vec::new();
    diff_json("", Some(&before), Some(&after), &mut changes);
    Ok(changes)
}

fn summarize_bytes(value: &mut JsonValue) {
    match value {
        JsonValue::Object(object) => {
            for (key, child) in object {
                if key == "bytes" && child.is_array() {
                    *child = json!({ "encoded_byte_length": child.as_array().map_or(0, Vec::len) });
                } else {
                    summarize_bytes(child);
                }
            }
        }
        JsonValue::Array(values) => values.iter_mut().for_each(summarize_bytes),
        _ => {}
    }
}

fn diff_json(path: &str, before: Option<&JsonValue>, after: Option<&JsonValue>, changes: &mut Vec<MaterialChange>) {
    if before == after {
        return;
    }
    match (before, after) {
        (Some(JsonValue::Object(left)), Some(JsonValue::Object(right))) => {
            let keys: BTreeSet<_> = left.keys().chain(right.keys()).collect();
            for key in keys {
                let child_path = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{path}.{key}")
                };
                diff_json(&child_path, left.get(key), right.get(key), changes);
            }
        }
        _ => changes.push(MaterialChange {
            path: path.into(),
            before: before.cloned(),
            after: after.cloned(),
        }),
    }
}

/// Stable visual/text/structured input for caller-selected embedding models.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct EmbeddingInput {
    pub schema: &'static str,
    pub material_id: String,
    pub input_digest: Sha256,
    pub text: String,
    pub structured: Vec<f32>,
    pub structured_labels: Vec<String>,
    pub visual: Option<VisualEmbeddingInput>,
}

/// Canonical image selected for a visual embedding provider.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct VisualEmbeddingInput {
    pub role: MapRole,
    pub tier: Tier,
    pub hash: Sha256,
    pub media_type: String,
    pub bytes: Vec<u8>,
}

/// Options which do not depend on an embedding model or provider.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default)]
pub struct EmbeddingOptions {
    pub visual_tier: Tier,
    pub include_provenance_metadata: bool,
}

impl Default for EmbeddingOptions {
    fn default() -> Self {
        Self {
            visual_tier: Tier::Preview,
            include_provenance_metadata: true,
        }
    }
}

/// Produces deterministic inputs; it intentionally does not choose or invoke a model.
#[must_use]
pub fn prepare_embedding_input(material: &Material, options: &EmbeddingOptions) -> EmbeddingInput {
    let mut text_parts = vec![material.name.clone(), material.id.0.clone()];
    if options.include_provenance_metadata {
        for (key, value) in &material.provenance.metadata {
            text_parts.push(format!("{key}:{}", canonical_json(value)));
        }
        if let Some(supplier) = &material.provenance.supplier_source {
            text_parts.push(format!("supplier:{supplier}"));
        }
    }
    let labels = vec![
        "base_color_r",
        "base_color_g",
        "base_color_b",
        "metalness",
        "roughness",
        "tile_width_m",
        "tile_height_m",
        "has_normal",
        "has_ao",
        "has_height",
        "has_opacity",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect::<Vec<_>>();
    let base = constant_color4(&material.surface.base_color).unwrap_or([0.0; 4]);
    let mut roles = BTreeSet::new();
    material.visit_textures(|_, texture| {
        roles.insert(texture.role);
    });
    let structured = vec![
        base[0],
        base[1],
        base[2],
        constant_scalar(&material.surface.base_metalness).unwrap_or(0.0),
        constant_scalar(&material.surface.specular_roughness).unwrap_or(0.0),
        material
            .tiling
            .as_ref()
            .and_then(|tiling| tiling.width_mm)
            .unwrap_or(0.0) as f32
            / 1000.0,
        material
            .tiling
            .as_ref()
            .and_then(|tiling| tiling.height_mm)
            .unwrap_or(0.0) as f32
            / 1000.0,
        flag(roles.contains(&MapRole::Normal)),
        flag(roles.contains(&MapRole::Ao)),
        flag(roles.contains(&MapRole::Height)),
        flag(roles.contains(&MapRole::Opacity)),
    ];
    let mut visual = None;
    material.visit_textures(|_, texture| {
        if visual.is_none()
            && texture.role == MapRole::BaseColor
            && let Some((tier, source)) = selected_source(texture, options.visual_tier)
        {
            visual = Some(VisualEmbeddingInput {
                role: texture.role,
                tier,
                hash: source.hash.clone(),
                media_type: source.media_type.clone(),
                bytes: source.bytes.clone(),
            });
        }
    });
    let text = text_parts.join("\n");
    let mut hasher = Sha256Hasher::new();
    hasher.update(text.as_bytes());
    for value in &structured {
        hasher.update(value.to_le_bytes());
    }
    if let Some(image) = &visual {
        hasher.update(image.hash.0.as_bytes());
    }
    EmbeddingInput {
        schema: "usd-toolbox.embedding-input.v1",
        material_id: material.id.0.clone(),
        input_digest: Sha256(format!("{:x}", hasher.finalize())),
        text,
        structured,
        structured_labels: labels,
        visual,
    }
}

/// A model-versioned caller-produced embedding.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct EmbeddingRecord {
    pub entity_id: String,
    pub modality: String,
    pub model: String,
    pub model_version: String,
    pub input_digest: Sha256,
    pub vector: Vec<f32>,
}

/// One density-threshold cluster assignment.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ClusterAssignment {
    pub entity_id: String,
    pub cluster: Option<u32>,
    pub outlier: bool,
}

/// Deterministic clustering output which can be versioned by OPAL.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ClusterResult {
    pub schema: &'static str,
    pub similarity_threshold: f32,
    pub min_cluster_size: usize,
    pub assignments: Vec<ClusterAssignment>,
}

/// Groups embeddings by connected cosine-similarity neighbourhoods. This is a
/// model-agnostic baseline; callers may replace it with HDBSCAN while retaining
/// the same model/version/input-digest records.
pub fn cluster_embeddings(
    records: &[EmbeddingRecord],
    similarity_threshold: f32,
    min_cluster_size: usize,
) -> Result<ClusterResult, MaterialWorkflowError> {
    if !(0.0..=1.0).contains(&similarity_threshold) || min_cluster_size == 0 {
        return Err(MaterialWorkflowError::InvalidEmbedding(
            "threshold must be in [0,1] and min_cluster_size must be positive".into(),
        ));
    }
    let dimensions = records.first().map_or(0, |record| record.vector.len());
    if dimensions == 0 || records.iter().any(|record| record.vector.len() != dimensions) {
        return Err(MaterialWorkflowError::InvalidEmbedding(
            "all vectors must be non-empty and have identical dimensions".into(),
        ));
    }
    if records
        .iter()
        .any(|record| record.entity_id.trim().is_empty() || !record.input_digest.is_canonical())
    {
        return Err(MaterialWorkflowError::InvalidEmbedding(
            "every embedding needs an entity id and canonical input digest".into(),
        ));
    }
    let first = &records[0];
    if records.iter().any(|record| {
        record.modality != first.modality || record.model != first.model || record.model_version != first.model_version
    }) {
        return Err(MaterialWorkflowError::InvalidEmbedding(
            "a cluster run cannot mix modalities, models, or model versions".into(),
        ));
    }
    let mut entity_ids = BTreeSet::new();
    if records
        .iter()
        .any(|record| !entity_ids.insert(record.entity_id.as_str()))
    {
        return Err(MaterialWorkflowError::InvalidEmbedding(
            "a cluster run cannot contain duplicate entity ids".into(),
        ));
    }
    let mut visited = vec![false; records.len()];
    let mut components = Vec::new();
    for start in 0..records.len() {
        if visited[start] {
            continue;
        }
        let mut queue = VecDeque::from([start]);
        let mut component = Vec::new();
        visited[start] = true;
        while let Some(index) = queue.pop_front() {
            component.push(index);
            for candidate in 0..records.len() {
                if !visited[candidate]
                    && cosine_similarity(&records[index].vector, &records[candidate].vector)? >= similarity_threshold
                {
                    visited[candidate] = true;
                    queue.push_back(candidate);
                }
            }
        }
        components.push(component);
    }
    components.sort_by_key(|component| {
        component
            .iter()
            .map(|index| records[*index].entity_id.as_str())
            .min()
            .unwrap_or_default()
            .to_owned()
    });
    let mut assignment_by_index = vec![None; records.len()];
    let mut next_cluster = 0_u32;
    for component in components {
        if component.len() >= min_cluster_size {
            for index in component {
                assignment_by_index[index] = Some(next_cluster);
            }
            next_cluster += 1;
        }
    }
    let mut assignments = records
        .iter()
        .enumerate()
        .map(|(index, record)| ClusterAssignment {
            entity_id: record.entity_id.clone(),
            cluster: assignment_by_index[index],
            outlier: assignment_by_index[index].is_none(),
        })
        .collect::<Vec<_>>();
    assignments.sort_by(|left, right| left.entity_id.cmp(&right.entity_id));
    Ok(ClusterResult {
        schema: "usd-toolbox.clusters.v1",
        similarity_threshold,
        min_cluster_size,
        assignments,
    })
}

/// Cosine similarity with finite-value and zero-vector checks.
pub fn cosine_similarity(left: &[f32], right: &[f32]) -> Result<f32, MaterialWorkflowError> {
    if left.len() != right.len() || left.is_empty() || left.iter().chain(right).any(|value| !value.is_finite()) {
        return Err(MaterialWorkflowError::InvalidEmbedding(
            "vectors must be finite, non-empty, and have identical dimensions".into(),
        ));
    }
    let dot = left.iter().zip(right).map(|(left, right)| left * right).sum::<f32>();
    let left_norm = left.iter().map(|value| value * value).sum::<f32>().sqrt();
    let right_norm = right.iter().map(|value| value * value).sum::<f32>().sqrt();
    if left_norm == 0.0 || right_norm == 0.0 {
        return Err(MaterialWorkflowError::InvalidEmbedding(
            "zero vectors have no cosine similarity".into(),
        ));
    }
    Ok((dot / (left_norm * right_norm)).clamp(-1.0, 1.0))
}

/// Errors at the high-level material workflow boundary.
#[derive(Debug, Error)]
pub enum MaterialWorkflowError {
    #[error("unsupported neutral document schema `{0}`")]
    UnsupportedSchema(String),
    #[error("invalid material patch: {0}")]
    InvalidPatch(String),
    #[error("invalid texture attachment: {0}")]
    InvalidAttachment(String),
    #[error("texture not found: {0}")]
    TextureNotFound(String),
    #[error("invalid embedding: {0}")]
    InvalidEmbedding(String),
    #[error("invalid JSON: {0}")]
    Json(#[from] serde_json::Error),
}

fn ensure_texture(
    material: &mut Material,
    role: MapRole,
    color_space: ColorSpace,
    channel: Channel,
    normal_convention: Option<NormalConvention>,
) -> Result<&mut TextureRef, MaterialWorkflowError> {
    if texture_mut(material, role).is_none() {
        let texture = TextureRef {
            role,
            tiers: BTreeMap::new(),
            color_space,
            channel,
            normal_convention,
        };
        assign_texture(material, texture)?;
    }
    let texture = texture_mut(material, role).expect("texture was just assigned");
    if texture.color_space != color_space
        || texture.channel != channel
        || texture.normal_convention != normal_convention
    {
        return Err(MaterialWorkflowError::InvalidAttachment(
            "all tiers for a map must use identical color-space, channel, and normal metadata".into(),
        ));
    }
    Ok(texture)
}

fn texture_mut(material: &mut Material, role: MapRole) -> Option<&mut TextureRef> {
    match role {
        MapRole::BaseColor => value_texture_mut(&mut material.surface.base_color),
        MapRole::Normal => optional_texture_mut(&mut material.geometry.normal),
        MapRole::Roughness => value_texture_mut(&mut material.surface.specular_roughness),
        MapRole::Metallic => value_texture_mut(&mut material.surface.base_metalness),
        MapRole::Height => optional_texture_mut(&mut material.geometry.height),
        MapRole::Bump => optional_texture_mut(&mut material.geometry.bump),
        MapRole::Ao => optional_texture_mut(&mut material.geometry.ambient_occlusion),
        MapRole::Opacity => optional_texture_mut(&mut material.geometry.opacity),
        MapRole::Specular => optional_texture_mut(&mut material.surface.specular_weight),
        MapRole::Transmission => optional_texture_mut(&mut material.surface.transmission_weight),
        MapRole::Emissive => optional_texture_mut(&mut material.surface.emission_color),
        MapRole::Glossiness => None,
    }
}

fn value_texture_mut<T>(value: &mut Value<T>) -> Option<&mut TextureRef> {
    match value {
        Value::Texture { texture } | Value::Modulated { texture, .. } => Some(texture),
        Value::Constant { .. } => None,
    }
}

fn optional_texture_mut<T>(value: &mut Option<Value<T>>) -> Option<&mut TextureRef> {
    value.as_mut().and_then(value_texture_mut)
}

fn assign_texture(material: &mut Material, texture: TextureRef) -> Result<(), MaterialWorkflowError> {
    match texture.role {
        MapRole::BaseColor => material.surface.base_color = Value::Texture { texture },
        MapRole::Normal => material.geometry.normal = Some(Value::Texture { texture }),
        MapRole::Roughness => material.surface.specular_roughness = Value::Texture { texture },
        MapRole::Metallic => material.surface.base_metalness = Value::Texture { texture },
        MapRole::Height => material.geometry.height = Some(Value::Texture { texture }),
        MapRole::Bump => material.geometry.bump = Some(Value::Texture { texture }),
        MapRole::Ao => material.geometry.ambient_occlusion = Some(Value::Texture { texture }),
        MapRole::Opacity => material.geometry.opacity = Some(Value::Texture { texture }),
        MapRole::Specular => material.surface.specular_weight = Some(Value::Texture { texture }),
        MapRole::Transmission => material.surface.transmission_weight = Some(Value::Texture { texture }),
        MapRole::Emissive => material.surface.emission_color = Some(Value::Texture { texture }),
        MapRole::Glossiness => {
            return Err(MaterialWorkflowError::InvalidAttachment(
                "neutral materials cannot contain glossiness".into(),
            ));
        }
    }
    Ok(())
}

fn clear_texture_input(material: &mut Material, role: MapRole) {
    match role {
        MapRole::BaseColor => material.surface.base_color = [0.8, 0.8, 0.8, 1.0].into(),
        MapRole::Roughness => material.surface.specular_roughness = 0.3.into(),
        MapRole::Metallic => material.surface.base_metalness = 0.0.into(),
        MapRole::Normal => material.geometry.normal = None,
        MapRole::Height => material.geometry.height = None,
        MapRole::Bump => material.geometry.bump = None,
        MapRole::Ao => material.geometry.ambient_occlusion = None,
        MapRole::Opacity => material.geometry.opacity = None,
        MapRole::Specular => material.surface.specular_weight = None,
        MapRole::Transmission => material.surface.transmission_weight = None,
        MapRole::Emissive => material.surface.emission_color = None,
        MapRole::Glossiness => {}
    }
}

fn hash_resolves(material: &Material, hash: &Sha256) -> bool {
    if material.provenance.source_assets.contains_key(hash) || material.provenance.derivations.contains_key(hash) {
        return true;
    }
    let mut found = false;
    material.visit_textures(|_, texture| {
        found |= texture.tiers.values().any(|source| &source.hash == hash);
    });
    found
}

fn texture_source_by_hash(
    material: &Material,
    role: MapRole,
    hash: &Sha256,
) -> Option<(TextureSource, ColorSpace, Channel, Option<NormalConvention>)> {
    let mut result = None;
    material.visit_textures(|_, texture| {
        if result.is_none()
            && texture.role == role
            && let Some(source) = texture.tiers.values().find(|source| &source.hash == hash)
        {
            result = Some((
                source.clone(),
                texture.color_space,
                texture.channel,
                texture.normal_convention,
            ));
        }
    });
    result
}

fn selected_source(texture: &TextureRef, requested: Tier) -> Option<(Tier, &TextureSource)> {
    texture
        .tiers
        .get_key_value(&requested)
        .or_else(|| texture.tiers.range(requested..).next())
        .or_else(|| texture.tiers.iter().next_back())
        .map(|(tier, source)| (*tier, source))
}

fn longest(source: &TextureSource) -> u32 {
    source.width.unwrap_or(0).max(source.height.unwrap_or(0))
}

fn display_dimension(value: Option<u32>) -> String {
    value.map_or_else(|| "unknown".into(), |value| value.to_string())
}

fn constant_scalar(value: &Value<f32>) -> Option<f32> {
    match value {
        Value::Constant { value } | Value::Modulated { factor: value, .. } => Some(*value),
        Value::Texture { .. } => None,
    }
}

fn constant_color4(value: &Value<[f32; 4]>) -> Option<[f32; 4]> {
    match value {
        Value::Constant { value } | Value::Modulated { factor: value, .. } => Some(*value),
        Value::Texture { .. } => None,
    }
}

const fn flag(value: bool) -> f32 {
    if value { 1.0 } else { 0.0 }
}

fn canonical_json(value: &JsonValue) -> String {
    match value {
        JsonValue::Object(object) => {
            let sorted = object.iter().collect::<BTreeMap<_, _>>();
            serde_json::to_string(&sorted).unwrap_or_default()
        }
        _ => serde_json::to_string(value).unwrap_or_default(),
    }
}
