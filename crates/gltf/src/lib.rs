//! glTF 2.0 and GLB material export with explicit loss reporting.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Cursor;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use image::{DynamicImage, ImageFormat, Rgba, RgbaImage, imageops::FilterType};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value as JsonValue, json};
use sha2::{Digest, Sha256};
use usd_toolbox_core::{
    Capabilities, Channel, Color3, Color4, Export, ExportError, Exporter, Loss, LossKind, Material, NormalConvention,
    Target, TextureRef, Tier, Value, target_capabilities, validate_materials,
};

/// glTF output container.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GltfFormat {
    /// JSON glTF with image data URIs.
    Gltf,
    /// Binary GLB with image buffer views.
    #[default]
    Glb,
}

/// glTF export settings.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct GltfExportOptions {
    /// Output container.
    pub format: GltfFormat,
    /// Texture resolution selected from each neutral tier set.
    pub texture_tier: Tier,
    /// Pretty-print `.gltf` JSON. GLB JSON is always compact.
    pub pretty: bool,
    /// Preserve Olsyn identity, provenance, tiling, and variants in `extras`.
    pub include_olsyn_extras: bool,
}

impl Default for GltfExportOptions {
    fn default() -> Self {
        Self {
            format: GltfFormat::Glb,
            texture_tier: Tier::K2,
            pretty: false,
            include_olsyn_extras: true,
        }
    }
}

/// glTF/GLB exporter.
#[derive(Clone, Copy, Debug, Default)]
pub struct GltfExporter;

impl Exporter for GltfExporter {
    type Options = GltfExportOptions;

    fn capabilities(&self) -> Capabilities {
        target_capabilities(Target::Gltf)
    }

    fn export(&self, materials: &[Material], options: &Self::Options) -> Result<Export, ExportError> {
        if materials.is_empty() {
            return Err(ExportError::InvalidModel("at least one material is required".into()));
        }
        let issues = validate_materials(materials);
        if !issues.is_empty() {
            return Err(ExportError::InvalidModel(
                issues
                    .into_iter()
                    .map(|issue| format!("{}: {}", issue.path, issue.detail))
                    .collect::<Vec<_>>()
                    .join("; "),
            ));
        }
        validate_payload_hashes(materials)?;

        let mut losses = self.dry_run(materials);
        if !options.include_olsyn_extras {
            for material in materials {
                if material.provenance != Default::default() {
                    losses.push(Loss {
                        material: material.id.clone(),
                        parameter: "provenance",
                        kind: LossKind::Dropped,
                        detail: "glTF Olsyn extras were disabled, so provenance was omitted".into(),
                    });
                }
            }
        }
        let mut registry = ImageRegistry::default();
        let mut extensions_used = BTreeSet::new();
        let mut encoded_materials = Vec::with_capacity(materials.len());
        for material in materials {
            encoded_materials.push(encode_material(
                material,
                options,
                &mut registry,
                &mut extensions_used,
                &mut losses,
            )?);
        }
        sort_losses(&mut losses);

        let mut root = Map::new();
        let mut asset = Map::from_iter([
            ("version".into(), json!("2.0")),
            (
                "generator".into(),
                json!(format!("usd-toolbox {}", env!("CARGO_PKG_VERSION"))),
            ),
        ]);
        if options.include_olsyn_extras {
            asset.insert("extras".into(), json!({ "olsyn": { "losses": losses } }));
        }
        root.insert("asset".into(), JsonValue::Object(asset));
        root.insert("materials".into(), JsonValue::Array(encoded_materials));
        if !extensions_used.is_empty() {
            root.insert("extensionsUsed".into(), json!(extensions_used));
        }
        registry.add_to_document(&mut root, options.format);

        let document = JsonValue::Object(root);
        let bytes = match options.format {
            GltfFormat::Gltf => {
                if options.pretty {
                    serde_json::to_vec_pretty(&document).map_err(json_export_error)?
                } else {
                    serde_json::to_vec(&document).map_err(json_export_error)?
                }
            }
            GltfFormat::Glb => write_glb(&document, &registry.binary)?,
        };
        Ok(Export { bytes, losses })
    }
}

