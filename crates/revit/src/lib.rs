//! Autodesk Revit Generic appearance-asset export.
//!
//! Revit does not use USD, XML, or JSON directly for appearance assets.
//! The OPAL connector applies a file-based Generic schema with diffuse, bump,
//! and glossiness slots. This exporter produces that exact portable image set
//! plus a manifest; the connector remains responsible for Revit API calls.

use std::collections::BTreeMap;
use std::io::{Cursor, Write};

use image::{DynamicImage, GenericImageView, ImageFormat, Rgba, RgbaImage};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use usd_toolbox_core::{
    Capabilities, Channel, Export, ExportError, Exporter, Loss, LossKind, MapRole, Material, Repeat, Target,
    TextureSource, Tier, Value, target_capabilities, validate_materials,
};
use zip::CompressionMethod;
use zip::write::SimpleFileOptions;

/// Revit image-set export settings.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct RevitExportOptions {
    /// Preferred source tier; the nearest available tier is selected.
    pub texture_tier: Tier,
    /// Include the connector-readable `revit-material.json` contract.
    pub include_manifest: bool,
}

impl Default for RevitExportOptions {
    fn default() -> Self {
        Self {
            texture_tier: Tier::K2,
            include_manifest: true,
        }
    }
}

/// Produces a ZIP containing Revit Generic appearance image sets.
#[derive(Clone, Copy, Debug, Default)]
pub struct RevitExporter;

#[derive(Debug, Serialize)]
struct RevitManifest<'a> {
    schema: &'static str,
    appearance_schema: &'static str,
    materials: Vec<RevitMaterial<'a>>,
}

#[derive(Debug, Serialize)]
struct RevitMaterial<'a> {
    id: &'a str,
    name: &'a str,
    texture_scale_mm: Option<[f64; 2]>,
    parameters: BTreeMap<&'static str, String>,
    slots: BTreeMap<&'static str, RevitSlot>,
}

#[derive(Debug, Serialize)]
struct RevitSlot {
    property: &'static str,
    connected_asset_schema: &'static str,
    path: String,
    sha256: String,
    media_type: &'static str,
    width: u32,
    height: u32,
    source_role: MapRole,
    source_tier: Option<Tier>,
    conversion: Option<&'static str>,
}

impl Exporter for RevitExporter {
    type Options = RevitExportOptions;

    fn capabilities(&self) -> Capabilities {
        target_capabilities(Target::Revit)
    }

