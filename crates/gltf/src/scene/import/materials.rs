use super::*;
use image::ImageDecoder;

pub(in crate::scene) const TEXTURE_SLOTS: &[(&str, &str)] = &[
    ("base_color", "/pbrMetallicRoughness/baseColorTexture"),
    ("metallic_roughness", "/pbrMetallicRoughness/metallicRoughnessTexture"),
    ("normal", "/normalTexture"),
    ("occlusion", "/occlusionTexture"),
    ("emissive", "/emissiveTexture"),
    (
        "transmission",
        "/extensions/KHR_materials_transmission/transmissionTexture",
    ),
    ("clearcoat", "/extensions/KHR_materials_clearcoat/clearcoatTexture"),
    (
        "clearcoat_roughness",
        "/extensions/KHR_materials_clearcoat/clearcoatRoughnessTexture",
    ),
];
fn scalar(value: &JsonValue, key: &str, default: f32, min: f32, max: f32) -> Result<f32, SceneError> {
    let result = value
        .get(key)
        .map_or(Some(f64::from(default)), JsonValue::as_f64)
        .ok_or_else(|| invalid(key, "expected number"))?;
    if !result.is_finite() || result < f64::from(min) || result > f64::from(max) {
        return Err(invalid(key, "number exceeds allowed range"));
    }
    Ok(result as f32)
}
impl Reader<'_> {
    fn texture(
        &mut self,
        info: &JsonValue,
        role: MapRole,
        color_space: ColorSpace,
        channel: Channel,
    ) -> Result<TextureRef, SceneError> {
        let texture_index = required(info, "index")?;
        let texture = array(self.document, "textures")?
            .get(texture_index)
            .ok_or_else(|| invalid("textures", "texture index is out of bounds"))?;
        let image_index = required(texture, "source")?;
        if optional(info, "texCoord", 0)? > 31 {
            return Err(limit("texCoord"));
        }
        if let Some(sampler) = texture.get("sampler") {
            let sampler = array(self.document, "samplers")?
                .get(index(sampler, "sampler")?)
                .ok_or_else(|| invalid("samplers", "sampler index is out of bounds"))?;
            for key in ["wrapS", "wrapT"] {
                if !matches!(optional(sampler, key, 10497)?, 33071 | 33648 | 10497) {
                    return Err(invalid(key, "invalid sampler wrapping mode"));
                }
            }
            if sampler.get("magFilter").is_some() && !matches!(required(sampler, "magFilter")?, 9728 | 9729) {
                return Err(invalid("magFilter", "invalid magnification filter"));
            }
            if sampler.get("minFilter").is_some()
                && !matches!(required(sampler, "minFilter")?, 9728 | 9729 | 9984..=9987)
            {
                return Err(invalid("minFilter", "invalid minification filter"));
            }
        }
        if let Some(transform) = info.pointer("/extensions/KHR_texture_transform") {
            numbers(transform.get("offset"), [0., 0.], "texture_transform/offset")?;
            numbers(transform.get("scale"), [1., 1.], "texture_transform/scale")?;
            scalar(transform, "rotation", 0., -f32::MAX, f32::MAX)?;
            if optional(transform, "texCoord", 0)? > 31 {
                return Err(limit("texture_transform/texCoord"));
            }
        }
        if !self.images.contains_key(&image_index) {
            let image = array(self.document, "images")?
                .get(image_index)
                .ok_or_else(|| invalid("images", "image index is out of bounds"))?;
            let bytes = match (image.get("uri"), image.get("bufferView")) {
                (Some(uri), None) => self.resource(
                    uri.as_str()
                        .ok_or_else(|| invalid("image/uri", "URI must be a string"))?,
                )?,
                (None, Some(view)) => {
                    let (bytes, view) = self.view(index(view, "image/bufferView")?)?;
                    if view.get("byteStride").is_some() || view.get("target").is_some() {
                        return Err(invalid("image/bufferView", "image views cannot have target or stride"));
                    }
                    Cow::Owned(bytes.to_vec())
                }
                _ => return Err(invalid("image", "image must define exactly one URI or bufferView")),
            };
            let format = image::guess_format(&bytes).map_err(|e| invalid("image", e.to_string()))?;
            let (extension, mime) = match format {
                ImageFormat::Png => ("png", "image/png"),
                ImageFormat::Jpeg => ("jpg", "image/jpeg"),
                _ => return Err(unsupported("image", "only PNG and JPEG textures are supported")),
            };
            if image.get("mimeType").is_some_and(|v| v != mime) {
                return Err(invalid("image/mimeType", "MIME type differs from encoded image format"));
            }
            if image.get("bufferView").is_some() && image.get("mimeType").is_none() {
                return Err(invalid("image/mimeType", "buffer-view images require MIME type"));
            }
            let mut header_reader = image::ImageReader::with_format(Cursor::new(&bytes), format);
            let mut header_limits = image::Limits::default();
            header_limits.max_alloc = Some(self.limits.max_bytes as u64);
            header_reader.limits(header_limits);
            let decoder = header_reader
                .into_decoder()
                .map_err(|e| invalid("image", e.to_string()))?;
            let (width, height) = decoder.dimensions();
            let decoded_bytes = usize::try_from(decoder.total_bytes())
                .unwrap_or(usize::MAX)
                .max((width as usize).saturating_mul(height as usize).saturating_mul(4));
            drop(decoder);
            self.image_bytes = self.image_bytes.saturating_add(decoded_bytes);
            if width == 0 || height == 0 || self.image_bytes > self.limits.max_bytes {
                return Err(limit("images/decoded_pixels"));
            }
            // Import validates encoded image payloads once. Preflight deliberately
            // skips this work. Decoder scratch is separately bounded on a best-
            // effort basis by image's portable codec limits.
            let mut decode_limits = image::Limits::default();
            decode_limits.max_alloc = Some(self.limits.max_bytes as u64);
            decode_limits.max_image_width = Some(width);
            decode_limits.max_image_height = Some(height);
            let mut decoder = image::ImageReader::with_format(Cursor::new(&bytes), format);
            decoder.limits(decode_limits);
            let decoded = decoder.decode().map_err(|e| invalid("image/payload", e.to_string()))?;
            if decoded.as_bytes().len() > self.limits.max_bytes {
                return Err(limit("images/decoded_pixels"));
            }
            drop(decoded);
            self.images.insert(
                image_index,
                TextureSource {
                    hash: usd_toolbox_core::Sha256(format!("{:x}", Sha256::digest(&bytes))),
                    extension: extension.into(),
                    media_type: mime.into(),
                    width: Some(width),
                    height: Some(height),
                    bytes: bytes.into_owned(),
                },
            );
        }
        let source = self.images[&image_index].clone();
        self.charge(source.bytes.len(), "material/textures")?;
        let tier = match source.width.unwrap().max(source.height.unwrap()) {
            0..=512 => Tier::Preview,
            513..=1024 => Tier::K1,
            1025..=2048 => Tier::K2,
            2049..=4096 => Tier::K4,
            _ => Tier::K8,
        };
        Ok(TextureRef {
            role,
            tiers: BTreeMap::from([(tier, source)]),
            color_space,
            channel,
            normal_convention: (role == MapRole::Normal).then_some(NormalConvention::OpenGl),
        })
    }
    fn scalar_value(
        &mut self,
        value: &JsonValue,
        key: &str,
        texture_key: &str,
        default: f32,
        role: MapRole,
        channel: Channel,
    ) -> Result<Value<f32>, SceneError> {
        let factor = scalar(value, key, default, 0., 1.)?;
        Ok(if let Some(info) = value.get(texture_key) {
            Value::Modulated {
                texture: self.texture(info, role, ColorSpace::Raw, channel)?,
                factor,
            }
        } else {
            Value::Constant { value: factor }
        })
    }
    pub(super) fn material(
        &mut self,
        i: usize,
        value: &JsonValue,
        diagnostics: &mut Vec<SceneDiagnostic>,
    ) -> Result<Material, SceneError> {
        if !value.is_object() {
            return Err(invalid(format!("materials/{i}"), "material must be an object"));
        }
        if value.get("pbrMetallicRoughness").is_some_and(|v| !v.is_object()) {
            return Err(invalid("pbrMetallicRoughness", "PBR settings must be an object"));
        }
        if value.get("alphaMode").is_some_and(|v| !v.is_string()) {
            return Err(invalid("alphaMode", "alpha mode must be a string"));
        }
        let id = value
            .pointer("/extras/olsyn/id")
            .and_then(JsonValue::as_str)
            .map_or_else(|| format!("gltf/materials/{i}"), str::to_owned);
        let mut material = Material::new(
            id,
            value["name"]
                .as_str()
                .map_or_else(|| format!("Material {i}"), str::to_owned),
        );
        material.model = ShadingModel::GltfPbr;
        let pbr = &value["pbrMetallicRoughness"];
        let base = numbers(pbr.get("baseColorFactor"), [1.; 4], "baseColorFactor")?.map(|v| v as f32);
        if base.iter().any(|v| !(0. ..=1.).contains(v)) {
            return Err(invalid("baseColorFactor", "base factors must be within zero and one"));
        }
        material.surface.base_color = if let Some(info) = pbr.get("baseColorTexture") {
            Value::Modulated {
                texture: self.texture(info, MapRole::BaseColor, ColorSpace::Srgb, Channel::Rgb)?,
                factor: base,
            }
        } else {
            base.into()
        };
        material.surface.base_metalness = self.scalar_value(
            pbr,
            "metallicFactor",
            "metallicRoughnessTexture",
            1.,
            MapRole::Metallic,
            Channel::B,
        )?;
        material.surface.specular_roughness = self.scalar_value(
            pbr,
            "roughnessFactor",
            "metallicRoughnessTexture",
            1.,
            MapRole::Roughness,
            Channel::G,
        )?;
        if let Some(info) = value.get("normalTexture") {
            scalar(info, "scale", 1., -f32::MAX, f32::MAX)?;
            material.geometry.normal = Some(Value::Texture {
                texture: self.texture(info, MapRole::Normal, ColorSpace::Raw, Channel::Rgb)?,
            });
        }
        if let Some(info) = value.get("occlusionTexture") {
            let factor = scalar(info, "strength", 1., 0., 1.)?;
            material.geometry.ambient_occlusion = Some(Value::Modulated {
                texture: self.texture(info, MapRole::Ao, ColorSpace::Raw, Channel::R)?,
                factor,
            });
        }
        let emission = numbers(value.get("emissiveFactor"), [0.; 3], "emissiveFactor")?.map(|v| v as f32);
        if emission.iter().any(|v| !(0. ..=1.).contains(v)) {
            return Err(invalid(
                "emissiveFactor",
                "emission factors must be within zero and one",
            ));
        }
        let strength = scalar(
            &value["extensions"]["KHR_materials_emissive_strength"],
            "emissiveStrength",
            1.,
            0.,
            f32::MAX,
        )?;
        let emission = emission.map(|v| v * strength);
        if let Some(info) = value.get("emissiveTexture") {
            material.surface.emission_color = Some(Value::Modulated {
                texture: self.texture(info, MapRole::Emissive, ColorSpace::Srgb, Channel::Rgb)?,
                factor: emission,
            });
        } else if emission != [0.; 3] {
            material.surface.emission_color = Some(emission.into());
        }
        match value.get("alphaMode").and_then(JsonValue::as_str).unwrap_or("OPAQUE") {
            "OPAQUE" | "BLEND" | "MASK" => {}
            _ => return Err(invalid("alphaMode", "invalid alpha mode")),
        }
        scalar(value, "alphaCutoff", 0.5, 0., f32::MAX)?;
        if let Some(ior) = value.pointer("/extensions/KHR_materials_ior") {
            material.surface.specular_ior = Some(scalar(ior, "ior", 1.5, 0., f32::MAX)?.into());
        }
        if let Some(transmission) = value.pointer("/extensions/KHR_materials_transmission") {
            material.surface.transmission_weight = Some(self.scalar_value(
                transmission,
                "transmissionFactor",
                "transmissionTexture",
                0.,
                MapRole::Transmission,
                Channel::R,
            )?);
        }
        if let Some(coat) = value.pointer("/extensions/KHR_materials_clearcoat") {
            if coat.get("clearcoatNormalTexture").is_some() {
                return Err(unsupported(
                    "clearcoatNormalTexture",
                    "independent clearcoat normal maps have no neutral projection",
                ));
            }
            material.surface.coat_weight = Some(self.scalar_value(
                coat,
                "clearcoatFactor",
                "clearcoatTexture",
                0.,
                MapRole::Specular,
                Channel::R,
            )?);
            material.surface.coat_roughness = Some(self.scalar_value(
                coat,
                "clearcoatRoughnessFactor",
                "clearcoatRoughnessTexture",
                0.,
                MapRole::Roughness,
                Channel::G,
            )?);
        }
        diagnostics.push(SceneDiagnostic::warning("gltf.material_profile",&format!("materials/{i}"),"glTF alpha, sampler, UV-transform and normal-scale settings are retained as typed metadata and restored by scene GLB export; other target exporters must report unsupported settings"));
        Ok(material)
    }
}

pub(super) fn render_settings(doc: &JsonValue, value: &JsonValue) -> Result<JsonValue, SceneError> {
    let mut textures = Map::new();
    for (name, path) in TEXTURE_SLOTS {
        let Some(info) = value.pointer(path) else {
            continue;
        };
        let texture = &array(doc, "textures")?[required(info, "index")?];
        let sampler = if let Some(s) = texture.get("sampler") {
            array(doc, "samplers")?
                .get(index(s, "sampler")?)
                .cloned()
                .unwrap_or_else(|| json!({}))
        } else {
            json!({})
        };
        let mut settings = json!({"tex_coord":optional(info,"texCoord",0)?,"sampler":sampler});
        if let Some(transform) = info.pointer("/extensions/KHR_texture_transform") {
            settings["transform"] = transform.clone();
        }
        if let Some(scale) = info.get("scale") {
            settings["scale"] = scale.clone();
        }
        textures.insert((*name).into(), settings);
    }
    Ok(
        json!({"alpha_mode":value["alphaMode"].as_str().unwrap_or("OPAQUE"),"alpha_cutoff":scalar(value,"alphaCutoff",0.5,0.,f32::MAX)?,"textures":textures}),
    )
}