fn encode_material(
    material: &Material,
    options: &GltfExportOptions,
    registry: &mut ImageRegistry,
    extensions_used: &mut BTreeSet<&'static str>,
    losses: &mut Vec<Loss>,
) -> Result<JsonValue, ExportError> {
    let mut encoded = Map::new();
    encoded.insert("name".into(), json!(material.name));

    let (base_factor, base_texture) = encode_base_color(material, options.texture_tier, registry, losses)?;
    let (metallic_factor, metallic_texture) = scalar_parts(&material.surface.base_metalness);
    let (roughness_factor, roughness_texture) = scalar_parts(&material.surface.specular_roughness);
    let packed_metallic_roughness = pack_metallic_roughness(
        material,
        metallic_texture,
        roughness_texture,
        options.texture_tier,
        registry,
        losses,
    )?;
    let mut pbr = Map::from_iter([
        ("baseColorFactor".into(), json!(base_factor)),
        ("metallicFactor".into(), json!(metallic_factor)),
        ("roughnessFactor".into(), json!(roughness_factor)),
    ]);
    if let Some(index) = base_texture {
        pbr.insert("baseColorTexture".into(), json!({ "index": index }));
    }
    if let Some(index) = packed_metallic_roughness {
        pbr.insert("metallicRoughnessTexture".into(), json!({ "index": index }));
    }
    encoded.insert("pbrMetallicRoughness".into(), JsonValue::Object(pbr));

    if let Some(normal) = &material.geometry.normal
        && let Some(texture) = normal.texture()
    {
        let index = register_normal(material, texture, options.texture_tier, registry, losses)?;
        encoded.insert("normalTexture".into(), json!({ "index": index }));
    }
    if let Some(occlusion) = &material.geometry.ambient_occlusion
        && let Some(texture) = occlusion.texture()
    {
        let index = registry.register_scalar(
            material,
            "ambient_occlusion",
            texture,
            Channel::R,
            options.texture_tier,
            losses,
        )?;
        let strength = scalar_parts(occlusion).0;
        encoded.insert(
            "occlusionTexture".into(),
            json!({ "index": index, "strength": strength }),
        );
    }
    let mut emissive_strength = None;
    if let Some(emission) = &material.surface.emission_color {
        let (mut factor, texture) = color3_parts(emission);
        let strength = factor.iter().copied().fold(1.0_f32, f32::max);
        if strength > 1.0 {
            for component in &mut factor {
                *component /= strength;
            }
            emissive_strength = Some(strength);
        }
        encoded.insert("emissiveFactor".into(), json!(factor));
        if let Some(texture) = texture {
            let index = registry.register_existing(texture, options.texture_tier)?;
            encoded.insert("emissiveTexture".into(), json!({ "index": index }));
        }
    }

    let opacity_factor = base_factor[3];
    if opacity_factor < 1.0 || material.geometry.opacity.as_ref().and_then(Value::texture).is_some() {
        encoded.insert("alphaMode".into(), json!("BLEND"));
    }

    let mut extensions = Map::new();
    if let Some(strength) = emissive_strength {
        extensions.insert(
            "KHR_materials_emissive_strength".into(),
            json!({ "emissiveStrength": strength }),
        );
        extensions_used.insert("KHR_materials_emissive_strength");
    }
    if let Some(value) = &material.surface.specular_ior {
        let ior = constant_only(material, "specular_ior", value, 1.5, losses);
        extensions.insert("KHR_materials_ior".into(), json!({ "ior": ior }));
        extensions_used.insert("KHR_materials_ior");
    }
    if let Some(value) = &material.surface.specular_weight {
        let (factor, texture) = scalar_parts(value);
        let mut extension = Map::from_iter([("specularFactor".into(), json!(factor))]);
        if let Some(texture) = texture {
            let index = registry.register_scalar(
                material,
                "specular_weight",
                texture,
                Channel::A,
                options.texture_tier,
                losses,
            )?;
            extension.insert("specularTexture".into(), json!({ "index": index }));
        }
        extensions.insert("KHR_materials_specular".into(), JsonValue::Object(extension));
        extensions_used.insert("KHR_materials_specular");
    }
    if let Some(value) = &material.surface.specular_anisotropy {
        let (factor, texture) = scalar_parts(value);
        let mut extension = Map::from_iter([("anisotropyStrength".into(), json!(factor))]);
        if let Some(texture) = texture {
            let index = registry.register_anisotropy_strength(
                material,
                "specular_anisotropy",
                texture,
                options.texture_tier,
                losses,
            )?;
            extension.insert("anisotropyTexture".into(), json!({ "index": index }));
        }
        extensions.insert("KHR_materials_anisotropy".into(), JsonValue::Object(extension));
        extensions_used.insert("KHR_materials_anisotropy");
    }
    encode_scalar_extension(
        material,
        &mut extensions,
        extensions_used,
        registry,
        losses,
        options.texture_tier,
        "KHR_materials_transmission",
        "transmissionFactor",
        "transmissionTexture",
        "transmission_weight",
        material.surface.transmission_weight.as_ref(),
        Channel::R,
    )?;
    encode_volume(
        material,
        &mut extensions,
        extensions_used,
        registry,
        losses,
        options.texture_tier,
    )?;
    encode_clearcoat(
        material,
        &mut extensions,
        extensions_used,
        registry,
        losses,
        options.texture_tier,
    )?;
    encode_sheen(
        material,
        &mut extensions,
        extensions_used,
        registry,
        losses,
        options.texture_tier,
    )?;
    if !extensions.is_empty() {
        encoded.insert("extensions".into(), JsonValue::Object(extensions));
    }

    if options.include_olsyn_extras {
        encoded.insert(
            "extras".into(),
            json!({
                "olsyn": {
                    "id": material.id,
                    "shading_model": material.model,
                    "tiling": material.tiling,
                    "variants": material.variants,
                    "provenance": material.provenance,
                }
            }),
        );
    }
    Ok(JsonValue::Object(encoded))
}