    fn export(&self, materials: &[Material], options: &Self::Options) -> Result<Export, ExportError> {
        if materials.is_empty() {
            return Err(ExportError::InvalidModel("at least one material is required".into()));
        }
        let validation = validate_materials(materials);
        if !validation.is_empty() {
            return Err(ExportError::InvalidModel(
                validation
                    .into_iter()
                    .map(|issue| format!("{}: {}", issue.path, issue.detail))
                    .collect::<Vec<_>>()
                    .join("; "),
            ));
        }

        let mut files = BTreeMap::<String, Vec<u8>>::new();
        let mut manifests = Vec::with_capacity(materials.len());
        let mut losses = self.dry_run(materials);
        for material in materials {
            let prefix = safe_name(&material.id.0);
            let mut slots = BTreeMap::new();
            if let Some(selected) = select_role(material, MapRole::BaseColor, options.texture_tier) {
                let path = format!(
                    "materials/{prefix}/base_color.{}",
                    revit_extension(&selected.source.bytes)?
                );
                files.insert(path.clone(), selected.source.bytes.clone());
                slots.insert("base_color", slot("generic_diffuse", path, &selected, None)?);
                if matches!(&material.surface.base_color, Value::Modulated { .. }) {
                    losses.push(Loss {
                        material: material.id.clone(),
                        parameter: "base_color",
                        kind: LossKind::Reduced,
                        detail: "Revit image-set export cannot preserve a separate base-color modulation factor".into(),
                    });
                }
            } else if let Value::Constant { value } = &material.surface.base_color {
                let bytes = constant_color_png(*value)?;
                let path = format!("materials/{prefix}/base_color.png");
                let sha256 = format!("{:x}", Sha256::digest(&bytes));
                files.insert(path.clone(), bytes);
                slots.insert(
                    "base_color",
                    RevitSlot {
                        property: "generic_diffuse",
                        connected_asset_schema: "UnifiedBitmapSchema",
                        path,
                        sha256,
                        media_type: "image/png",
                        width: 4,
                        height: 4,
                        source_role: MapRole::BaseColor,
                        source_tier: None,
                        conversion: Some("linear_constant_to_srgb_bitmap"),
                    },
                );
                losses.push(Loss {
                    material: material.id.clone(),
                    parameter: "base_color",
                    kind: LossKind::Converted,
                    detail: "Revit connector image-set contract baked the constant base color into a bitmap".into(),
                });
            }
            if let Some(selected) = select_first_role(
                material,
                &[MapRole::Normal, MapRole::Bump, MapRole::Height],
                options.texture_tier,
            ) {
                let path = format!("materials/{prefix}/bump.{}", revit_extension(&selected.source.bytes)?);
                files.insert(path.clone(), selected.source.bytes.clone());
                slots.insert(
                    "bump",
                    slot(
                        "generic_bump_map",
                        path,
                        &selected,
                        (selected.role == MapRole::Normal).then_some("normal_as_generic_bump"),
                    )?,
                );
            }
            if let Some(selected) = select_role(material, MapRole::Roughness, options.texture_tier) {
                let (bytes, width, height) = roughness_to_glossiness(&selected)?;
                let path = format!("materials/{prefix}/glossiness.png");
                let sha256 = format!("{:x}", Sha256::digest(&bytes));
                files.insert(path.clone(), bytes);
                slots.insert(
                    "glossiness",
                    RevitSlot {
                        property: "generic_glossiness",
                        connected_asset_schema: "UnifiedBitmapSchema",
                        path,
                        sha256,
                        media_type: "image/png",
                        width,
                        height,
                        source_role: MapRole::Roughness,
                        source_tier: Some(selected.tier),
                        conversion: Some("invert_roughness"),
                    },
                );
                losses.push(Loss {
                    material: material.id.clone(),
                    parameter: "specular_roughness",
                    kind: LossKind::Converted,
                    detail: "Revit Generic uses glossiness, so roughness pixels were inverted".into(),
                });
            }
            if slots.is_empty() {
                return Err(ExportError::InvalidModel(format!(
                    "material `{}` has no maps Revit Generic can use",
                    material.id
                )));
            }
            let texture_scale_mm = material
                .tiling
                .as_ref()
                .and_then(|tiling| tiling.width_mm.map(|width| [width, tiling.height_mm.unwrap_or(width)]));
            if material
                .tiling
                .as_ref()
                .is_some_and(|tiling| tiling.repeat != Repeat::Straight)
            {
                losses.push(Loss {
                    material: material.id.clone(),
                    parameter: "tiling.repeat",
                    kind: LossKind::Approximated {
                        as_parameter: "straight_repeat",
                    },
                    detail: "Revit Generic connector repeats U/V without preserving the neutral repeat pattern".into(),
                });
            }
            let mut parameters = BTreeMap::from([
                ("Description", format!("{} [{}]", material.name, material.id)),
                ("Model", material.id.0.clone()),
                ("Keywords", format!("opal, {}", material.id)),
            ]);
            if let Some(manufacturer) = material
                .provenance
                .metadata
                .get("manufacturer")
                .and_then(serde_json::Value::as_str)
                .or(material.provenance.supplier_source.as_deref())
            {
                parameters.insert("Manufacturer", manufacturer.into());
            }
            manifests.push(RevitMaterial {
                id: &material.id.0,
                name: &material.name,
                texture_scale_mm,
                parameters,
                slots,
            });
        }
        losses.sort_by(|left, right| {
            left.material
                .cmp(&right.material)
                .then_with(|| left.parameter.cmp(right.parameter))
                .then_with(|| left.detail.cmp(&right.detail))
        });
        losses.dedup();
        if options.include_manifest {
            files.insert(
                "revit-material.json".into(),
                serde_json::to_vec_pretty(&RevitManifest {
                    schema: "usd-toolbox.revit-material.v1",
                    appearance_schema: "GenericSchema",
                    materials: manifests,
                })
                .map_err(|error| encode_error(error.to_string()))?,
            );
        }
        Ok(Export {
            bytes: write_zip(files)?,
            losses,
        })
    }
}

#[derive(Clone)]
struct SelectedTexture {
    role: MapRole,
    tier: Tier,
    channel: Channel,
    source: TextureSource,
}

fn select_first_role(material: &Material, roles: &[MapRole], tier: Tier) -> Option<SelectedTexture> {
    roles.iter().find_map(|role| select_role(material, *role, tier))
}

fn select_role(material: &Material, role: MapRole, requested: Tier) -> Option<SelectedTexture> {
    let mut selected = None;
    material.visit_textures(|_, texture| {
        if selected.is_some() || texture.role != role {
            return;
        }
        if let Some((tier, source)) = texture
            .tiers
            .get_key_value(&requested)
            .or_else(|| texture.tiers.range(requested..).next())
            .or_else(|| texture.tiers.iter().next_back())
        {
            selected = Some(SelectedTexture {
                role,
                tier: *tier,
                channel: texture.channel,
                source: source.clone(),
            });
        }
    });
    selected
}

