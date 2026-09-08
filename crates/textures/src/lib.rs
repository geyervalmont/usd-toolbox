//! Texture-set import and deterministic tier generation.
//!
//! The API accepts encoded image bytes. It never reads directories itself, so
//! callers can use the same implementation from native processes and browsers.

use std::collections::BTreeMap;
use std::io::Cursor;
use std::str::FromStr;

use fast_image_resize::{PixelType, Resizer, images::Image as ResizeImage};
use image::{DynamicImage, ImageFormat, RgbaImage};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256 as Sha256Hasher};
use time::OffsetDateTime;
use usd_toolbox_core::{
    Channel, ColorSpace, Derivation, ImportError, Importer, Input, Loss, LossKind, MapRole, Material, NormalConvention,
    Operation, ProvenanceAsset, ProvenanceEvent, Sha256, TextureRef, TextureSource, Tier, Value,
};

/// Metadata which cannot safely be inferred from image pixels.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct TextureMetadata {
    /// Semantic role. When absent, the importer may infer it from the filename.
    pub role: Option<MapRole>,
    /// Declared resolution tier. When absent, the filename and dimensions are used.
    pub tier: Option<Tier>,
    /// Required transfer-function interpretation.
    pub color_space: Option<ColorSpace>,
    /// Selected channel. Defaults to RGB for colour maps and red for scalar maps.
    pub channel: Option<Channel>,
    /// Required for normal maps unless the caller explicitly enables the legacy
    /// filename compatibility mode.
    pub normal_convention: Option<NormalConvention>,
}

/// One encoded texture supplied by a caller.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct TextureInput {
    /// Stable source name used for role/tier hints and provenance.
    pub name: String,
    /// Complete PNG or JPEG bytes.
    pub bytes: Vec<u8>,
    /// Explicit source metadata.
    #[serde(flatten)]
    pub metadata: TextureMetadata,
}

/// Texture-set import configuration.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct TextureImportOptions {
    /// Stable identity assigned to the resulting material.
    pub material_id: String,
    /// Display name assigned to the resulting material.
    pub material_name: String,
    /// Tiers which must exist for every map after import.
    pub required_tiers: Vec<Tier>,
    /// Optional target convention. A differing normal map is flipped and recorded.
    pub normal_target: Option<NormalConvention>,
    /// Permit `_gl`/`_dx` filename hints when explicit normal metadata is absent.
    /// Disabled by default because a filename is not authoritative colour data.
    pub infer_normal_convention_from_filename: bool,
    /// Exact timestamp recorded on generated derivations. Defaults to the Unix epoch
    /// for reproducibility; production callers should supply their ingest time.
    #[serde(with = "time::serde::rfc3339")]
    pub created: OffsetDateTime,
    /// Metadata keyed by exact bundle filename when using the [`Importer`] trait.
    pub files: BTreeMap<String, TextureMetadata>,
}

impl Default for TextureImportOptions {
    fn default() -> Self {
        Self {
            material_id: "material".into(),
            material_name: "Material".into(),
            required_tiers: vec![Tier::Preview, Tier::K1, Tier::K2, Tier::K4, Tier::K8],
            normal_target: None,
            infer_normal_convention_from_filename: false,
            created: OffsetDateTime::UNIX_EPOCH,
            files: BTreeMap::new(),
        }
    }
}

/// Rich texture-import result including non-reversible convention changes.
#[derive(Clone, Debug, PartialEq)]
pub struct TextureImport {
    /// Resulting material.
    pub material: Material,
    /// Ordered conversion report.
    pub losses: Vec<Loss>,
}

/// Texture set importer.
#[derive(Clone, Copy, Debug, Default)]
pub struct TextureSetImporter;