#[allow(clippy::too_many_arguments)]
fn encode_scalar_extension(
    material: &Material,
    extensions: &mut Map<String, JsonValue>,
    extensions_used: &mut BTreeSet<&'static str>,
    registry: &mut ImageRegistry,
    losses: &mut Vec<Loss>,
    tier: Tier,
    extension_name: &'static str,
    factor_name: &'static str,
    texture_name: &'static str,
    parameter: &'static str,
    value: Option<&Value<f32>>,
    channel: Channel,
) -> Result<(), ExportError> {
    let Some(value) = value else {
        return Ok(());
    };
    let (factor, texture) = scalar_parts(value);
    let mut extension = Map::from_iter([(factor_name.into(), json!(factor))]);
    if let Some(texture) = texture {
        let index = registry.register_scalar(material, parameter, texture, channel, tier, losses)?;
        extension.insert(texture_name.into(), json!({ "index": index }));
    }
    extensions.insert(extension_name.into(), JsonValue::Object(extension));
    extensions_used.insert(extension_name);
    Ok(())
}

fn encode_volume(
    material: &Material,
    extensions: &mut Map<String, JsonValue>,
    extensions_used: &mut BTreeSet<&'static str>,
    registry: &mut ImageRegistry,
    losses: &mut Vec<Loss>,
    tier: Tier,
) -> Result<(), ExportError> {
    let Some(value) = &material.surface.transmission_thickness else {
        return Ok(());
    };
    let (factor_mm, texture) = scalar_parts(value);
    let mut extension = Map::from_iter([("thicknessFactor".into(), json!(factor_mm / 1_000.0))]);
    if let Some(texture) = texture {
        let index = registry.register_scalar(material, "transmission_thickness", texture, Channel::G, tier, losses)?;
        extension.insert("thicknessTexture".into(), json!({ "index": index }));
    }
    extensions.insert("KHR_materials_volume".into(), JsonValue::Object(extension));
    extensions_used.insert("KHR_materials_volume");
    Ok(())
}

