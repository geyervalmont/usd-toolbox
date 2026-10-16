//! Scene preview derivatives. Source materials and their encoded images stay intact.
use super::*;
use image::{ImageDecoder, Rgba32FImage};
use usd_toolbox_core::{ColorSpace, MapRole, TextureSource, scene::*};

#[derive(Default)]
pub(crate) struct TexturePreviews {
    images: BTreeMap<(String, u8), TextureSource>,
    pub(crate) max_bytes: Option<usize>,
    pub(crate) alpha_weighted: bool,
}

// A source can be used as colour, packed scalar data and/or a normal map. A digest
// alone is not a valid cache key: those uses need different filtering arithmetic.
fn interpretation(texture: &TextureRef, alpha_weighted: bool) -> u8 {
    if texture.role == MapRole::Normal {
        4
    } else {
        u8::from(texture.color_space == ColorSpace::Srgb)
            | (u8::from(alpha_weighted && texture.role == MapRole::BaseColor && texture.channel == Channel::Rgb) << 1)
    }
}

pub(crate) fn uses_alpha(scene: &Scene, material: &Material) -> bool {
    // Separate opacity maps replace base-image alpha during channel packing.
    if material.geometry.opacity.as_ref().and_then(Value::texture).is_some() {
        return false;
    }
    if let Some(settings) = scene
        .metadata
        .materials
        .get(&material.id.0)
        .and_then(|m| m.original.get("gltf:render_settings"))
        .and_then(MetadataProperty::resolved)
    {
        return matches!(settings.to_json()["alpha_mode"].as_str(), Some("MASK" | "BLEND"));
    }
    let alpha = color4_parts(&material.surface.base_color).0[3]
        * material
            .geometry
            .opacity
            .as_ref()
            .map_or(1., |value| scalar_parts(value).0);
    alpha < 1.
}

fn image_error(path: &str, detail: impl std::fmt::Display) -> SceneError {
    SceneError::new("gltf.image", path, detail.to_string())
}

fn budget(path: &str, detail: &str) -> SceneError {
    SceneError::new("scene.limit", path, detail)
}

impl TexturePreviews {
    pub(crate) fn source<'a>(&'a self, texture: &'a TextureRef, tier: Tier) -> Result<&'a TextureSource, ExportError> {
        let source = texture
            .source_for(tier)
            .ok_or_else(|| ExportError::InvalidModel(format!("texture `{}` has no tiers", texture.role)))?;
        Ok(self
            .images
            .get(&(source.hash.0.clone(), interpretation(texture, self.alpha_weighted)))
            .unwrap_or(source))
    }

    pub(crate) fn prepare(
        scene: &Scene,
        options: &scene::SceneGltfOptions,
    ) -> Result<(Self, Vec<SceneDiagnostic>), SceneError> {
        let max_bytes = options.limits.max_bytes;
        let mut previews = Self {
            max_bytes: Some(max_bytes),
            ..Default::default()
        };
        let mut diagnostics = Vec::new();
        let mut seen = BTreeMap::new();
        let mut decoded_bytes = 0usize;
        let mut derivative_bytes = 0usize;
        for material in &scene.materials {
            let mut result = Ok(());
            material.visit_textures(|parameter, texture| {
                if result.is_err() {
                    return;
                }
                result = (|| {
                    let Some(source) = texture.source_for(options.texture_tier) else { return Ok(()) };
                    let path = format!("{}/{parameter}", material.id);
                    let (width, height, decoded_size) = if let Some(&dimensions) = seen.get(&source.hash) {
                        dimensions
                    } else {
                        let format = image::guess_format(&source.bytes).map_err(|e| image_error(&path, e))?;
                        if !matches!(format, ImageFormat::Png | ImageFormat::Jpeg) {
                            return Err(SceneError::new("gltf.unsupported", &path, "scene preview images support only PNG and JPEG; EXR, HDR, TIFF and tiled/UDIM assembly require an explicit source conversion"));
                        }
                        let expected_mime = if format == ImageFormat::Png { "image/png" } else { "image/jpeg" };
                        if source.media_type != expected_mime {
                            return Err(image_error(&path, "image media type differs from encoded PNG/JPEG payload"));
                        }
                        let mut reader = image::ImageReader::with_format(Cursor::new(&source.bytes), format);
                        let mut limits = image::Limits::default();
                        limits.max_alloc = Some(max_bytes as u64);
                        reader.limits(limits);
                        let decoder = reader.into_decoder().map_err(|e| image_error(&path, e))?;
                        let (width, height) = decoder.dimensions();
                        // A PNG can carry 16-bit samples; width*height*4 alone
                        // would undercount its decoder output.
                        let decoded_size = usize::try_from(decoder.total_bytes()).unwrap_or(usize::MAX)
                            .max((width as usize).saturating_mul(height as usize).saturating_mul(4));
                        decoded_bytes = decoded_bytes.saturating_add(decoded_size);
                        if width == 0 || height == 0 || decoded_bytes > max_bytes {
                            return Err(budget(&path, "decoded texture pixels exceed byte budget"));
                        }
                        seen.insert(source.hash.clone(), (width, height, decoded_size));
                        (width, height, decoded_size)
                    };
                    let cap = options.texture_tier.pixels();
                    if width.max(height) <= cap {
                        return Ok(());
                    }
                    let longest = u64::from(width.max(height));
                    let size = |n: u32| ((u64::from(n) * u64::from(cap) + longest / 2) / longest).max(1) as u32;
                    let (out_width, out_height) = (size(width), size(height));
                    let mode = interpretation(texture, uses_alpha(scene, material));
                    let key = (source.hash.0.clone(), mode);
                    if let std::collections::btree_map::Entry::Vacant(entry) = previews.images.entry(key) {
                        let source_pixels = (width as usize).saturating_mul(height as usize);
                        let output_bytes = (out_width as usize).saturating_mul(out_height as usize).saturating_mul(4);
                        // Float source pixels, codec output, output pixels, PNG
                        // writer clone and retained encoded derivatives. Codec
                        // scratch is separately constrained by image::Limits.
                        let work = source_pixels.saturating_mul(16)
                            .saturating_add(decoded_size)
                            .saturating_add(output_bytes.saturating_mul(2))
                            .saturating_add(derivative_bytes);
                        if work > max_bytes {
                            return Err(budget(&path, "preview texture filter working buffers exceed byte budget"));
                        }
                        let mut reader = image::ImageReader::new(Cursor::new(&source.bytes))
                            .with_guessed_format().map_err(|e| image_error(&path, e))?;
                        let mut limits = image::Limits::default();
                        limits.max_alloc = Some(max_bytes as u64);
                        limits.max_image_width = Some(width);
                        limits.max_image_height = Some(height);
                        reader.limits(limits);
                        let decoded = reader.decode().map_err(|e| image_error(&path, e))?;
                        let pixels = decoded.into_rgba32f();
                        let output = downsample(pixels, out_width, out_height, mode);
                        let bytes = encode_png(&output).map_err(|e| image_error(&path, e))?;
                        derivative_bytes = derivative_bytes.saturating_add(bytes.len());
                        if derivative_bytes > max_bytes {
                            return Err(budget(&path, "encoded preview textures exceed byte budget"));
                        }
                        entry.insert(TextureSource {
                            hash: usd_toolbox_core::Sha256(format!("{:x}", Sha256::digest(&bytes))),
                            extension: "png".into(), media_type: "image/png".into(),
                            width: Some(out_width), height: Some(out_height), bytes,
                        });
                    }
                    let filtering = match mode {
                        4 => "linear signed vectors, normalized after area filtering",
                        3 => "linear-light sRGB, alpha-weighted colour and linear alpha",
                        1 => "linear-light sRGB and independent linear alpha",
                        2 => "linear samples, alpha-weighted colour and linear alpha",
                        _ => "independent linear data channels",
                    };
                    diagnostics.push(SceneDiagnostic::warning("gltf.texture_downsampled", &path,
                        format!("{} tier: {width}x{height} -> {out_width}x{out_height}; {filtering}; encoded as RGBA8 PNG; source retained", options.texture_tier.as_str())));
                    Ok(())
                })();
            });
            result?;
        }
        Ok((previews, diagnostics))
    }
}

