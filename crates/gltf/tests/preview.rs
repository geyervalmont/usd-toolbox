use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use image::{DynamicImage, ImageFormat, Rgba, RgbaImage};
use serde_json::{Value as Json, json};
use sha2::Digest;
use std::{collections::BTreeMap, io::Cursor};
use usd_toolbox_core::{scene::*, *};
use usd_toolbox_gltf::{GltfFormat, scene::*};

fn source(image: DynamicImage, format: ImageFormat) -> TextureSource {
    let (width, height) = (image.width(), image.height());
    let mut bytes = Cursor::new(Vec::new());
    image.write_to(&mut bytes, format).unwrap();
    let bytes = bytes.into_inner();
    TextureSource {
        hash: Sha256(format!("{:x}", sha2::Sha256::digest(&bytes))),
        extension: if format == ImageFormat::Png { "png" } else { "jpg" }.into(),
        media_type: if format == ImageFormat::Png {
            "image/png"
        } else {
            "image/jpeg"
        }
        .into(),
        width: Some(width),
        height: Some(height),
        bytes,
    }
}
fn texture(source: TextureSource, role: MapRole, channel: Channel, color_space: ColorSpace) -> TextureRef {
    TextureRef {
        role,
        channel,
        color_space,
        normal_convention: (role == MapRole::Normal).then_some(NormalConvention::OpenGl),
        // Deliberately mislabel full-size bytes as preview: the real image header
        // must determine whether downsampling is required.
        tiers: BTreeMap::from([(Tier::Preview, source)]),
    }
}
fn scene() -> Scene {
    let mut scene = Scene::new("preview");
    let mut mesh = Mesh {
        positions: vec![[0., 0., 0.], [1., 0., 0.], [0., 1., 0.]],
        face_counts: vec![3],
        face_indices: vec![0, 1, 2],
        ..Default::default()
    };
    mesh.uv_sets.insert(
        "st".into(),
        Attribute {
            interpolation: Interpolation::Vertex,
            values: vec![[0., 0.], [1., 0.], [0., 1.]],
            indices: None,
        },
    );
    scene.meshes.insert("triangle".into(), mesh);
    let mut node = SceneNode::new("Triangle");
    node.mesh = Some("triangle".into());
    node.material = Some(MaterialId::new("test"));
    scene.nodes.insert("triangle".into(), node);
    scene.materials.push(Material::new("test", "Test"));
    scene
}
fn options() -> SceneGltfOptions {
    SceneGltfOptions {
        format: GltfFormat::Gltf,
        texture_tier: Tier::Preview,
        ..Default::default()
    }
}
fn output(scene: &Scene) -> (Json, SceneExport) {
    let output = export_scene(scene, &options()).unwrap();
    (serde_json::from_slice(&output.bytes).unwrap(), output)
}
fn image_bytes(doc: &Json, slot: &str) -> Vec<u8> {
    let index = doc["materials"][0].pointer(slot).unwrap()["index"].as_u64().unwrap() as usize;
    let image = doc["textures"][index]["source"].as_u64().unwrap() as usize;
    BASE64
        .decode(doc["images"][image]["uri"].as_str().unwrap().split_once(',').unwrap().1)
        .unwrap()
}
fn image(doc: &Json, slot: &str) -> RgbaImage {
    image::load_from_memory(&image_bytes(doc, slot)).unwrap().to_rgba8()
}
const BASE: &str = "/pbrMetallicRoughness/baseColorTexture";
const MR: &str = "/pbrMetallicRoughness/metallicRoughnessTexture";

#[test]
fn shared_source_uses_distinct_srgb_data_and_normal_derivatives() {
    let pixels = RgbaImage::from_fn(1024, 4, |x, _| {
        Rgba(if x % 2 == 0 { [255, 0, 0, 255] } else { [0, 255, 0, 255] })
    });
    let source = source(pixels.into(), ImageFormat::Png);
    let mut scene = scene();
    scene.materials[0].surface.base_color = Value::Texture {
        texture: texture(source.clone(), MapRole::BaseColor, Channel::Rgb, ColorSpace::Srgb),
    };
    scene.materials[0].surface.base_metalness = Value::Texture {
        texture: texture(source.clone(), MapRole::Metallic, Channel::B, ColorSpace::Raw),
    };
    scene.materials[0].surface.specular_roughness = Value::Texture {
        texture: texture(source.clone(), MapRole::Roughness, Channel::G, ColorSpace::Raw),
    };
    scene.materials[0].geometry.normal = Some(Value::Texture {
        texture: texture(source, MapRole::Normal, Channel::Rgb, ColorSpace::Raw),
    });
    let before = scene.clone();
    let (doc, out) = output(&scene);
    assert_eq!(scene, before, "export must preserve full-resolution source images");
    assert_eq!(out.bytes, export_scene(&scene, &options()).unwrap().bytes);
    let colour = image(&doc, BASE);
    let data = image(&doc, MR);
    let normal = image(&doc, "/normalTexture");
    assert_eq!(colour.dimensions(), (512, 2));
    assert_eq!(colour.get_pixel(100, 0).0, [188, 188, 0, 255]);
    assert_eq!(data.get_pixel(100, 0).0, [128, 128, 0, 255]);
    assert_eq!(normal.get_pixel(100, 0).0, [128, 128, 0, 255]); // mean vector points -Z
    assert_eq!(
        doc["images"].as_array().unwrap().len(),
        2,
        "identical resulting pixels can still deduplicate"
    );
    assert_eq!(
        out.diagnostics
            .iter()
            .filter(|d| d.code == "gltf.texture_downsampled")
            .count(),
        4
    );
}