fn encode_clearcoat(
    material: &Material,
    extensions: &mut Map<String, JsonValue>,
    extensions_used: &mut BTreeSet<&'static str>,
    registry: &mut ImageRegistry,
    losses: &mut Vec<Loss>,
    tier: Tier,
) -> Result<(), ExportError> {
    if material.surface.coat_weight.is_none() && material.surface.coat_roughness.is_none() {
        return Ok(());
    }
    let mut extension = Map::new();
    if let Some(value) = &material.surface.coat_weight {
        let (factor, texture) = scalar_parts(value);
        extension.insert("clearcoatFactor".into(), json!(factor));
        if let Some(texture) = texture {
            let index = registry.register_scalar(material, "coat_weight", texture, Channel::R, tier, losses)?;
            extension.insert("clearcoatTexture".into(), json!({ "index": index }));
        }
    }
    if let Some(value) = &material.surface.coat_roughness {
        let (factor, texture) = scalar_parts(value);
        extension.insert("clearcoatRoughnessFactor".into(), json!(factor));
        if let Some(texture) = texture {
            let index = registry.register_scalar(material, "coat_roughness", texture, Channel::G, tier, losses)?;
            extension.insert("clearcoatRoughnessTexture".into(), json!({ "index": index }));
        }
    }
    extensions.insert("KHR_materials_clearcoat".into(), JsonValue::Object(extension));
    extensions_used.insert("KHR_materials_clearcoat");
    Ok(())
}

fn encode_sheen(
    material: &Material,
    extensions: &mut Map<String, JsonValue>,
    extensions_used: &mut BTreeSet<&'static str>,
    registry: &mut ImageRegistry,
    losses: &mut Vec<Loss>,
    tier: Tier,
) -> Result<(), ExportError> {
    if material.surface.fuzz_weight.is_none() && material.surface.fuzz_roughness.is_none() {
        return Ok(());
    }
    let mut extension = Map::new();
    if let Some(value) = &material.surface.fuzz_weight {
        let (factor, texture) = scalar_parts(value);
        extension.insert("sheenColorFactor".into(), json!([factor, factor, factor]));
        if let Some(texture) = texture {
            let index = registry.register_scalar_rgb(material, "fuzz_weight", texture, tier, losses)?;
            extension.insert("sheenColorTexture".into(), json!({ "index": index }));
        }
    }
    if let Some(value) = &material.surface.fuzz_roughness {
        let (factor, texture) = scalar_parts(value);
        extension.insert("sheenRoughnessFactor".into(), json!(factor));
        if let Some(texture) = texture {
            let index = registry.register_scalar(material, "fuzz_roughness", texture, Channel::A, tier, losses)?;
            extension.insert("sheenRoughnessTexture".into(), json!({ "index": index }));
        }
    }
    extensions.insert("KHR_materials_sheen".into(), JsonValue::Object(extension));
    extensions_used.insert("KHR_materials_sheen");
    Ok(())
}

fn encode_base_color(
    material: &Material,
    tier: Tier,
    registry: &mut ImageRegistry,
    losses: &mut Vec<Loss>,
) -> Result<(Color4, Option<u32>), ExportError> {
    let (mut factor, base_texture) = color4_parts(&material.surface.base_color);
    let (opacity_factor, opacity_texture) = material.geometry.opacity.as_ref().map_or((1.0, None), scalar_parts);
    factor[3] *= opacity_factor;
    match (base_texture, opacity_texture) {
        (None, None) => Ok((factor, None)),
        (Some(base), None) => Ok((factor, Some(registry.register_existing(base, tier)?))),
        (base, Some(opacity)) => {
            let base_image = base.map(|texture| decode_texture(texture, tier)).transpose()?;
            let opacity_image = decode_texture(opacity, tier)?;
            let (width, height) = base_image
                .as_ref()
                .map_or_else(|| opacity_image.dimensions(), RgbaImage::dimensions);
            let mut combined = base_image.unwrap_or_else(|| RgbaImage::from_pixel(width, height, Rgba([255; 4])));
            let opacity_image = resize_if_needed(opacity_image, width, height, material, "geometry_opacity", losses);
            for (target, opacity_pixel) in combined.pixels_mut().zip(opacity_image.pixels()) {
                target.0[3] = select_channel(opacity_pixel, opacity.channel);
            }
            let index = registry.register_png(combined)?;
            losses.push(converted_loss(
                material,
                "geometry_opacity",
                "packed the opacity map into base-color alpha for glTF",
            ));
            Ok((factor, Some(index)))
        }
    }
}