impl TextureSetImporter {
    /// Imports caller-owned images into one material and generates missing tiers.
    pub fn import_set(
        &self,
        inputs: &[TextureInput],
        options: &TextureImportOptions,
    ) -> Result<TextureImport, ImportError> {
        if inputs.is_empty() {
            return Err(ImportError::Missing("at least one texture image".into()));
        }
        if options.material_id.trim().is_empty() {
            return Err(ImportError::Missing("material_id".into()));
        }

        let mut material = Material::new(&options.material_id, &options.material_name);
        material.provenance.builder = Some("usd-toolbox".into());
        material.provenance.builder_version = Some(env!("CARGO_PKG_VERSION").into());
        material.provenance.built_at = Some(options.created);

        let mut grouped: BTreeMap<MapRole, Vec<PreparedTexture>> = BTreeMap::new();
        let mut losses = Vec::new();
        for input in inputs {
            let prepared = prepare_input(input, options, &mut material, &mut losses)?;
            grouped.entry(prepared.role).or_default().push(prepared);
        }

        for (role, prepared) in grouped {
            let texture = assemble_texture(role, prepared, options, &mut material, &mut losses)?;
            assign_texture(&mut material, texture)?;
        }
        losses.sort_by(|left, right| {
            left.material
                .cmp(&right.material)
                .then_with(|| left.parameter.cmp(right.parameter))
                .then_with(|| left.detail.cmp(&right.detail))
        });
        losses.dedup();

        Ok(TextureImport { material, losses })
    }
}

impl Importer for TextureSetImporter {
    type Options = TextureImportOptions;

    fn import(&self, input: Input<'_>, options: &Self::Options) -> Result<Vec<Material>, ImportError> {
        let files = match input {
            Input::Bundle(files) => files,
            Input::Bytes(_) | Input::Named { .. } => {
                return Err(ImportError::Invalid {
                    format: "texture set",
                    detail: "a texture set must be supplied as an Input::Bundle".into(),
                });
            }
        };
        let inputs: Vec<_> = files
            .iter()
            .map(|file| TextureInput {
                name: file.name.into(),
                bytes: file.bytes.to_vec(),
                metadata: options.files.get(file.name).cloned().unwrap_or_default(),
            })
            .collect();
        Ok(vec![self.import_set(&inputs, options)?.material])
    }
}

#[derive(Clone, Debug)]
struct PreparedTexture {
    role: MapRole,
    tier: Tier,
    color_space: ColorSpace,
    channel: Channel,
    normal_convention: Option<NormalConvention>,
    source: TextureSource,
}

fn prepare_input(
    input: &TextureInput,
    options: &TextureImportOptions,
    material: &mut Material,
    losses: &mut Vec<Loss>,
) -> Result<PreparedTexture, ImportError> {
    let source_role = match input.metadata.role {
        Some(role) => role,
        None => infer_role(&input.name)?,
    };
    let role = if source_role == MapRole::Glossiness {
        MapRole::Roughness
    } else {
        source_role
    };
    let color_space = input
        .metadata
        .color_space
        .ok_or_else(|| ImportError::Missing(format!("color_space for `{}`", input.name)))?;
    let channel = input.metadata.channel.unwrap_or_else(|| default_channel(source_role));
    let mut normal_convention = if source_role == MapRole::Normal {
        input.metadata.normal_convention.or_else(|| {
            options
                .infer_normal_convention_from_filename
                .then(|| infer_normal_convention(&input.name))
                .flatten()
        })
    } else {
        input.metadata.normal_convention
    };
    if source_role == MapRole::Normal && normal_convention.is_none() {
        return Err(ImportError::Missing(format!("normal_convention for `{}`", input.name)));
    }

    let original_hash = digest(&input.bytes);
    let format = image::guess_format(&input.bytes).map_err(|error| invalid_image(&input.name, error))?;
    let decoded = image::load_from_memory_with_format(&input.bytes, format)
        .map_err(|error| invalid_image(&input.name, error))?
        .to_rgba8();
    let (width, height) = decoded.dimensions();
    let tier = input
        .metadata
        .tier
        .or_else(|| infer_tier(&input.name))
        .unwrap_or_else(|| closest_tier(width.max(height)));

    let mut transformed = decoded;
    let mut conversion = None;
    if source_role == MapRole::Glossiness {
        invert_channel(&mut transformed, channel);
        conversion = Some((
            "specular_roughness",
            "glossiness pixels were inverted to the roughness convention".to_owned(),
        ));
    }
    if source_role == MapRole::Normal
        && let Some(target) = options.normal_target
        && normal_convention != Some(target)
    {
        flip_green(&mut transformed);
        normal_convention = Some(target);
        conversion = Some((
            "geometry_normal",
            format!("normal map green channel was flipped to {target:?} convention"),
        ));
    }

    let source = if let Some((parameter, detail)) = conversion {
        let (extension, media_type) = format_details(format)?;
        material.provenance.source_assets.insert(
            original_hash.clone(),
            ProvenanceAsset {
                name: input.name.clone(),
                extension: extension.into(),
                media_type: media_type.into(),
                bytes: input.bytes.clone(),
            },
        );
        let bytes = encode_png(&transformed)?;
        let result_hash = digest(&bytes);
        material.provenance.derivations.insert(
            result_hash.clone(),
            Derivation {
                parent: Some(original_hash),
                operation: Operation::Converted,
                tool: "usd-toolbox".into(),
                tool_version: env!("CARGO_PKG_VERSION").into(),
                model: None,
                parameters: json!({ "detail": detail }),
                created: options.created,
            },
        );
        material.provenance.events.push(ProvenanceEvent {
            operation: Operation::Converted,
            detail: detail.clone(),
            created: options.created,
        });
        losses.push(Loss {
            material: material.id.clone(),
            parameter,
            kind: LossKind::Converted,
            detail,
        });
        TextureSource {
            hash: result_hash,
            extension: "png".into(),
            media_type: "image/png".into(),
            width: Some(width),
            height: Some(height),
            bytes,
        }
    } else {
        let (extension, media_type) = format_details(format)?;
        material
            .provenance
            .derivations
            .entry(original_hash.clone())
            .or_insert(Derivation {
                parent: None,
                operation: Operation::Copied,
                tool: "usd-toolbox".into(),
                tool_version: env!("CARGO_PKG_VERSION").into(),
                model: None,
                parameters: json!({ "source_name": input.name }),
                created: options.created,
            });
        TextureSource {
            hash: original_hash,
            extension: extension.into(),
            media_type: media_type.into(),
            width: Some(width),
            height: Some(height),
            bytes: input.bytes.clone(),
        }
    };

    Ok(PreparedTexture {
        role,
        tier,
        color_space,
        channel,
        normal_convention,
        source,
    })
}

