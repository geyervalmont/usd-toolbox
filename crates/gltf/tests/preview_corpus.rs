//! Optional evidence from hash-pinned production assets, separate from exact
//! full-resolution roundtrip fixtures in corpus.rs.
use serde_json::{Value, json};
use sha2::Digest;
use std::{collections::BTreeMap, path::Path};
use usd_toolbox_core::{Tier, scene::Scene};
use usd_toolbox_gltf::scene::*;

fn dimensions(scene: &Scene) -> BTreeMap<String, (u32, u32)> {
    let mut images = BTreeMap::new();
    for material in &scene.materials {
        material.visit_textures(|_, texture| {
            let source = texture.source_for(Tier::Preview).unwrap();
            images.insert(source.hash.0.clone(), (source.width.unwrap(), source.height.unwrap()));
        });
    }
    images
}

#[test]
#[ignore = "requires pinned external corpus under target/corpus"]
fn preview_tiers_on_pinned_corpus() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let corpus = std::env::var_os("USD_TOOLBOX_CORPUS_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| root.join("target/corpus"));
    let destination = root.join("target/corpus-preview");
    std::fs::create_dir_all(&destination).unwrap();
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(root.join("fixtures/corpus/manifest.json")).unwrap()).unwrap();
    let mut report = Vec::new();
    for asset in manifest["assets"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|a| a["expected"] == "accept")
    {
        let id = asset["id"].as_str().unwrap();
        let path = asset["root"].as_str().unwrap();
        let bytes = std::fs::read(corpus.join(path)).unwrap();
        let source = asset["files"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["path"] == path)
            .unwrap();
        assert_eq!(format!("{:x}", sha2::Sha256::digest(&bytes)), source["sha256"]);
        let imported = import_scene(&bytes, path, &Default::default()).unwrap();
        let original_dimensions = dimensions(&imported.scene);
        for tier in [Tier::Preview, Tier::K1] {
            let started = std::time::Instant::now();
            let output = export_scene(
                &imported.scene,
                &SceneGltfOptions {
                    texture_tier: tier,
                    ..Default::default()
                },
            )
            .unwrap();
            let elapsed = started.elapsed();
            let roundtrip = import_scene(&output.bytes, "preview.glb", &Default::default()).unwrap();
            let preview_dimensions = dimensions(&roundtrip.scene);
            assert!(preview_dimensions.values().all(|(w, h)| (*w).max(*h) <= tier.pixels()));
            assert_eq!(dimensions(&imported.scene), original_dimensions);
            assert_eq!(
                output.bytes,
                export_scene(
                    &imported.scene,
                    &SceneGltfOptions {
                        texture_tier: tier,
                        ..Default::default()
                    }
                )
                .unwrap()
                .bytes
            );
            std::fs::write(destination.join(format!("{id}-{}.glb", tier.as_str())), &output.bytes).unwrap();
            report.push(json!({"id":id,"tier":tier.as_str(),"input_bytes":bytes.len(),"output_bytes":output.bytes.len(),"export_ms":elapsed.as_secs_f64()*1000.,"source_images":original_dimensions,"preview_images":preview_dimensions,"diagnostics":output.diagnostics.iter().filter(|d|d.code=="gltf.texture_downsampled").collect::<Vec<_>>()}));
        }
    }
    std::fs::write(
        destination.join("report.json"),
        serde_json::to_vec_pretty(&report).unwrap(),
    )
    .unwrap();
    eprintln!("{}", serde_json::to_string_pretty(&report).unwrap());
}