fn pack_metallic_roughness(
    material: &Material,
    metallic: Option<&TextureRef>,
    roughness: Option<&TextureRef>,
    tier: Tier,
    registry: &mut ImageRegistry,
    losses: &mut Vec<Loss>,
) -> Result<Option<u32>, ExportError> {
    let Some((first, _second)) = metallic
        .map(|value| (value, roughness))
        .or_else(|| roughness.map(|value| (value, metallic)))
    else {
        return Ok(None);
    };
    if let (Some(metallic), Some(roughness)) = (metallic, roughness) {
        let metallic_source = metallic.source_for(tier);
        let roughness_source = roughness.source_for(tier);
        if metallic_source.map(|source| &source.hash) == roughness_source.map(|source| &source.hash)
            && metallic.channel == Channel::B
            && roughness.channel == Channel::G
        {
            return registry.register_existing(metallic, tier).map(Some);
        }
    }

    let first_image = decode_texture(first, tier)?;
    let (width, height) = first_image.dimensions();
    let metallic_image = metallic
        .map(|texture| decode_texture(texture, tier))
        .transpose()?
        .map(|image| resize_if_needed(image, width, height, material, "base_metalness", losses));
    let roughness_image = roughness
        .map(|texture| decode_texture(texture, tier))
        .transpose()?
        .map(|image| resize_if_needed(image, width, height, material, "specular_roughness", losses));
    let mut packed = RgbaImage::from_pixel(width, height, Rgba([255; 4]));
    for (index, target) in packed.pixels_mut().enumerate() {
        let x = u32::try_from(index).unwrap_or(0) % width;
        let y = u32::try_from(index).unwrap_or(0) / width;
        if let (Some(texture), Some(image)) = (roughness, roughness_image.as_ref()) {
            target.0[1] = select_channel(image.get_pixel(x, y), texture.channel);
        }
        if let (Some(texture), Some(image)) = (metallic, metallic_image.as_ref()) {
            target.0[2] = select_channel(image.get_pixel(x, y), texture.channel);
        }
    }
    let index = registry.register_png(packed)?;
    losses.push(converted_loss(
        material,
        "metallic_roughness",
        "packed roughness into green and metalness into blue for glTF",
    ));
    Ok(Some(index))
}

fn register_normal(
    material: &Material,
    texture: &TextureRef,
    tier: Tier,
    registry: &mut ImageRegistry,
    losses: &mut Vec<Loss>,
) -> Result<u32, ExportError> {
    if texture.normal_convention == Some(NormalConvention::DirectX) {
        let mut image = decode_texture(texture, tier)?;
        for pixel in image.pixels_mut() {
            pixel.0[1] = 255 - pixel.0[1];
        }
        losses.push(converted_loss(
            material,
            "geometry_normal",
            "flipped the DirectX normal-map green channel to glTF/OpenGL convention",
        ));
        registry.register_png(image)
    } else {
        registry.register_existing(texture, tier)
    }
}

#[derive(Default)]
struct ImageRegistry {
    images: Vec<ImageEntry>,
    by_digest: BTreeMap<String, u32>,
    binary: Vec<u8>,
}

struct ImageEntry {
    bytes: Vec<u8>,
    mime_type: String,
}

impl ImageRegistry {
    fn register_existing(&mut self, texture: &TextureRef, tier: Tier) -> Result<u32, ExportError> {
        let source = texture
            .source_for(tier)
            .ok_or_else(|| ExportError::InvalidModel(format!("texture `{}` has no tiers", texture.role)))?;
        self.register(source.bytes.clone(), source.media_type.clone())
    }