fn assemble_texture(
    role: MapRole,
    prepared: Vec<PreparedTexture>,
    options: &TextureImportOptions,
    material: &mut Material,
    losses: &mut Vec<Loss>,
) -> Result<TextureRef, ImportError> {
    let first = prepared.first().ok_or_else(|| ImportError::Missing(role.to_string()))?;
    let color_space = first.color_space;
    let channel = first.channel;
    let normal_convention = first.normal_convention;
    let mut tiers = BTreeMap::new();
    for item in prepared {
        if item.color_space != color_space || item.channel != channel || item.normal_convention != normal_convention {
            return Err(ImportError::Invalid {
                format: "texture set",
                detail: format!("all `{role}` tiers must have identical colour-space, channel, and normal metadata"),
            });
        }
        let item_tier = item.tier;
        if tiers.insert(item_tier, item.source).is_some() {
            return Err(ImportError::Invalid {
                format: "texture set",
                detail: format!(
                    "more than one `{role}` texture was assigned to tier `{}`",
                    item_tier.as_str()
                ),
            });
        }
    }

    let parent = tiers
        .values()
        .max_by_key(|source| source.width.unwrap_or(0).max(source.height.unwrap_or(0)))
        .cloned()
        .ok_or_else(|| ImportError::Missing(role.to_string()))?;
    let parent_image = image::load_from_memory(&parent.bytes)
        .map_err(|error| invalid_image(&parent.package_path(), error))?
        .to_rgba8();
    let parent_longest = parent_image.width().max(parent_image.height());

    let mut required = options.required_tiers.clone();
    required.sort_unstable();
    required.dedup();
    for tier in required {
        if tiers.contains_key(&tier) {
            continue;
        }
        if tier.pixels() > parent_longest {
            return Err(ImportError::Missing(format!(
                "{} tier for `{role}`; the largest source is {parent_longest}px and upscaling is outside the texture importer",
                tier.as_str()
            )));
        }
        let resized = resize_to_longest(&parent_image, tier.pixels())?;
        let bytes = encode_png(&resized)?;
        let hash = digest(&bytes);
        let operation = if tier.pixels() < parent_longest {
            Operation::Downscale
        } else if tier.pixels() > parent_longest {
            Operation::Upscale
        } else {
            Operation::Copied
        };
        let (width, height) = resized.dimensions();
        material
            .provenance
            .derivations
            .entry(hash.clone())
            .or_insert(Derivation {
                parent: Some(parent.hash.clone()),
                operation,
                tool: "fast_image_resize".into(),
                tool_version: "6.1".into(),
                model: None,
                parameters: json!({
                    "filter": "lanczos3",
                    "tier": tier.as_str(),
                    "width": width,
                    "height": height,
                }),
                created: options.created,
            });
        tiers.insert(
            tier,
            TextureSource {
                hash,
                extension: "png".into(),
                media_type: "image/png".into(),
                width: Some(width),
                height: Some(height),
                bytes,
            },
        );
        losses.push(Loss {
            material: material.id.clone(),
            parameter: parameter_for_role(role),
            kind: LossKind::Converted,
            detail: format!(
                "generated missing {} tier from {} using Lanczos3 ({operation:?})",
                tier.as_str(),
                parent.hash
            ),
        });
    }

    Ok(TextureRef {
        role,
        tiers,
        color_space,
        channel,
        normal_convention,
    })
}

