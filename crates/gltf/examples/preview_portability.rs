//! Native/WASM parity probe: run natively, then invoke the exported checksum in
//! Node from the wasm32-unknown-unknown build. No browser or WASI imports needed.
use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use image::{DynamicImage, ImageFormat, Rgba, RgbaImage};
use sha2::Digest;
use std::{collections::BTreeMap, io::Cursor};
use usd_toolbox_core::{scene::*, *};
use usd_toolbox_gltf::{GltfFormat, scene::*};

#[unsafe(no_mangle)]
pub extern "C" fn preview_checksum() -> u64 {
    let pixels = RgbaImage::from_fn(1027, 5, |x, y| {
        Rgba([(x * 17 + y) as u8, (x * 31 + y * 11) as u8, (x + y * 43) as u8, 255])
    });
    let mut encoded = Cursor::new(Vec::new());
    DynamicImage::ImageRgba8(pixels)
        .write_to(&mut encoded, ImageFormat::Png)
        .unwrap();
    let bytes = encoded.into_inner();
    let source = TextureSource {
        hash: Sha256(format!("{:x}", sha2::Sha256::digest(&bytes))),
        extension: "png".into(),
        media_type: "image/png".into(),
        width: Some(1027),
        height: Some(5),
        bytes,
    };
    let texture = |role, channel, color_space| TextureRef {
        role,
        channel,
        color_space,
        normal_convention: (role == MapRole::Normal).then_some(NormalConvention::OpenGl),
        tiers: BTreeMap::from([(Tier::K2, source.clone())]),
    };
    let mut material = Material::new("test", "Test");
    material.surface.base_color = Value::Texture {
        texture: texture(MapRole::BaseColor, Channel::Rgb, ColorSpace::Srgb),
    };
    material.surface.specular_roughness = Value::Texture {
        texture: texture(MapRole::Roughness, Channel::G, ColorSpace::Raw),
    };
    material.geometry.normal = Some(Value::Texture {
        texture: texture(MapRole::Normal, Channel::Rgb, ColorSpace::Raw),
    });
    let mut scene = Scene::new("portable-preview");
    scene.materials.push(material);
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
    let output = export_scene(
        &scene,
        &SceneGltfOptions {
            format: GltfFormat::Gltf,
            texture_tier: Tier::Preview,
            ..Default::default()
        },
    )
    .unwrap();
    let document: serde_json::Value = serde_json::from_slice(&output.bytes).unwrap();
    assert_eq!(document["images"].as_array().unwrap().len(), 3);
    for image in document["images"].as_array().unwrap() {
        let bytes = BASE64
            .decode(image["uri"].as_str().unwrap().split_once(',').unwrap().1)
            .unwrap();
        let decoded = image::load_from_memory(&bytes).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (512, 2));
    }
    assert_eq!(
        scene.materials[0]
            .surface
            .base_color
            .texture()
            .unwrap()
            .source_for(Tier::Preview)
            .unwrap()
            .width,
        Some(1027)
    );
    u64::from_le_bytes(sha2::Sha256::digest(&output.bytes)[..8].try_into().unwrap())
}

fn main() {
    println!("{}", preview_checksum());
}