    fn register_png(&mut self, image: RgbaImage) -> Result<u32, ExportError> {
        self.register(encode_png(&image)?, "image/png".into())
    }

    fn register(&mut self, bytes: Vec<u8>, mime_type: String) -> Result<u32, ExportError> {
        if !matches!(mime_type.as_str(), "image/png" | "image/jpeg") {
            return Err(ExportError::Unsupported(format!(
                "glTF core image media type `{mime_type}`"
            )));
        }
        let digest = format!("{:x}", Sha256::digest(&bytes));
        if let Some(index) = self.by_digest.get(&digest) {
            return Ok(*index);
        }
        let index = u32::try_from(self.images.len())
            .map_err(|_| ExportError::InvalidModel("more than u32::MAX images".into()))?;
        self.images.push(ImageEntry { bytes, mime_type });
        self.by_digest.insert(digest, index);
        Ok(index)
    }

    fn register_scalar(
        &mut self,
        material: &Material,
        parameter: &'static str,
        texture: &TextureRef,
        destination: Channel,
        tier: Tier,
        losses: &mut Vec<Loss>,
    ) -> Result<u32, ExportError> {
        if texture.channel == destination {
            return self.register_existing(texture, tier);
        }
        let source = decode_texture(texture, tier)?;
        let mut output = RgbaImage::from_pixel(source.width(), source.height(), Rgba([255; 4]));
        for (target, source) in output.pixels_mut().zip(source.pixels()) {
            let value = select_channel(source, texture.channel);
            match destination {
                Channel::R => target.0[0] = value,
                Channel::G => target.0[1] = value,
                Channel::B => target.0[2] = value,
                Channel::A => target.0[3] = value,
                Channel::Rgb => target.0[..3].fill(value),
            }
        }
        losses.push(converted_loss(
            material,
            parameter,
            &format!(
                "moved source channel {:?} to glTF channel {destination:?}",
                texture.channel
            ),
        ));
        self.register_png(output)
    }

    fn register_scalar_rgb(
        &mut self,
        material: &Material,
        parameter: &'static str,
        texture: &TextureRef,
        tier: Tier,
        losses: &mut Vec<Loss>,
    ) -> Result<u32, ExportError> {
        self.register_scalar(material, parameter, texture, Channel::Rgb, tier, losses)
    }

    fn register_anisotropy_strength(
        &mut self,
        material: &Material,
        parameter: &'static str,
        texture: &TextureRef,
        tier: Tier,
        losses: &mut Vec<Loss>,
    ) -> Result<u32, ExportError> {
        if texture.channel == Channel::B {
            return self.register_existing(texture, tier);
        }
        let source = decode_texture(texture, tier)?;
        // The Khronos default direction is +X, encoded as (1.0, 0.5).
        let mut output = RgbaImage::from_pixel(source.width(), source.height(), Rgba([255, 128, 255, 255]));
        for (target, source) in output.pixels_mut().zip(source.pixels()) {
            target.0[2] = select_channel(source, texture.channel);
        }
        losses.push(converted_loss(
            material,
            parameter,
            &format!(
                "moved source channel {:?} to glTF blue with the default +X anisotropy direction",
                texture.channel
            ),
        ));
        self.register_png(output)
    }

    fn add_to_document(&mut self, root: &mut Map<String, JsonValue>, format: GltfFormat) {
        if self.images.is_empty() {
            return;
        }
        root.insert(
            "samplers".into(),
            json!([{ "magFilter": 9729, "minFilter": 9987, "wrapS": 10497, "wrapT": 10497 }]),
        );
        root.insert(
            "textures".into(),
            JsonValue::Array(
                (0..self.images.len())
                    .map(|index| json!({ "sampler": 0, "source": index }))
                    .collect(),
            ),
        );
        match format {
            GltfFormat::Gltf => {
                root.insert(
                    "images".into(),
                    JsonValue::Array(
                        self.images
                            .iter()
                            .map(|image| {
                                json!({
                                    "uri": format!("data:{};base64,{}", image.mime_type, BASE64.encode(&image.bytes))
                                })
                            })
                            .collect(),
                    ),
                );
            }
            GltfFormat::Glb => {
                let mut images = Vec::new();
                let mut views = Vec::new();
                for image in &self.images {
                    while !self.binary.len().is_multiple_of(4) {
                        self.binary.push(0);
                    }
                    let offset = self.binary.len();
                    self.binary.extend_from_slice(&image.bytes);
                    images.push(json!({ "bufferView": views.len(), "mimeType": image.mime_type }));
                    views.push(json!({
                        "buffer": 0,
                        "byteOffset": offset,
                        "byteLength": image.bytes.len(),
                    }));
                }
                root.insert("images".into(), JsonValue::Array(images));
                root.insert("bufferViews".into(), JsonValue::Array(views));
                root.insert("buffers".into(), json!([{ "byteLength": self.binary.len() }]));
            }
        }
    }
}