fn assign_texture(material: &mut Material, texture: TextureRef) -> Result<(), ImportError> {
    let role = texture.role;
    match role {
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
            return Err(ImportError::Invalid {
                format: "texture set",
                detail: "internal error: glossiness was not converted to roughness".into(),
            });
        }
    }
    Ok(())
}

fn infer_role(name: &str) -> Result<MapRole, ImportError> {
    let normalized = normalize_name(name);
    let candidates: [(MapRole, &[&str]); 12] = [
        (MapRole::BaseColor, &["base_color", "basecolor", "albedo", "diffuse"]),
        (MapRole::Normal, &["normal"]),
        (MapRole::Roughness, &["roughness", "rough"]),
        (MapRole::Glossiness, &["glossiness", "gloss"]),
        (MapRole::Metallic, &["metallic", "metalness"]),
        (MapRole::Height, &["height", "displacement", "disp"]),
        (MapRole::Bump, &["bump"]),
        (MapRole::Ao, &["ambient_occlusion", "occlusion", "ao"]),
        (MapRole::Opacity, &["opacity", "alpha", "mask"]),
        (MapRole::Specular, &["specular", "spec"]),
        (MapRole::Transmission, &["transmission", "translucency"]),
        (MapRole::Emissive, &["emissive", "emission"]),
    ];
    candidates
        .into_iter()
        .find(|(_, aliases)| aliases.iter().any(|alias| has_token(&normalized, alias)))
        .map(|(role, _)| role)
        .ok_or_else(|| ImportError::Missing(format!("texture role for `{name}`")))
}

fn infer_tier(name: &str) -> Option<Tier> {
    let normalized = normalize_name(name);
    let candidates: [(Tier, &[&str]); 5] = [
        (Tier::Preview, &["preview", "thumb", "thumbnail", "512"]),
        (Tier::K1, &["1k", "1024"]),
        (Tier::K2, &["2k", "2048"]),
        (Tier::K4, &["4k", "4096"]),
        (Tier::K8, &["8k", "8192"]),
    ];
    candidates
        .into_iter()
        .find(|(_, aliases)| aliases.iter().any(|alias| has_token(&normalized, alias)))
        .map(|(tier, _)| tier)
}

fn infer_normal_convention(name: &str) -> Option<NormalConvention> {
    let normalized = normalize_name(name);
    if ["dx", "directx", "normal_dx", "normaldx"]
        .iter()
        .any(|alias| has_token(&normalized, alias))
    {
        Some(NormalConvention::DirectX)
    } else if ["gl", "opengl", "normal_gl", "normalgl"]
        .iter()
        .any(|alias| has_token(&normalized, alias))
    {
        Some(NormalConvention::OpenGl)
    } else {
        None
    }
}

fn normalize_name(name: &str) -> String {
    name.to_ascii_lowercase()
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character
            } else {
                '_'
            }
        })
        .collect()
}

fn has_token(normalized: &str, token: &str) -> bool {
    let padded = format!("_{normalized}_");
    padded.contains(&format!("_{token}_")) || normalized.contains(token)
}

const fn default_channel(role: MapRole) -> Channel {
    match role {
        MapRole::BaseColor | MapRole::Normal | MapRole::Emissive => Channel::Rgb,
        _ => Channel::R,
    }
}

fn closest_tier(longest: u32) -> Tier {
    [Tier::Preview, Tier::K1, Tier::K2, Tier::K4, Tier::K8]
        .into_iter()
        .min_by_key(|tier| tier.pixels().abs_diff(longest))
        .unwrap_or(Tier::Preview)
}

