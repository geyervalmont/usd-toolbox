use super::*;

pub(super) fn read_transform(data: &dyn AbstractData, prim: &Path) -> Result<(Matrix4, bool, bool), SceneError> {
    let order = match value(data, prim, "xformOpOrder")? {
        None => return Ok((IDENTITY, false, false)),
        Some(UsdValue::TokenVec(v)) => v,
        _ => return Err(err("usd.transform", prim.as_str(), "invalid xformOpOrder")),
    };
    let mut result = IDENTITY;
    let mut reset = false;
    for op in &order {
        let name = op.as_str();
        if name == "!resetXformStack!" {
            result = IDENTITY;
            reset = true;
            continue;
        }
        let (inverted, name) = name.strip_prefix("!invert!").map_or((false, name), |v| (true, v));
        let v =
            value(data, prim, name)?.ok_or_else(|| err("usd.transform", prim.as_str(), format!("missing {name}")))?;
        let kind = name.split(':').nth(1).unwrap_or("");
        let mut m = IDENTITY;
        match kind {
            "transform" => {
                let UsdValue::Matrix4d(v) = v else {
                    return Err(err("usd.transform", prim.as_str(), "expected matrix4d"));
                };
                m = std::array::from_fn(|r| std::array::from_fn(|c| v.0[c * 4 + r]));
            }
            "translate" | "scale" => {
                let v = vec3(v, prim)?;
                for i in 0..3 {
                    if kind == "translate" {
                        m[i][3] = v[i];
                    } else {
                        m[i][i] = v[i];
                    }
                }
            }
            "rotateX" | "rotateY" | "rotateZ" => {
                let angle = match v {
                    UsdValue::Float(v) => f64::from(v),
                    UsdValue::Double(v) => v,
                    _ => return Err(err("usd.transform", prim.as_str(), "expected angle")),
                };
                m = rotation(kind.as_bytes()[6] as char, angle);
            }
            "rotateXYZ" | "rotateXZY" | "rotateYXZ" | "rotateYZX" | "rotateZXY" | "rotateZYX" => {
                let v = vec3(v, prim)?;
                for axis in kind[6..].chars() {
                    m = multiply(rotation(axis, v[axis as usize - 'X' as usize]), m);
                }
            }
            "orient" => {
                let q = match v {
                    UsdValue::Quatf(v) => [v.w, v.x, v.y, v.z].map(f64::from),
                    UsdValue::Quatd(v) => [v.w, v.x, v.y, v.z],
                    _ => return Err(err("usd.transform", prim.as_str(), "expected quaternion")),
                };
                let length = q.iter().map(|v| v * v).sum::<f64>().sqrt();
                if !length.is_finite() || length < 1e-15 {
                    return Err(err("usd.transform", prim.as_str(), "invalid quaternion"));
                }
                // Gf quaternion storage is real, imaginary x/y/z.
                let [w, x, y, z] = q.map(|v| v / length);
                m = [
                    [
                        1. - 2. * (y * y + z * z),
                        2. * (x * y - z * w),
                        2. * (x * z + y * w),
                        0.,
                    ],
                    [
                        2. * (x * y + z * w),
                        1. - 2. * (x * x + z * z),
                        2. * (y * z - x * w),
                        0.,
                    ],
                    [
                        2. * (x * z - y * w),
                        2. * (y * z + x * w),
                        1. - 2. * (x * x + y * y),
                        0.,
                    ],
                    [0., 0., 0., 1.],
                ];
            }
            _ => return Err(err("usd.transform_unsupported", prim.as_str(), name)),
        }
        if inverted {
            m = inverse(m).ok_or_else(|| err("usd.transform", prim.as_str(), "singular inverse operation"))?;
        }
        result = multiply(result, m);
    }
    let projected = order
        .iter()
        .any(|v| !matches!(v.as_str(), "xformOp:transform" | "!resetXformStack!"));
    Ok((result, reset, projected))
}

fn vec3(v: UsdValue, p: &Path) -> Result<[f64; 3], SceneError> {
    match v {
        UsdValue::Vec3f(v) => Ok(<[f32; 3]>::from(v).map(f64::from)),
        UsdValue::Vec3d(v) => Ok(v.into()),
        _ => Err(err("usd.transform", p.as_str(), "expected vector3")),
    }
}

fn rotation(axis: char, degrees: f64) -> Matrix4 {
    let (s, c) = degrees.to_radians().sin_cos();
    match axis {
        'X' => [[1., 0., 0., 0.], [0., c, -s, 0.], [0., s, c, 0.], [0., 0., 0., 1.]],
        'Y' => [[c, 0., s, 0.], [0., 1., 0., 0.], [-s, 0., c, 0.], [0., 0., 0., 1.]],
        _ => [[c, -s, 0., 0.], [s, c, 0., 0.], [0., 0., 1., 0.], [0., 0., 0., 1.]],
    }
}