fn scalar_parts(value: &Value<f32>) -> (f32, Option<&TextureRef>) {
    match value {
        Value::Constant { value } => (*value, None),
        Value::Texture { texture } => (1.0, Some(texture)),
        Value::Modulated { texture, factor } => (*factor, Some(texture)),
    }
}

fn color3_parts(value: &Value<Color3>) -> (Color3, Option<&TextureRef>) {
    match value {
        Value::Constant { value } => (*value, None),
        Value::Texture { texture } => ([1.0; 3], Some(texture)),
        Value::Modulated { texture, factor } => (*factor, Some(texture)),
    }
}

fn color4_parts(value: &Value<Color4>) -> (Color4, Option<&TextureRef>) {
    match value {
        Value::Constant { value } => (*value, None),
        Value::Texture { texture } => ([1.0; 4], Some(texture)),
        Value::Modulated { texture, factor } => (*factor, Some(texture)),
    }
}

fn constant_only(
    material: &Material,
    parameter: &'static str,
    value: &Value<f32>,
    default: f32,
    losses: &mut Vec<Loss>,
) -> f32 {
    match value {
        Value::Constant { value } => *value,
        Value::Modulated { factor, .. } => {
            losses.push(Loss {
                material: material.id.clone(),
                parameter,
                kind: LossKind::Dropped,
                detail: format!("glTF represents `{parameter}` as a constant; its texture was omitted"),
            });
            *factor
        }
        Value::Texture { .. } => {
            losses.push(Loss {
                material: material.id.clone(),
                parameter,
                kind: LossKind::Dropped,
                detail: format!("glTF represents `{parameter}` as a constant; its texture was omitted"),
            });
            default
        }
    }
}

fn decode_texture(texture: &TextureRef, tier: Tier) -> Result<RgbaImage, ExportError> {
    let source = texture
        .source_for(tier)
        .ok_or_else(|| ExportError::InvalidModel(format!("texture `{}` has no tiers", texture.role)))?;
    image::load_from_memory(&source.bytes)
        .map(|image| image.to_rgba8())
        .map_err(|error| ExportError::Encode {
            format: "glTF image",
            detail: format!("{}: {error}", source.package_path()),
        })
}

fn resize_if_needed(
    image: RgbaImage,
    width: u32,
    height: u32,
    material: &Material,
    parameter: &'static str,
    losses: &mut Vec<Loss>,
) -> RgbaImage {
    if image.dimensions() == (width, height) {
        image
    } else {
        losses.push(Loss {
            material: material.id.clone(),
            parameter,
            kind: LossKind::Reduced,
            detail: format!("resampled texture to {width}x{height} while packing glTF channels"),
        });
        image::imageops::resize(&image, width, height, FilterType::Lanczos3)
    }
}

fn select_channel(pixel: &Rgba<u8>, channel: Channel) -> u8 {
    match channel {
        Channel::Rgb | Channel::R => pixel.0[0],
        Channel::G => pixel.0[1],
        Channel::B => pixel.0[2],
        Channel::A => pixel.0[3],
    }
}

fn encode_png(image: &RgbaImage) -> Result<Vec<u8>, ExportError> {
    let mut cursor = Cursor::new(Vec::new());
    DynamicImage::ImageRgba8(image.clone())
        .write_to(&mut cursor, ImageFormat::Png)
        .map_err(|error| ExportError::Encode {
            format: "glTF image",
            detail: error.to_string(),
        })?;
    Ok(cursor.into_inner())
}