const fn parameter_for_role(role: MapRole) -> &'static str {
    match role {
        MapRole::BaseColor => "base_color",
        MapRole::Normal => "geometry_normal",
        MapRole::Roughness | MapRole::Glossiness => "specular_roughness",
        MapRole::Metallic => "base_metalness",
        MapRole::Height => "geometry_height",
        MapRole::Bump => "geometry_bump",
        MapRole::Ao => "ambient_occlusion",
        MapRole::Opacity => "geometry_opacity",
        MapRole::Specular => "specular_weight",
        MapRole::Transmission => "transmission_weight",
        MapRole::Emissive => "emission_color",
    }
}

fn invert_channel(image: &mut RgbaImage, channel: Channel) {
    for pixel in image.pixels_mut() {
        match channel {
            Channel::Rgb => {
                pixel.0[0] = 255 - pixel.0[0];
                pixel.0[1] = 255 - pixel.0[1];
                pixel.0[2] = 255 - pixel.0[2];
            }
            Channel::R => pixel.0[0] = 255 - pixel.0[0],
            Channel::G => pixel.0[1] = 255 - pixel.0[1],
            Channel::B => pixel.0[2] = 255 - pixel.0[2],
            Channel::A => pixel.0[3] = 255 - pixel.0[3],
        }
    }
}

fn flip_green(image: &mut RgbaImage) {
    for pixel in image.pixels_mut() {
        pixel.0[1] = 255 - pixel.0[1];
    }
}

fn resize_to_longest(source: &RgbaImage, longest: u32) -> Result<RgbaImage, ImportError> {
    let source_longest = source.width().max(source.height());
    let width = u64::from(source.width());
    let height = u64::from(source.height());
    let denominator = u64::from(source_longest);
    let target = u64::from(longest);
    let new_width = u32::try_from((width * target + denominator / 2) / denominator)
        .unwrap_or(u32::MAX)
        .max(1);
    let new_height = u32::try_from((height * target + denominator / 2) / denominator)
        .unwrap_or(u32::MAX)
        .max(1);

    let source_image = ResizeImage::from_vec_u8(
        source.width(),
        source.height(),
        source.clone().into_raw(),
        PixelType::U8x4,
    )
    .map_err(|error| ImportError::Invalid {
        format: "image",
        detail: error.to_string(),
    })?;
    let mut destination = ResizeImage::new(new_width, new_height, PixelType::U8x4);
    Resizer::new()
        .resize(&source_image, &mut destination, None)
        .map_err(|error| ImportError::Invalid {
            format: "image",
            detail: error.to_string(),
        })?;
    RgbaImage::from_raw(new_width, new_height, destination.into_vec()).ok_or_else(|| ImportError::Invalid {
        format: "image",
        detail: "resizer returned an invalid RGBA buffer".into(),
    })
}

fn encode_png(image: &RgbaImage) -> Result<Vec<u8>, ImportError> {
    let mut cursor = Cursor::new(Vec::new());
    DynamicImage::ImageRgba8(image.clone())
        .write_to(&mut cursor, ImageFormat::Png)
        .map_err(|error| ImportError::Invalid {
            format: "image",
            detail: format!("could not encode deterministic PNG tier: {error}"),
        })?;
    Ok(cursor.into_inner())
}

fn format_details(format: ImageFormat) -> Result<(&'static str, &'static str), ImportError> {
    match format {
        ImageFormat::Png => Ok(("png", "image/png")),
        ImageFormat::Jpeg => Ok(("jpg", "image/jpeg")),
        _ => Err(ImportError::Unsupported(format!(
            "image format {format:?}; the initial build accepts PNG and JPEG"
        ))),
    }
}

fn digest(bytes: &[u8]) -> Sha256 {
    Sha256(format!("{:x}", Sha256Hasher::digest(bytes)))
}

fn invalid_image(name: &str, error: impl std::fmt::Display) -> ImportError {
    ImportError::Invalid {
        format: "image",
        detail: format!("`{name}`: {error}"),
    }
}

/// Parses a role using the fixed OPAL vocabulary.
pub fn parse_role(value: &str) -> Result<MapRole, ImportError> {
    MapRole::from_str(value).map_err(|detail| ImportError::Invalid {
        format: "texture role",
        detail,
    })
}
