use super::*;

/// Deterministic ear clipping of simple planar polygons. Returns indices into
/// the face's corner list, preserving face-varying attributes and winding.
pub fn triangulate(points: &[[f64; 3]]) -> Result<Vec<[usize; 3]>, SceneError> {
    let invalid = |detail| SceneError::new("mesh.polygon", "face", detail);
    if points.len() < 3 || points.len() > 1024 || points.iter().flatten().any(|v| !v.is_finite()) {
        return Err(invalid("polygon must have 3..1024 finite corners"));
    }
    // Work near the origin so large model coordinates do not cancel polygon area.
    let local: Vec<_> = points
        .iter()
        .map(|p| std::array::from_fn::<_, 3, _>(|i| p[i] - points[0][i]))
        .collect();
    let points = local.as_slice();
    let mut normal = [0.; 3];
    for i in 0..points.len() {
        let a = points[i];
        let b = points[(i + 1) % points.len()];
        normal[0] += (a[1] - b[1]) * (a[2] + b[2]);
        normal[1] += (a[2] - b[2]) * (a[0] + b[0]);
        normal[2] += (a[0] - b[0]) * (a[1] + b[1]);
    }
    let normal = normalize(normal).ok_or_else(|| invalid("degenerate polygon normal"))?;
    let extent = points
        .iter()
        .map(|p| (0..3).map(|i| (p[i] - points[0][i]).abs()).fold(0f64, f64::max))
        .fold(0f64, f64::max);
    let tolerance = (extent * 1e-7).max(1e-12);
    if points
        .iter()
        .any(|p| (0..3).map(|i| (p[i] - points[0][i]) * normal[i]).sum::<f64>().abs() > tolerance)
    {
        return Err(invalid("non-planar polygon; tessellate at source"));
    }
    let axis = (0..3)
        .max_by(|&a, &b| normal[a].abs().total_cmp(&normal[b].abs()))
        .unwrap();
    let axes: Vec<_> = (0..3).filter(|i| *i != axis).collect();
    let p: Vec<_> = points.iter().map(|v| [v[axes[0]], v[axes[1]]]).collect();
    let epsilon = (extent * extent * 1e-12).max(1e-24);
    let area: f64 = (0..p.len())
        .map(|i| p[i][0] * p[(i + 1) % p.len()][1] - p[(i + 1) % p.len()][0] * p[i][1])
        .sum();
    if area.abs() <= epsilon {
        return Err(invalid("zero area polygon"));
    }
    for i in 0..p.len() {
        for j in i + 1..p.len() {
            if (p[i][0] - p[j][0]).abs() <= tolerance && (p[i][1] - p[j][1]).abs() <= tolerance {
                return Err(invalid("repeated polygon vertex"));
            }
            let i2 = (i + 1) % p.len();
            let j2 = (j + 1) % p.len();
            if i2 == j || j2 == i {
                continue;
            }
            if intersects(p[i], p[i2], p[j], p[j2], epsilon) {
                return Err(invalid("self-intersecting polygon"));
            }
        }
    }
    let sign = area.signum();
    let mut remaining: Vec<_> = (0..p.len()).collect();
    let mut triangles = Vec::new();
    while remaining.len() > 3 {
        let ear = (0..remaining.len())
            .find(|&i| {
                let a = remaining[(i + remaining.len() - 1) % remaining.len()];
                let b = remaining[i];
                let c = remaining[(i + 1) % remaining.len()];
                orient(p[a], p[b], p[c]) * sign > epsilon
                    && !remaining.iter().any(|&j| {
                        j != a
                            && j != b
                            && j != c
                            && orient(p[a], p[b], p[j]) * sign >= -epsilon
                            && orient(p[b], p[c], p[j]) * sign >= -epsilon
                            && orient(p[c], p[a], p[j]) * sign >= -epsilon
                    })
            })
            .ok_or_else(|| invalid("polygon cannot be triangulated without degeneracy"))?;
        let a = remaining[(ear + remaining.len() - 1) % remaining.len()];
        let b = remaining[ear];
        let c = remaining[(ear + 1) % remaining.len()];
        triangles.push([a, b, c]);
        remaining.remove(ear);
    }
    if orient(p[remaining[0]], p[remaining[1]], p[remaining[2]]).abs() <= epsilon {
        return Err(invalid("degenerate final triangle"));
    }
    triangles.push([remaining[0], remaining[1], remaining[2]]);
    Ok(triangles)
}

fn orient(a: [f64; 2], b: [f64; 2], c: [f64; 2]) -> f64 {
    (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
}
fn intersects(a: [f64; 2], b: [f64; 2], c: [f64; 2], d: [f64; 2], epsilon: f64) -> bool {
    let values = [orient(a, b, c), orient(a, b, d), orient(c, d, a), orient(c, d, b)];
    if values.iter().all(|v| v.abs() > epsilon) {
        return values[0].signum() != values[1].signum() && values[2].signum() != values[3].signum();
    }
    let on = |a: [f64; 2], b: [f64; 2], p: [f64; 2]| {
        (0..2).all(|i| p[i] >= a[i].min(b[i]) - epsilon && p[i] <= a[i].max(b[i]) + epsilon)
    };
    (values[0].abs() <= epsilon && on(a, b, c))
        || (values[1].abs() <= epsilon && on(a, b, d))
        || (values[2].abs() <= epsilon && on(c, d, a))
        || (values[3].abs() <= epsilon && on(c, d, b))
}