#[test]
fn normal_vectors_renormalize_and_directx_green_still_flips() {
    let source = source(
        RgbaImage::from_fn(1024, 2, |x, _| {
            Rgba(if x % 2 == 0 {
                [255, 128, 128, 255]
            } else {
                [128, 255, 128, 255]
            })
        })
        .into(),
        ImageFormat::Png,
    );
    let mut scene = scene();
    let mut normal = texture(source, MapRole::Normal, Channel::Rgb, ColorSpace::Raw);
    normal.normal_convention = Some(NormalConvention::DirectX);
    scene.materials[0].geometry.normal = Some(Value::Texture { texture: normal });
    let (doc, _) = output(&scene);
    let image = image(&doc, "/normalTexture");
    let pixel = image.get_pixel(10, 0).0;
    assert!((216..=219).contains(&pixel[0]) && (36..=39).contains(&pixel[1]));
    let length = pixel[..3]
        .iter()
        .map(|&v| (f32::from(v) / 255. * 2. - 1.).powi(2))
        .sum::<f32>()
        .sqrt();
    assert!((length - 1.).abs() < 0.01);
}

#[test]
fn blend_alpha_is_linear_and_colour_is_alpha_weighted_while_opaque_ignores_alpha() {
    let source = source(
        RgbaImage::from_fn(1024, 2, |x, _| Rgba(if x % 2 == 0 { [255; 4] } else { [0; 4] })).into(),
        ImageFormat::Png,
    );
    let mut scene = scene();
    scene.materials[0].surface.base_color = Value::Texture {
        texture: texture(source, MapRole::BaseColor, Channel::Rgb, ColorSpace::Srgb),
    };
    let (doc, _) = output(&scene);
    assert_eq!(image(&doc, BASE).get_pixel(10, 0).0, [188, 188, 188, 128]);
    scene
        .metadata
        .materials
        .entry("test".into())
        .or_default()
        .original
        .insert(
            "gltf:render_settings".into(),
            MetadataProperty::authored(MetadataValue::from_json(
                json!({"alpha_mode":"BLEND", "alpha_cutoff":0.5, "textures":{}}),
            )),
        );
    let (doc, _) = output(&scene);
    assert_eq!(image(&doc, BASE).get_pixel(10, 0).0, [255, 255, 255, 128]);
    assert_eq!(doc["materials"][0]["alphaMode"], "BLEND");
}

#[test]
fn non_square_and_small_images_preserve_aspect_and_are_never_upscaled() {
    for (width, height, expected) in [(1031, 7, (512, 3)), (1, 1025, (1, 512)), (31, 7, (31, 7))] {
        let source = source(
            RgbaImage::from_pixel(width, height, Rgba([50, 100, 150, 255])).into(),
            ImageFormat::Png,
        );
        let original = source.bytes.clone();
        let mut scene = scene();
        scene.materials[0].surface.base_color = Value::Texture {
            texture: texture(source, MapRole::BaseColor, Channel::Rgb, ColorSpace::Srgb),
        };
        let (doc, out) = output(&scene);
        assert_eq!(image(&doc, BASE).dimensions(), expected);
        if width.max(height) <= 512 {
            assert_eq!(image_bytes(&doc, BASE), original);
            assert!(!out.diagnostics.iter().any(|d| d.code == "gltf.texture_downsampled"));
        }
    }
}

