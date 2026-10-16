use super::*;

pub fn multiply(a: Matrix4, b: Matrix4) -> Matrix4 {
    std::array::from_fn(|r| std::array::from_fn(|c| (0..4).map(|k| a[r][k] * b[k][c]).sum()))
}

pub fn transform_point(m: Matrix4, p: [f64; 3]) -> [f64; 3] {
    std::array::from_fn(|r| m[r][3] + (0..3).map(|c| m[r][c] * p[c]).sum::<f64>())
}

pub fn determinant(m: Matrix4) -> f64 {
    m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1]) - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0])
}

pub fn inverse(m: Matrix4) -> Option<Matrix4> {
    let mut a = m;
    let mut b = IDENTITY;
    for i in 0..4 {
        let pivot = (i..4).max_by(|&x, &y| a[x][i].abs().total_cmp(&a[y][i].abs()))?;
        if a[pivot][i].abs() < 1e-15 {
            return None;
        }
        a.swap(i, pivot);
        b.swap(i, pivot);
        let scale = a[i][i];
        for j in 0..4 {
            a[i][j] /= scale;
            b[i][j] /= scale;
        }
        for r in 0..4 {
            if r == i {
                continue;
            }
            let factor = a[r][i];
            for c in 0..4 {
                a[r][c] -= factor * a[i][c];
                b[r][c] -= factor * b[i][c];
            }
        }
    }
    Some(b)
}

pub fn world_transform(scene: &Scene, id: &str) -> Result<Matrix4, SceneError> {
    let mut chain = Vec::new();
    let mut current = Some(id);
    let mut visited = std::collections::BTreeSet::new();
    while let Some(id) = current {
        if !visited.insert(id) {
            return Err(SceneError::new("scene.cycle", id, "parent cycle"));
        }
        let node = scene
            .nodes
            .get(id)
            .ok_or_else(|| SceneError::new("scene.missing_node", id, "node not found"))?;
        chain.push(node.transform);
        current = if node.reset_transform {
            None
        } else {
            node.parent.as_deref()
        };
    }
    Ok(chain.into_iter().rev().fold(IDENTITY, multiply))
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
pub struct Bounds {
    pub min: [f64; 3],
    pub max: [f64; 3],
}

impl Bounds {
    pub fn include(&mut self, point: [f64; 3]) {
        for (i, value) in point.iter().enumerate() {
            self.min[i] = self.min[i].min(*value);
            self.max[i] = self.max[i].max(*value);
        }
    }
}

pub fn mesh_bounds(mesh: &Mesh) -> Option<Bounds> {
    let first = *mesh.positions.first()?;
    let mut bounds = Bounds { min: first, max: first };
    for &p in &mesh.positions {
        bounds.include(p);
    }
    Some(bounds)
}

pub fn scene_bounds(scene: &Scene) -> Result<Option<Bounds>, SceneError> {
    let mut bounds: Option<Bounds> = None;
    for (id, node) in &scene.nodes {
        let Some(mesh_id) = &node.mesh else {
            continue;
        };
        let mesh = scene
            .meshes
            .get(mesh_id)
            .ok_or_else(|| SceneError::new("scene.missing_mesh", id, mesh_id))?;
        let matrix = world_transform(scene, id)?;
        for &p in &mesh.positions {
            let p = transform_point(matrix, p);
            if !p.iter().all(|v| v.is_finite()) {
                return Err(SceneError::new(
                    "scene.non_finite_bounds",
                    id,
                    "world transform overflow",
                ));
            }
            match &mut bounds {
                Some(b) => b.include(p),
                None => bounds = Some(Bounds { min: p, max: p }),
            }
        }
    }
    Ok(bounds)
}

pub fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

pub fn normalize(v: [f64; 3]) -> Option<[f64; 3]> {
    let length = v.iter().map(|x| x * x).sum::<f64>().sqrt();
    (length.is_finite() && length > 1e-20).then(|| v.map(|x| x / length))
}