fn write_glb(document: &JsonValue, binary: &[u8]) -> Result<Vec<u8>, ExportError> {
    let mut json_bytes = serde_json::to_vec(document).map_err(json_export_error)?;
    while !json_bytes.len().is_multiple_of(4) {
        json_bytes.push(b' ');
    }
    let mut binary = binary.to_vec();
    while !binary.len().is_multiple_of(4) {
        binary.push(0);
    }
    let total_length = 12_usize
        .checked_add(8 + json_bytes.len())
        .and_then(|length| length.checked_add(if binary.is_empty() { 0 } else { 8 + binary.len() }))
        .ok_or_else(|| ExportError::Encode {
            format: "GLB",
            detail: "output size overflow".into(),
        })?;
    let mut out = Vec::with_capacity(total_length);
    out.extend_from_slice(b"glTF");
    out.extend_from_slice(&2_u32.to_le_bytes());
    out.extend_from_slice(
        &u32::try_from(total_length)
            .map_err(|_| ExportError::Encode {
                format: "GLB",
                detail: "GLB exceeds 4 GiB".into(),
            })?
            .to_le_bytes(),
    );
    write_chunk(&mut out, &json_bytes, *b"JSON")?;
    if !binary.is_empty() {
        write_chunk(&mut out, &binary, *b"BIN\0")?;
    }
    Ok(out)
}

fn write_chunk(out: &mut Vec<u8>, bytes: &[u8], kind: [u8; 4]) -> Result<(), ExportError> {
    out.extend_from_slice(
        &u32::try_from(bytes.len())
            .map_err(|_| ExportError::Encode {
                format: "GLB",
                detail: "chunk exceeds 4 GiB".into(),
            })?
            .to_le_bytes(),
    );
    out.extend_from_slice(&kind);
    out.extend_from_slice(bytes);
    Ok(())
}

fn converted_loss(material: &Material, parameter: &'static str, detail: &str) -> Loss {
    Loss {
        material: material.id.clone(),
        parameter,
        kind: LossKind::Converted,
        detail: detail.into(),
    }
}

fn sort_losses(losses: &mut Vec<Loss>) {
    losses.sort_by(|left, right| {
        left.material
            .cmp(&right.material)
            .then_with(|| left.parameter.cmp(right.parameter))
            .then_with(|| left.detail.cmp(&right.detail))
    });
    losses.dedup();
}

fn json_export_error(error: impl std::fmt::Display) -> ExportError {
    ExportError::Encode {
        format: "glTF",
        detail: error.to_string(),
    }
}

fn validate_payload_hashes(materials: &[Material]) -> Result<(), ExportError> {
    for material in materials {
        let mut mismatch = None;
        material.visit_textures(|parameter, texture| {
            if mismatch.is_none() {
                mismatch = texture
                    .tiers
                    .iter()
                    .find(|(_, source)| format!("{:x}", Sha256::digest(&source.bytes)) != source.hash.0)
                    .map(|(tier, _)| format!("{parameter}.tiers.{}", tier.as_str()));
            }
        });
        if let Some(path) = mismatch {
            return Err(ExportError::InvalidModel(format!(
                "material `{}` has a payload/hash mismatch at {path}",
                material.id
            )));
        }
        for asset in &material.auxiliary {
            if format!("{:x}", Sha256::digest(&asset.bytes)) != asset.hash.0 {
                return Err(ExportError::InvalidModel(format!(
                    "material `{}` has a payload/hash mismatch for auxiliary `{}`",
                    material.id, asset.name
                )));
            }
        }
        for (hash, asset) in &material.provenance.source_assets {
            if format!("{:x}", Sha256::digest(&asset.bytes)) != hash.0 {
                return Err(ExportError::InvalidModel(format!(
                    "material `{}` has a payload/hash mismatch for provenance source `{}`",
                    material.id, asset.name
                )));
            }
        }
    }
    Ok(())
}