#[test]
fn sixteen_bit_data_and_jpeg_are_filtered_without_gamma_for_raw_channels() {
    let pixels = image::ImageBuffer::<image::Rgba<u16>, _>::from_fn(1024, 2, |x, _| {
        image::Rgba(if x % 2 == 0 {
            [0, 16384, 32768, 65535]
        } else {
            [65535, 49151, 32768, 65535]
        })
    });
    let mut scene = scene();
    scene.materials[0].surface.specular_roughness = Value::Texture {
        texture: texture(
            source(DynamicImage::ImageRgba16(pixels), ImageFormat::Png),
            MapRole::Roughness,
            Channel::G,
            ColorSpace::Raw,
        ),
    };
    let (doc, _) = output(&scene);
    assert_eq!(image(&doc, MR).get_pixel(0, 0).0[1], 128);
    let jpg = source(
        DynamicImage::ImageRgb8(image::RgbImage::from_pixel(1024, 2, image::Rgb([100, 150, 200]))),
        ImageFormat::Jpeg,
    );
    scene.materials[0].surface.base_color = Value::Texture {
        texture: texture(jpg, MapRole::BaseColor, Channel::Rgb, ColorSpace::Srgb),
    };
    let (doc, _) = output(&scene);
    assert_eq!(image(&doc, BASE).dimensions(), (512, 1));
    assert!(image_bytes(&doc, BASE).starts_with(b"\x89PNG"));
}

#[test]
fn source_and_float_working_buffers_are_bounded_before_downsampling() {
    let mut scene = scene();
    let mut input = source(RgbaImage::from_pixel(1024, 4, Rgba([255; 4])).into(), ImageFormat::Png);
    input.width = Some(1);
    input.height = Some(1); // Do not trust supplied dimensions.
    scene.materials[0].surface.base_color = Value::Texture {
        texture: texture(input, MapRole::BaseColor, Channel::Rgb, ColorSpace::Srgb),
    };
    for (max_bytes, expected) in [(8192, "decoded texture"), (32768, "working buffers")] {
        let mut options = options();
        options.limits.max_bytes = max_bytes;
        let error = export_scene(&scene, &options).unwrap_err();
        assert_eq!(error.code, "scene.limit");
        assert!(error.detail.contains(expected), "{error}");
    }
}

#[test]
fn imported_model_exports_actual_requested_preview_dimensions() {
    let mut scene = scene();
    scene.materials[0].surface.base_color = Value::Texture {
        texture: texture(
            source(
                RgbaImage::from_pixel(2048, 4, Rgba([100, 200, 50, 255])).into(),
                ImageFormat::Png,
            ),
            MapRole::BaseColor,
            Channel::Rgb,
            ColorSpace::Srgb,
        ),
    };
    let full = export_scene(&scene, &SceneGltfOptions::default()).unwrap();
    let imported = import_scene(&full.bytes, "input.glb", &Default::default()).unwrap();
    let (doc, _) = output(&imported.scene);
    assert_eq!(image(&doc, BASE).dimensions(), (512, 1));
    assert_eq!(
        imported.scene.materials[0]
            .surface
            .base_color
            .texture()
            .unwrap()
            .source_for(Tier::Preview)
            .unwrap()
            .width,
        Some(2048)
    );
}

#[test]
fn derived_double_sided_material_ids_roundtrip_without_colliding() {
    let mut scene = scene();
    scene.meshes.get_mut("triangle").unwrap().double_sided = true;
    scene
        .materials
        .push(Material::new("test/double-sided/0", "Existing authored identity"));
    let encoded = export_scene(&scene, &SceneGltfOptions::default()).unwrap();
    let imported = import_scene(&encoded.bytes, "double-sided.glb", &Default::default()).unwrap();
    assert!(imported.scene.materials.iter().any(|m| m.id.0 == "test/double-sided/1"));
    assert!(imported.scene.meshes.values().all(|m| m.double_sided));
}

#[test]
fn exr_is_explicitly_outside_the_shipping_preview_profile() {
    let bytes = vec![0x76, 0x2f, 0x31, 0x01, 0x02, 0, 0, 0];
    let source = TextureSource {
        hash: Sha256(format!("{:x}", sha2::Sha256::digest(&bytes))),
        extension: "exr".into(),
        media_type: "image/x-exr".into(),
        width: Some(1024),
        height: Some(1024),
        bytes,
    };
    let mut scene = scene();
    scene.materials[0].surface.specular_roughness = Value::Texture {
        texture: texture(source, MapRole::Roughness, Channel::R, ColorSpace::Raw),
    };
    let error = export_scene(&scene, &options()).unwrap_err();
    assert_eq!(error.code, "gltf.unsupported");
    assert!(error.detail.contains("EXR"));
    let profile = gltf_scene_capabilities();
    assert!(profile["unsupported"].as_array().unwrap().contains(&json!("exr")));
    assert!(
        profile["unsupported"]
            .as_array()
            .unwrap()
            .contains(&json!("udim_assembly"))
    );
}