fn srgb_to_linear(v: f32) -> f32 {
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}
fn linear_to_srgb(v: f32) -> f32 {
    if v <= 0.0031308 {
        v * 12.92
    } else {
        1.055 * v.powf(1. / 2.4) - 0.055
    }
}

// Exact pixel-area averaging avoids ringing in data maps and gives deterministic
// minification for non-power-of-two dimensions without a large filter scratch
// image. Alpha weighting prevents dark transparent texels bleeding into colour.
fn downsample(mut source: Rgba32FImage, width: u32, height: u32, mode: u8) -> RgbaImage {
    for pixel in source.pixels_mut() {
        let alpha = pixel.0[3];
        for channel in &mut pixel.0[..3] {
            if mode == 4 {
                *channel = *channel * 2. - 1.;
            } else {
                if mode & 1 != 0 {
                    *channel = srgb_to_linear(*channel);
                }
                if mode & 2 != 0 {
                    *channel *= alpha;
                }
            }
        }
    }
    let scale_x = f64::from(source.width()) / f64::from(width);
    let scale_y = f64::from(source.height()) / f64::from(height);
    RgbaImage::from_fn(width, height, |x, y| {
        let (left, right) = (f64::from(x) * scale_x, f64::from(x + 1) * scale_x);
        let (top, bottom) = (f64::from(y) * scale_y, f64::from(y + 1) * scale_y);
        let mut sum = [0.; 4];
        for sy in top.floor() as u32..(bottom.ceil() as u32).min(source.height()) {
            let dy = (bottom.min(f64::from(sy + 1)) - top.max(f64::from(sy))).max(0.);
            for sx in left.floor() as u32..(right.ceil() as u32).min(source.width()) {
                let weight = dy * (right.min(f64::from(sx + 1)) - left.max(f64::from(sx))).max(0.);
                for (sum, value) in sum.iter_mut().zip(source.get_pixel(sx, sy).0) {
                    *sum += f64::from(value) * weight;
                }
            }
        }
        let mut value = sum.map(|v| (v / (scale_x * scale_y)) as f32);
        if mode == 4 {
            let length = value[..3].iter().map(|v| v * v).sum::<f32>().sqrt();
            let vector = if length > 1e-6 {
                [value[0] / length, value[1] / length, value[2] / length]
            } else {
                [0., 0., 1.]
            };
            for (component, normal) in value[..3].iter_mut().zip(vector) {
                *component = normal * 0.5 + 0.5;
            }
        } else {
            let alpha = value[3];
            for channel in &mut value[..3] {
                if mode & 2 != 0 {
                    *channel = if alpha > 1e-8 { *channel / alpha } else { 0. };
                }
                if mode & 1 != 0 {
                    *channel = linear_to_srgb(*channel);
                }
            }
        }
        Rgba(value.map(|v| (v.clamp(0., 1.) * 255.).round() as u8))
    })
}
