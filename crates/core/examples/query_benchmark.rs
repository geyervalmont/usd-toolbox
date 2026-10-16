//! Repeatable release-stage query benchmark. Counts are synthetic, never a claim
//! of representative customer/browser/device performance. Run with:
//! cargo run --release -p usd-toolbox-core --example query_benchmark -- 10000 1000
use std::hint::black_box;
use std::time::Instant;
use usd_toolbox_core::scene::*;
fn main() {
    let args = std::env::args().collect::<Vec<_>>();
    let nodes = args.get(1).and_then(|v| v.parse().ok()).unwrap_or(10_000usize);
    let samples = args.get(2).and_then(|v| v.parse().ok()).unwrap_or(1000usize);
    assert!((1..=100_000).contains(&nodes) && (1..=100_000).contains(&samples));
    let mut scene = Scene::new("synthetic-query-benchmark");
    scene.meshes.insert(
        "shared".into(),
        Mesh {
            positions: vec![[0., 0., 0.], [1., 0., 0.], [0., 1., 0.]],
            face_counts: vec![3],
            face_indices: vec![0, 1, 2],
            ..Default::default()
        },
    );
    for i in 0..nodes {
        let id = format!("node-{i:06}");
        let mut node = SceneNode::new(format!("Object {i}"));
        node.source_path = Some(format!("/Assembly/{id}"));
        node.mesh = Some("shared".into());
        scene.nodes.insert(id.clone(), node);
        scene.metadata.nodes.entry(id).or_default().normalized.insert(
            "category".into(),
            MetadataProperty::authored(MetadataValue::String(format!("category-{}", i % 100))),
        );
    }
    let started = Instant::now();
    let mut doc = SceneDocument::with_options(
        scene,
        DocumentOptions {
            query_limits: QueryLimits {
                max_index_bytes: 128 * 1024 * 1024,
                ..Default::default()
            },
            metadata_indexes: vec![MetadataIndex {
                namespace: MetadataNamespace::Normalized,
                key: "category".into(),
            }],
            ..Default::default()
        },
    )
    .unwrap();
    let open_ms = started.elapsed().as_secs_f64() * 1000.;
    let id = format!("node-{:06}", nodes / 2);
    let path = format!("/Assembly/{id}");
    let request = QueryRequest {
        filter: NodeFilter {
            ids: Some(vec![id.clone()]),
            ..Default::default()
        },
        fields: vec![QueryField::Name],
        ..Default::default()
    };
    let mut lookup = Vec::new();
    let mut exact_query = Vec::new();
    let mut pages = Vec::new();
    for _ in 0..samples {
        let started = Instant::now();
        black_box(doc.node(&id));
        black_box(doc.ids_for_path(&path));
        lookup.push(started.elapsed().as_nanos() as u64);
        let started = Instant::now();
        black_box(doc.query(&request).unwrap());
        exact_query.push(started.elapsed().as_nanos() as u64);
        let started = Instant::now();
        black_box(doc.query(&QueryRequest::default()).unwrap());
        pages.push(started.elapsed().as_nanos() as u64);
    }
    let geometry = doc.scene().meshes["shared"].positions.as_ptr();
    let started = Instant::now();
    doc.apply(&[SceneEdit::SetMetadata {
        target: MetadataTarget::Node(id),
        namespace: MetadataNamespace::Normalized,
        key: "label".into(),
        value: Some(MetadataProperty::authored(MetadataValue::String("Updated".into()))),
    }])
    .unwrap();
    let metadata_edit_ms = started.elapsed().as_secs_f64() * 1000.;
    let same_geometry = geometry == doc.scene().meshes["shared"].positions.as_ptr();
    fn stats(mut values: Vec<u64>) -> serde_json::Value {
        values.sort_unstable();
        serde_json::json!({"p50_ns":values[values.len()/2],"p95_ns":values[(values.len()*95/100).min(values.len()-1)]})
    }
    println!("{}",serde_json::to_string_pretty(&serde_json::json!({"schema":"usd-toolbox.query-benchmark.v1","profile":if cfg!(debug_assertions){"debug"}else{"release"},"platform":std::env::consts::OS,"arch":std::env::consts::ARCH,"nodes":nodes,"samples":samples,"open_and_index_ms":open_ms,"id_and_path_lookup":stats(lookup),"exact_id_query":stats(exact_query),"first_100_rows":stats(pages),"metadata_edit_ms":metadata_edit_ms,"geometry_allocation_retained":same_geometry,"notes":["Synthetic shared mesh; no parse, file transfer, image decoding or browser initialization in measurements.","Open includes validation, typed metadata indexes and streamed scene revision digest.","Latency measured in one process; benchmark allocations/RSS separately with a profiler."]})).unwrap());
}
