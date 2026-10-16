//! Instrumented synthetic stage benchmark. The tracking allocator measures
//! requested allocation bytes, not allocator overhead or operating-system RSS.
use sha2::{Digest, Sha256};
use std::alloc::{GlobalAlloc, Layout, System};
use std::hint::black_box;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;
use usd_toolbox_core::scene::*;
use usd_toolbox_core::{
    Channel, ColorSpace, MapRole, Material, MaterialId, Sha256 as ContentHash, TextureRef, TextureSource, Tier, Value,
};
static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);
static ALLOCATED: AtomicUsize = AtomicUsize::new(0);
static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
struct Tracked;
fn allocated(bytes: usize) {
    ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
    ALLOCATED.fetch_add(bytes, Ordering::Relaxed);
    let live = LIVE.fetch_add(bytes, Ordering::Relaxed) + bytes;
    PEAK.fetch_max(live, Ordering::Relaxed);
}
// SAFETY: Every allocation/deallocation is forwarded unchanged to System. The
// atomics only observe successful allocation sizes and never access user memory.
unsafe impl GlobalAlloc for Tracked {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let p = unsafe { System.alloc(layout) };
        if !p.is_null() {
            allocated(layout.size());
        }
        p
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let p = unsafe { System.alloc_zeroed(layout) };
        if !p.is_null() {
            allocated(layout.size());
        }
        p
    }
    unsafe fn dealloc(&self, p: *mut u8, layout: Layout) {
        unsafe { System.dealloc(p, layout) };
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
    }
    unsafe fn realloc(&self, p: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        let next = unsafe { System.realloc(p, layout, size) };
        if !next.is_null() {
            LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
            allocated(size);
        }
        next
    }
}
#[global_allocator]
static ALLOCATOR: Tracked = Tracked;
#[derive(serde::Serialize)]
struct Metrics {
    milliseconds: f64,
    allocation_calls: usize,
    allocated_bytes: usize,
    live_requested_bytes: usize,
    peak_requested_bytes: usize,
    incremental_peak_requested_bytes: usize,
}
fn timed<T>(f: impl FnOnce() -> T) -> (T, Metrics) {
    let allocations = ALLOCATIONS.load(Ordering::Relaxed);
    let allocated = ALLOCATED.load(Ordering::Relaxed);
    let before = LIVE.load(Ordering::Relaxed);
    PEAK.store(before, Ordering::Relaxed);
    let started = Instant::now();
    let result = f();
    let elapsed = started.elapsed().as_secs_f64() * 1000.;
    let peak = PEAK.load(Ordering::Relaxed);
    (
        result,
        Metrics {
            milliseconds: elapsed,
            allocation_calls: ALLOCATIONS.load(Ordering::Relaxed) - allocations,
            allocated_bytes: ALLOCATED.load(Ordering::Relaxed) - allocated,
            live_requested_bytes: LIVE.load(Ordering::Relaxed),
            peak_requested_bytes: peak,
            incremental_peak_requested_bytes: peak.saturating_sub(before),
        },
    )
}
fn triangle() -> Mesh {
    Mesh {
        positions: vec![[0., 0., 0.], [1., 0., 0.], [0., 1., 0.]],
        face_counts: vec![3],
        face_indices: vec![0, 1, 2],
        ..Default::default()
    }
}
fn fixture(case: &str, size: usize) -> Scene {
    let mut scene = Scene::new(format!("benchmark:{case}"));
    scene.meshes.insert("mesh".into(), triangle());
    if case == "dense" {
        let n = size;
        let mesh = scene.meshes.get_mut("mesh").unwrap();
        *mesh = Mesh::default();
        for y in 0..=n {
            for x in 0..=n {
                mesh.positions.push([x as f64, y as f64, 0.]);
            }
        }
        for y in 0..n {
            for x in 0..n {
                let a = (y * (n + 1) + x) as u32;
                let b = a + 1;
                let c = a + (n + 1) as u32;
                mesh.face_counts.extend([3, 3]);
                mesh.face_indices.extend([a, b, c + 1, a, c + 1, c]);
            }
        }
    }
    let count = if case == "dense" { 1 } else { size };
    for i in 0..count {
        let id = format!("node-{i:06}");
        let mut node = SceneNode::new(format!("Object {i}"));
        node.source_path = Some(format!("/Assembly/{id}"));
        node.mesh = Some("mesh".into());
        if case == "repeated" {
            node.transform[0][3] = (i % 100) as f64;
            node.transform[2][3] = (i / 100) as f64;
        }
        if case == "deep" && i % 128 != 0 {
            node.parent = Some(format!("node-{:06}", i - 1));
        }
        if case == "many_small" {
            let mesh_id = format!("mesh-{i:06}");
            scene.meshes.insert(mesh_id.clone(), triangle());
            node.mesh = Some(mesh_id);
        }
        scene.nodes.insert(id.clone(), node);
        scene.metadata.nodes.entry(id).or_default().normalized.insert(
            "category".into(),
            MetadataProperty::authored(MetadataValue::String(format!("category-{}", i % 100))),
        );
    }
    if case == "many_small" {
        scene.meshes.remove("mesh");
    }
    if case == "textured" {
        let bytes = png(512);
        let hash = format!("{:x}", Sha256::digest(&bytes));
        for i in 0..4 {
            let mut material = Material::new(format!("mat-{i}"), format!("Texture {i}"));
            material.surface.base_color = Value::Texture {
                texture: TextureRef {
                    role: MapRole::BaseColor,
                    tiers: [(
                        Tier::Preview,
                        TextureSource {
                            hash: ContentHash(hash.clone()),
                            extension: "png".into(),
                            media_type: "image/png".into(),
                            width: Some(512),
                            height: Some(512),
                            bytes: bytes.clone(),
                        },
                    )]
                    .into(),
                    color_space: ColorSpace::Srgb,
                    channel: Channel::Rgb,
                    normal_convention: None,
                },
            };
            scene.materials.push(material);
        }
        for (i, node) in scene.nodes.values_mut().enumerate() {
            node.material = Some(MaterialId::new(format!("mat-{}", i % 4)));
        }
    }
    scene
}
fn options() -> DocumentOptions {
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
    }
}
fn main() {
    let args = std::env::args().collect::<Vec<_>>();
    let case = args.get(1).map(String::as_str).unwrap_or("repeated");
    assert!(
        [
            "repeated",
            "wide",
            "deep",
            "many_small",
            "dense",
            "textured",
            "over_limit"
        ]
        .contains(&case)
    );
    let size = args
        .get(2)
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(if case == "dense" { 128 } else { 10000 });
    let samples = args.get(3).and_then(|v| v.parse::<usize>().ok()).unwrap_or(1000);
    assert!((1..=50_000).contains(&size) && (1..=10_000).contains(&samples));
    if case == "dense" {
        assert!(size <= 256);
    }
    if case == "over_limit" {
        let scene = fixture("repeated", 2);
        let (_, metric) = timed(|| {
            let result = SceneDocument::new(
                scene,
                SceneLimits {
                    max_nodes: 1,
                    ..Default::default()
                },
            );
            assert_eq!(result.err().unwrap().code, "scene.limit");
        });
        println!(
            "{}",
            serde_json::json!({"schema":"usd-toolbox.document-benchmark.v1","case":case,"rejected":true,"rejection":metric})
        );
        return;
    }
    let scene = fixture(case, size);
    let nodes = scene.nodes.len();
    let meshes = scene.meshes.len();
    let faces = scene.meshes.values().map(|m| m.face_counts.len()).sum::<usize>();
    let encoded = serde_json::to_vec(&scene).unwrap();
    drop(scene);
    let (scene, parse) = timed(|| serde_json::from_slice::<Scene>(&encoded).unwrap());
    let (mut doc, open) = timed(|| SceneDocument::with_options(scene, options()).unwrap());
    let id = format!("node-{:06}", nodes / 2);
    let path = format!("/Assembly/{id}");
    let target_mesh = doc.node(&id).unwrap().mesh.clone().unwrap();
    let mut latencies = Vec::with_capacity(samples);
    let (_, lookups) = timed(|| {
        for _ in 0..samples {
            let started = Instant::now();
            black_box(doc.node(&id));
            black_box(doc.ids_for_path(&path));
            latencies.push(started.elapsed().as_nanos() as u64);
        }
    });
    latencies.sort_unstable();
    let lookup_p95_ns = latencies[samples * 95 / 100];
    let lookup_p50_ns = latencies[samples / 2];
    let request = QueryRequest {
        filter: NodeFilter {
            ids: Some(vec![id.clone()]),
            ..Default::default()
        },
        fields: vec![QueryField::Name],
        ..Default::default()
    };
    latencies.clear();
    let (_, queries) = timed(|| {
        for _ in 0..samples {
            let started = Instant::now();
            black_box(doc.query(&request).unwrap());
            latencies.push(started.elapsed().as_nanos() as u64);
        }
    });
    latencies.sort_unstable();
    let query_p95_ns = latencies[samples * 95 / 100];
    let query_p50_ns = latencies[samples / 2];
    let (_, bounds_cold) = timed(|| black_box(doc.bounds(&id).unwrap()));
    let (_, bounds_warm) = timed(|| {
        for _ in 0..samples {
            black_box(doc.bounds(&id).unwrap());
        }
    });
    let node_pointer = doc.node(&id).unwrap() as *const SceneNode;
    let geometry_pointer = doc.scene().meshes[&target_mesh].positions.as_ptr();
    let material_pointer = doc.scene().materials.as_ptr();
    let (_, metadata_edit) = timed(|| {
        doc.apply(&[SceneEdit::SetMetadata {
            target: MetadataTarget::Node(id.clone()),
            namespace: MetadataNamespace::Normalized,
            key: "label".into(),
            value: Some(MetadataProperty::authored(MetadataValue::String("Updated".into()))),
        }])
        .unwrap()
    });
    let no_payload_copy = std::ptr::eq(node_pointer, doc.node(&id).unwrap())
        && geometry_pointer == doc.scene().meshes[&target_mesh].positions.as_ptr()
        && material_pointer == doc.scene().materials.as_ptr();
    assert!(no_payload_copy);
    let (_, rename) = timed(|| {
        doc.apply(&[SceneEdit::Rename {
            node: id.clone(),
            name: "Renamed".into(),
        }])
        .unwrap()
    });
    let live = LIVE.load(Ordering::Relaxed);
    for _ in 0..samples {
        black_box(doc.query(&request).unwrap());
    }
    let repeated_query_live_growth = LIVE.load(Ordering::Relaxed) as i128 - live as i128;
    assert_eq!(repeated_query_live_growth, 0);
    let first_metadata_ms = parse.milliseconds + open.milliseconds;
    drop(doc);
    drop(encoded);
    let cycles = 10;
    let mut lifecycle_growth = Vec::with_capacity(cycles);
    let (_, lifecycle) = timed(|| {
        for _ in 0..cycles {
            let before = LIVE.load(Ordering::Relaxed);
            {
                let doc = SceneDocument::with_options(
                    fixture(case, size.min(if case == "dense" { 32 } else { 1000 })),
                    options(),
                )
                .unwrap();
                black_box(doc.query(&QueryRequest::default()).unwrap());
                black_box(doc.bounds("node-000000").unwrap());
            }
            let growth = LIVE.load(Ordering::Relaxed) as i128 - before as i128;
            lifecycle_growth.push(growth);
            assert_eq!(growth, 0);
        }
    });
    println!("{}",serde_json::to_string_pretty(&serde_json::json!({"schema":"usd-toolbox.document-benchmark.v1","case":case,"size":size,"samples":samples,"nodes":nodes,"unique_meshes":meshes,"faces":faces,"parse_json":parse,"open_and_index":open,"time_to_first_metadata_ms":first_metadata_ms,"lookup":{"measurement":lookups,"p50_ns":lookup_p50_ns,"p95_ns":lookup_p95_ns},"exact_id_query":{"measurement":queries,"p50_ns":query_p50_ns,"p95_ns":query_p95_ns},"bounds_cold":bounds_cold,"bounds_warm":bounds_warm,"metadata_edit":metadata_edit,"rename":rename,"payload_allocations_retained":no_payload_copy,"repeated_query_live_growth_bytes":repeated_query_live_growth,"lifecycle":{"cycles":cycles,"measurement":lifecycle,"retained_growth_bytes":lifecycle_growth},"notes":["Allocation tracking adds atomic overhead; compare this instrumented executable only with itself.","requested allocation bytes exclude allocator overhead; runner records OS peak RSS separately.","JSON parsing and document stages exclude file/network transfer and browser initialization; PNG bytes retained but never decoded by core metadata queries.","Deep workload uses independent chains of 128 nodes; lifecycle fixtures capped at 1000 nodes or 32x32 dense grid."]})).unwrap());
}
fn png(size: u32) -> Vec<u8> {
    fn crc(bytes: &[u8]) -> u32 {
        let mut crc = !0u32;
        for byte in bytes {
            crc ^= u32::from(*byte);
            for _ in 0..8 {
                crc = (crc >> 1) ^ if crc & 1 != 0 { 0xedb88320 } else { 0 };
            }
        }
        !crc
    }
    fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
        out.extend((data.len() as u32).to_be_bytes());
        let start = out.len();
        out.extend(kind);
        out.extend(data);
        out.extend(crc(&out[start..]).to_be_bytes());
    }
    let mut raw = Vec::new();
    for y in 0..size {
        raw.push(0);
        for x in 0..size {
            raw.extend([x as u8, y as u8, (x ^ y) as u8, 255]);
        }
    }
    let mut zlib = vec![0x78, 0x01];
    let chunks = raw.chunks(65535).collect::<Vec<_>>();
    for (i, block) in chunks.iter().enumerate() {
        zlib.push(u8::from(i + 1 == chunks.len()));
        let n = block.len() as u16;
        zlib.extend(n.to_le_bytes());
        zlib.extend((!n).to_le_bytes());
        zlib.extend(*block);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for byte in raw {
        a = (a + u32::from(byte)) % 65521;
        b = (b + a) % 65521;
    }
    zlib.extend(((b << 16) | a).to_be_bytes());
    let mut output = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut ihdr = Vec::new();
    ihdr.extend(size.to_be_bytes());
    ihdr.extend(size.to_be_bytes());
    ihdr.extend([8, 6, 0, 0, 0]);
    chunk(&mut output, b"IHDR", &ihdr);
    chunk(&mut output, b"IDAT", &zlib);
    chunk(&mut output, b"IEND", &[]);
    output
}