fn slot(
    property: &'static str,
    path: String,
    selected: &SelectedTexture,
    conversion: Option<&'static str>,
) -> Result<RevitSlot, ExportError> {
    if format!("{:x}", Sha256::digest(&selected.source.bytes)) != selected.source.hash.0 {
        return Err(ExportError::InvalidModel(format!(
            "Revit source `{}` has a payload/hash mismatch",
            selected.source.package_path()
        )));
    }
    let format = image::guess_format(&selected.source.bytes)
        .map_err(|error| encode_error(format!("Revit source image cannot be identified: {error}")))?;
    let media_type = match format {
        ImageFormat::Png => "image/png",
        ImageFormat::Jpeg => "image/jpeg",
        other => {
            return Err(ExportError::Unsupported(format!(
                "Revit image set requires PNG or JPEG, not {other:?}"
            )));
        }
    };
    let decoded = image::load_from_memory_with_format(&selected.source.bytes, format)
        .map_err(|error| encode_error(format!("Revit source image cannot be decoded: {error}")))?;
    let (width, height) = decoded.dimensions();
    Ok(RevitSlot {
        property,
        connected_asset_schema: "UnifiedBitmapSchema",
        path,
        sha256: selected.source.hash.0.clone(),
        media_type,
        width,
        height,
        source_role: selected.role,
        source_tier: Some(selected.tier),
        conversion,
    })
}

fn roughness_to_glossiness(selected: &SelectedTexture) -> Result<(Vec<u8>, u32, u32), ExportError> {
    let decoded = image::load_from_memory(&selected.source.bytes)
        .map_err(|error| encode_error(format!("roughness image cannot be decoded: {error}")))?
        .to_rgba8();
    let (width, height) = decoded.dimensions();
    let mut output = RgbaImage::new(width, height);
    for (x, y, pixel) in decoded.enumerate_pixels() {
        let roughness = match selected.channel {
            Channel::Rgb | Channel::R => pixel.0[0],
            Channel::G => pixel.0[1],
            Channel::B => pixel.0[2],
            Channel::A => pixel.0[3],
        };
        let glossiness = 255 - roughness;
        output.put_pixel(x, y, Rgba([glossiness, glossiness, glossiness, 255]));
    }
    let mut cursor = Cursor::new(Vec::new());
    DynamicImage::ImageRgba8(output)
        .write_to(&mut cursor, ImageFormat::Png)
        .map_err(|error| encode_error(error.to_string()))?;
    Ok((cursor.into_inner(), width, height))
}

fn constant_color_png(color: [f32; 4]) -> Result<Vec<u8>, ExportError> {
    let encoded = [
        linear_to_srgb_u8(color[0]),
        linear_to_srgb_u8(color[1]),
        linear_to_srgb_u8(color[2]),
        (color[3].clamp(0.0, 1.0) * 255.0).round() as u8,
    ];
    let image = RgbaImage::from_pixel(4, 4, Rgba(encoded));
    let mut cursor = Cursor::new(Vec::new());
    DynamicImage::ImageRgba8(image)
        .write_to(&mut cursor, ImageFormat::Png)
        .map_err(|error| encode_error(error.to_string()))?;
    Ok(cursor.into_inner())
}

fn linear_to_srgb_u8(value: f32) -> u8 {
    let value = value.clamp(0.0, 1.0);
    let srgb = if value <= 0.003_130_8 {
        12.92 * value
    } else {
        1.055 * value.powf(1.0 / 2.4) - 0.055
    };
    (srgb * 255.0).round() as u8
}

fn revit_extension(bytes: &[u8]) -> Result<&'static str, ExportError> {
    match image::guess_format(bytes)
        .map_err(|error| encode_error(format!("Revit source image cannot be identified: {error}")))?
    {
        ImageFormat::Png => Ok("png"),
        ImageFormat::Jpeg => Ok("jpg"),
        other => Err(ExportError::Unsupported(format!(
            "Revit image set requires PNG or JPEG, not {other:?}"
        ))),
    }
}

fn write_zip(files: BTreeMap<String, Vec<u8>>) -> Result<Vec<u8>, ExportError> {
    let cursor = Cursor::new(Vec::new());
    let mut archive = zip::ZipWriter::new(cursor);
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    for (name, bytes) in files {
        archive
            .start_file(name, options)
            .map_err(|error| encode_error(error.to_string()))?;
        archive
            .write_all(&bytes)
            .map_err(|error| encode_error(error.to_string()))?;
    }
    archive
        .finish()
        .map(Cursor::into_inner)
        .map_err(|error| encode_error(error.to_string()))
}

fn safe_name(value: &str) -> String {
    let value = value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    if value.is_empty() { "material".into() } else { value }
}

fn encode_error(detail: String) -> ExportError {
    ExportError::Encode {
        format: "Revit image set",
        detail,
    }
}
