//! Real inputs are fetched from the pinned manifest and hash-checked here. They are kept
//! outside Git; see fixtures/corpus/manifest.json for licences and attribution.
use serde_json::{Value, json};
use sha2::Digest;
use usd_toolbox_gltf::scene::*;

#[test]
#[ignore = "requires the pinned external corpus under target/corpus"]
fn pinned_khronos_corpus_roundtrips() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let corpus = std::env::var_os("USD_TOOLBOX_CORPUS_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| root.join("target/corpus"));
    let output = root.join("target/corpus-export");
    std::fs::create_dir_all(&output).unwrap();
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(root.join("fixtures/corpus/manifest.json")).unwrap()).unwrap();
    let mut results = Vec::new();
    for asset in manifest["assets"].as_array().unwrap() {
        let id = asset["id"].as_str().unwrap();
        let file = asset["root"].as_str().unwrap();
        let bytes = std::fs::read(corpus.join(file)).unwrap();
        let declared = asset["files"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["path"] == file)
            .unwrap();
        assert_eq!(
            format!("{:x}", sha2::Sha256::digest(&bytes)),
            declared["sha256"].as_str().unwrap(),
            "{id}: source hash mismatch"
        );
        let started = std::time::Instant::now();
        let preflight = preflight_scene(&bytes, file, &Default::default()).unwrap();
        std::fs::write(
            output.join(format!("{id}.preflight.json")),
            serde_json::to_vec_pretty(&preflight).unwrap(),
        )
        .unwrap();
        let result = import_scene(&bytes, file, &Default::default());
        let import_ms = started.elapsed().as_secs_f64() * 1000.;
        match result {
            Ok(imported) => {
                assert_ne!(asset["expected"], "reject", "{id}: expected profile rejection");
                let started = std::time::Instant::now();
                let encoded = export_scene(&imported.scene, &Default::default())
                    .unwrap_or_else(|e| panic!("{id}: export failed: {e}"));
                let export_ms = started.elapsed().as_secs_f64() * 1000.;
                std::fs::write(output.join(format!("{id}.glb")), &encoded.bytes).unwrap();
                results.push(json!({"id":id,"status":"accepted","input_bytes":bytes.len(),"output_bytes":encoded.bytes.len(),"nodes":imported.scene.nodes.len(),"meshes":imported.scene.meshes.len(),"import_ms":import_ms,"export_ms":export_ms,"diagnostics":imported.diagnostics}));
            }
            Err(error) => {
                assert_ne!(asset["expected"], "accept", "{id}: unexpected rejection: {error}");
                assert_eq!(error.code, "gltf.unsupported", "{id}: unexpected failure kind: {error}");
                results.push(json!({"id":id,"status":"rejected","code":error.code,"path":error.path,"detail":error.detail,"import_ms":import_ms}));
            }
        }
    }
    std::fs::write(output.join("report.json"), serde_json::to_vec_pretty(&results).unwrap()).unwrap();
    eprintln!("{}", serde_json::to_string_pretty(&results).unwrap());
}
