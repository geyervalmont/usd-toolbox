//! A bounded, deterministic adjacency walk. Triangle order is only a preview
//! optimization and must never change an order-dependent material's rendering.
use super::*;

pub(super) fn is_opaque(material: Option<&Material>, settings: Option<&JsonValue>) -> bool {
    let Some(material) = material else { return true };
    if settings
        .and_then(|s| s["alpha_mode"].as_str())
        .is_some_and(|mode| mode != "OPAQUE")
    {
        return false;
    }
    // Be conservative even if retained render metadata masks authored opacity.
    if color4_parts(&material.surface.base_color).0[3] < 1.
        || material
            .geometry
            .opacity
            .as_ref()
            .is_some_and(|v| v.texture().is_some() || scalar_parts(v).0 < 1.)
    {
        return false;
    }
    // Transmission is order-sensitive even when glTF alphaMode is OPAQUE.
    !material
        .surface
        .transmission_weight
        .as_ref()
        .is_some_and(|v| v.texture().is_some() || scalar_parts(v).0 > 0.)
}

pub(super) enum Outcome {
    Improved(Report),
    Unchanged,
    Budget,
}
pub(super) struct Report {
    pub original_vertices: Vec<u32>,
    pub before: [usize; 2],
    pub after: [usize; 2],
}

// FIFO insertion timestamps permit O(indices + vertices) cache simulation. This
// is an explicit proxy, not a prediction of a particular GPU's implementation.
fn misses(indices: &[u32], vertices: usize, capacity: u32) -> usize {
    let mut stamps = vec![0u32; vertices];
    let mut clock = 1u32;
    for &vertex in indices {
        let stamp = &mut stamps[vertex as usize];
        if *stamp == 0 || clock - *stamp > capacity {
            *stamp = clock;
            clock += 1;
        }
    }
    (clock - 1) as usize
}

pub(super) fn optimize(indices: &mut Vec<u32>, vertices: usize, available_bytes: usize) -> Outcome {
    if indices.len() < 6 {
        return Outcome::Unchanged;
    }
    // Includes adjacency, offsets/cursors, live valence, insertion timestamps,
    // emitted flags, traversal stack, candidate indices, both remapping arrays,
    // simulation scratch and the largest copied attribute stream. Arrays have
    // platform-sized offsets; this bound is conservative on wasm32 as well.
    let scratch = vertices
        .saturating_mul(64)
        .saturating_add(indices.len().saturating_mul(12))
        .saturating_add(indices.len() / 3)
        .saturating_add(16);
    if scratch > available_bytes || indices.len() >= u32::MAX as usize {
        return Outcome::Budget;
    }
    let before = [misses(indices, vertices, 16), misses(indices, vertices, 32)];
    let mut candidate = traversal(indices, vertices);
    let after = [misses(&candidate, vertices, 16), misses(&candidate, vertices, 32)];
    if after[0] > before[0] || after[1] > before[1] || after == before {
        return Outcome::Unchanged;
    }
    let mut remapping = vec![u32::MAX; vertices];
    let mut original_vertices = Vec::with_capacity(vertices);
    for vertex in &mut candidate {
        let mapped = &mut remapping[*vertex as usize];
        if *mapped == u32::MAX {
            *mapped = original_vertices.len() as u32;
            original_vertices.push(*vertex);
        }
        *vertex = *mapped;
    }
    debug_assert_eq!(original_vertices.len(), vertices);
    *indices = candidate;
    Outcome::Improved(Report {
        original_vertices,
        before,
        after,
    })
}

fn traversal(indices: &[u32], vertices: usize) -> Vec<u32> {
    let mut remaining = vec![0u32; vertices];
    for &v in indices {
        remaining[v as usize] += 1;
    }
    let mut offsets = Vec::with_capacity(vertices + 1);
    offsets.push(0usize);
    for &count in &remaining {
        offsets.push(offsets.last().unwrap() + count as usize);
    }
    let mut cursors = offsets[..vertices].to_vec();
    let mut adjacency = vec![0u32; indices.len()];
    for (triangle, corners) in indices.as_chunks::<3>().0.iter().enumerate() {
        for &v in corners {
            adjacency[cursors[v as usize]] = triangle as u32;
            cursors[v as usize] += 1;
        }
    }
    cursors.copy_from_slice(&offsets[..vertices]);
    let mut emitted = vec![false; indices.len() / 3];
    let mut stamps = vec![0u32; vertices];
    let mut cache = [u32::MAX; 16];
    let mut clock = 1u32;
    let mut stack = Vec::with_capacity(indices.len());
    let mut output = Vec::with_capacity(indices.len());
    let mut seed = 0usize;
    let mut current = indices[0] as usize;
    loop {
        // Each vertex's adjacency range is drained once; every adjacency entry
        // and stack entry is examined at most once, including high-valence fans.
        while cursors[current] < offsets[current + 1] {
            let triangle = adjacency[cursors[current]] as usize;
            cursors[current] += 1;
            if emitted[triangle] {
                continue;
            }
            emitted[triangle] = true;
            for &v in &indices[triangle * 3..triangle * 3 + 3] {
                output.push(v);
                remaining[v as usize] -= 1;
                stack.push(v);
                let stamp = &mut stamps[v as usize];
                if *stamp == 0 || clock - *stamp > 16 {
                    cache[(clock as usize - 1) % 16] = v;
                    *stamp = clock;
                    clock += 1;
                }
            }
        }
        // Prefer a cached vertex with few pending triangles, then the most
        // recent insertion. Stable numeric tie-breaking keeps exports repeatable.
        if let Some(&next) = cache
            .iter()
            .filter(|&&v| v != u32::MAX && remaining[v as usize] > 0)
            .min_by_key(|&&v| (remaining[v as usize], std::cmp::Reverse(stamps[v as usize]), v))
        {
            current = next as usize;
            continue;
        }
        let mut next = None;
        while let Some(v) = stack.pop() {
            if remaining[v as usize] > 0 {
                next = Some(v as usize);
                break;
            }
        }
        if let Some(next) = next {
            current = next;
            continue;
        }
        while seed < emitted.len() && emitted[seed] {
            seed += 1;
        }
        if seed == emitted.len() {
            break;
        }
        current = indices[seed * 3] as usize;
    }
    output
}

pub(super) fn remap(values: &mut Vec<f32>, components: usize, original_vertices: &[u32]) {
    let original = std::mem::take(values);
    values.reserve_exact(original.len());
    for &vertex in original_vertices {
        let start = vertex as usize * components;
        values.extend_from_slice(&original[start..start + components]);
    }
}
