use crate::hle::math::Mat4;

type Basis = [[f32; 3]; 3];

pub(super) fn linear(a: Mat4, b: Mat4, t: f32) -> Mat4 {
    std::array::from_fn(|i| std::array::from_fn(|j| a[i][j] + (b[i][j] - a[i][j]) * t))
}

pub(crate) fn rigid(a: Mat4, b: Mat4, t: f32) -> Option<Mat4> {
    let (sa, qa) = decompose(a)?;
    let (sb, qb) = decompose(b)?;
    let mut result = slerp(qa, qb, t);
    for i in 0..3 {
        let scale = sa[i] + (sb[i] - sa[i]) * t;
        for value in &mut result[i][..3] {
            *value *= scale;
        }
        result[3][i] = a[3][i] + (b[3][i] - a[3][i]) * t;
    }
    Some(result)
}

/// Blends `K · R` per endpoint, where R is the Gram–Schmidt rotation and `K = M · Rᵀ` keeps
/// whatever the guest's matrix has beyond rotation and scale, so no rigidity tolerance applies.
pub(crate) fn residual(a: Mat4, b: Mat4, t: f32) -> Option<Mat4> {
    let (_, ra, qa) = frame(a, false)?;
    let (_, rb, qb) = frame(b, false)?;
    let rotation = slerp(qa, qb, t);
    let stretch =
        |m: Mat4, r: Basis, i: usize, k: usize| (0..3).map(|j| m[i][j] * r[k][j]).sum::<f32>();
    let k: Basis = std::array::from_fn(|i| {
        std::array::from_fn(|k| {
            let ka = stretch(a, ra, i, k);
            ka + (stretch(b, rb, i, k) - ka) * t
        })
    });
    let mut result = rotation;
    for i in 0..3 {
        for j in 0..3 {
            result[i][j] = (0..3).map(|n| k[i][n] * rotation[n][j]).sum();
        }
        result[3][i] = a[3][i] + (b[3][i] - a[3][i]) * t;
    }
    Some(result)
}

fn slerp(mut qa: [f32; 4], qb: [f32; 4], t: f32) -> Mat4 {
    let mut dot: f32 = qa.into_iter().zip(qb).map(|(a, b)| a * b).sum();
    if dot < 0.0 {
        qa = qa.map(|v| -v);
        dot = -dot;
    }
    let (wa, wb) = if dot > 0.9995 {
        (1.0 - t, t)
    } else {
        let theta = dot.clamp(-1.0, 1.0).acos();
        (
            ((1.0 - t) * theta).sin() / theta.sin(),
            (t * theta).sin() / theta.sin(),
        )
    };
    let mut q: [f32; 4] = std::array::from_fn(|i| qa[i] * wa + qb[i] * wb);
    let norm = q.into_iter().map(|v| v * v).sum::<f32>().sqrt();
    q = q.map(|v| v / norm);
    let [x, y, z, w] = q;
    [
        [
            1.0 - 2.0 * (y * y + z * z),
            2.0 * (x * y + z * w),
            2.0 * (x * z - y * w),
            0.0,
        ],
        [
            2.0 * (x * y - z * w),
            1.0 - 2.0 * (x * x + z * z),
            2.0 * (y * z + x * w),
            0.0,
        ],
        [
            2.0 * (x * z + y * w),
            2.0 * (y * z - x * w),
            1.0 - 2.0 * (x * x + y * y),
            0.0,
        ],
        [0.0, 0.0, 0.0, 1.0],
    ]
}

fn decompose(m: Mat4) -> Option<([f32; 3], [f32; 4])> {
    frame(m, true).map(|(scales, _, q)| (scales, q))
}

fn frame(m: Mat4, rigid: bool) -> Option<([f32; 3], Basis, [f32; 4])> {
    if !m.iter().flatten().all(|x| x.is_finite())
        || m[0][3] != 0.0
        || m[1][3] != 0.0
        || m[2][3] != 0.0
        || m[3][3] != 1.0
    {
        return None;
    }
    let scales: [f32; 3] =
        std::array::from_fn(|i| m[i][..3].iter().map(|v| v * v).sum::<f32>().sqrt());
    if scales.contains(&0.0) {
        return None;
    }
    let determinant = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
        - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
    if determinant <= 0.0 {
        return None;
    }
    let mut r: [[f32; 3]; 3] =
        std::array::from_fn(|i| std::array::from_fn(|j| m[i][j] / scales[i]));
    if rigid {
        // Truncating three signed 16.16 components bounds each row's error by sqrt(3) / 2^16.
        let row_error = 3.0_f32.sqrt() / 65536.0;
        for (i, j) in [(0, 1), (0, 2), (1, 2)] {
            let ei = row_error / scales[i];
            let ej = row_error / scales[j];
            let tolerance = ei + ej + ei * ej + 8.0 * f32::EPSILON;
            if tolerance >= 1.0 || (0..3).map(|k| r[i][k] * r[j][k]).sum::<f32>().abs() > tolerance
            {
                return None;
            }
        }
    }
    let projection = (0..3).map(|k| r[0][k] * r[1][k]).sum::<f32>();
    r[1] = std::array::from_fn(|k| r[1][k] - projection * r[0][k]);
    let length = r[1].iter().map(|v| v * v).sum::<f32>().sqrt();
    if length == 0.0 || !length.is_finite() {
        return None;
    }
    r[1] = r[1].map(|v| v / length);
    r[2] = std::array::from_fn(|k| {
        let i = (k + 1) % 3;
        let j = (k + 2) % 3;
        r[0][i] * r[1][j] - r[0][j] * r[1][i]
    });
    let trace = r[0][0] + r[1][1] + r[2][2];
    let q = if trace > 0.0 {
        let s = (trace + 1.0).sqrt() * 2.0;
        [
            (r[1][2] - r[2][1]) / s,
            (r[2][0] - r[0][2]) / s,
            (r[0][1] - r[1][0]) / s,
            s * 0.25,
        ]
    } else {
        let i = if r[0][0] > r[1][1] && r[0][0] > r[2][2] {
            0
        } else if r[1][1] > r[2][2] {
            1
        } else {
            2
        };
        let j = (i + 1) % 3;
        let k = (i + 2) % 3;
        let s = (1.0 + r[i][i] - r[j][j] - r[k][k]).sqrt() * 2.0;
        let mut q = [0.0; 4];
        q[i] = s * 0.25;
        q[j] = (r[i][j] + r[j][i]) / s;
        q[k] = (r[i][k] + r[k][i]) / s;
        q[3] = (r[j][k] - r[k][j]) / s;
        q
    };
    Some((scales, r, q))
}

fn basis_matrix(basis: [[f32; 3]; 3]) -> Mat4 {
    let mut matrix = crate::hle::math::identity();
    for i in 0..3 {
        matrix[i][..3].copy_from_slice(&basis[i]);
    }
    matrix
}

fn fixed(matrix: Mat4) -> Mat4 {
    matrix.map(|row| row.map(|v| (v * 65536.0) as i32 as f32 / 65536.0))
}

pub(crate) fn billboard_model(recipe: crate::annotation::BillboardRecipe) -> Option<Mat4> {
    use crate::{
        annotation::BillboardProvenance,
        hle::math::{identity, mul4},
    };
    let basis = basis_matrix(recipe.basis);
    let (scales, _) = decompose(basis)?;
    if scales.iter().any(|s| (*s - 1.0).abs() > 0.001)
        || !recipe
            .anchor
            .into_iter()
            .chain(recipe.scale)
            .chain([recipe.spin])
            .all(f32::is_finite)
    {
        return None;
    }
    let (s, c) = recipe.spin.sin_cos();
    let spin = [
        [c, s, 0.0, 0.0],
        [-s, c, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ];
    let mut root = identity();
    for (i, scale) in recipe.scale.into_iter().enumerate() {
        root[i][i] = scale;
        root[3][i] = recipe.anchor[i];
    }
    Some(match recipe.provenance {
        BillboardProvenance::Cpu => fixed(mul4(spin, mul4(basis, root))),
        BillboardProvenance::Segment1 => mul4(fixed(basis), fixed(mul4(spin, root))),
    })
}

pub(crate) fn billboard(
    a: crate::annotation::BillboardRecipe,
    b: crate::annotation::BillboardRecipe,
    t: f32,
) -> Option<Mat4> {
    use crate::annotation::BillboardRecipe;
    if a.provenance != b.provenance {
        return None;
    }
    if t == 0.0 {
        return billboard_model(a);
    }
    if t == 1.0 {
        return billboard_model(b);
    }
    let basis = rigid(basis_matrix(a.basis), basis_matrix(b.basis), t)?;
    let turn = std::f32::consts::TAU;
    let delta = (b.spin - a.spin + std::f32::consts::PI).rem_euclid(turn) - std::f32::consts::PI;
    billboard_model(BillboardRecipe {
        anchor: std::array::from_fn(|i| a.anchor[i] + (b.anchor[i] - a.anchor[i]) * t),
        scale: std::array::from_fn(|i| a.scale[i] + (b.scale[i] - a.scale[i]) * t),
        spin: a.spin + delta * t,
        basis: std::array::from_fn(|i| std::array::from_fn(|j| basis[i][j])),
        provenance: a.provenance,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        annotation::{BillboardProvenance, BillboardRecipe},
        hle::math::{identity, mul_row_vec4},
    };

    #[test]
    fn rigid_motion_uses_rotation_scale_and_translation() {
        let a = identity();
        let b = [
            [0.0, 2.0, 0.0, 0.0],
            [-3.0, 0.0, 0.0, 0.0],
            [0.0, 0.0, 4.0, 0.0],
            [8.0, -4.0, 2.0, 1.0],
        ];
        for (t, want) in [
            (0.25, [3.1548494, -0.5216457, 0.5, 1.0]),
            (0.5, [5.0606604, -0.9393398, 1.0, 1.0]),
            (0.75, [6.669696, -1.3832108, 1.5, 1.0]),
        ] {
            let got = mul_row_vec4([1.0, 0.0, 0.0, 1.0], rigid(a, b, t).unwrap());
            for (got, want) in got.into_iter().zip(want) {
                assert!((got - want).abs() < 0.00001, "{t}: {got} != {want}");
            }
        }
    }

    #[test]
    fn quaternion_takes_short_path_across_pi() {
        let turn = |degrees: f32| {
            let (s, c) = degrees.to_radians().sin_cos();
            [
                [c, s, 0.0, 0.0],
                [-s, c, 0.0, 0.0],
                [0.0, 0.0, 1.0, 0.0],
                [0.0, 0.0, 0.0, 1.0],
            ]
        };
        let got = mul_row_vec4(
            [1.0, 0.0, 0.0, 1.0],
            rigid(turn(170.0), turn(-170.0), 0.5).unwrap(),
        );
        assert!((got[0] + 1.0).abs() < 0.00001);
        assert!(got[1].abs() < 0.00001);
    }

    #[test]
    fn small_quantized_models_use_orthonormal_rotation() {
        for deviation in [0.00115_f32, 0.00146] {
            let mut a = identity();
            for (i, row) in a.iter_mut().enumerate().take(3) {
                row[i] = 0.01;
            }
            a[0][1] = 0.01 * deviation / (1.0 - deviation * deviation).sqrt();
            let mut b = a;
            b[3][0] = 4.0;
            assert!(decompose(a).is_some(), "normalized row dot {deviation}");
            for t in [0.25, 0.5, 0.75] {
                let m = rigid(a, b, t).unwrap();
                let rows: [[f32; 3]; 3] = std::array::from_fn(|i| {
                    let length = m[i][..3].iter().map(|x| x * x).sum::<f32>().sqrt();
                    assert!((length - 0.01).abs() < 0.000001);
                    std::array::from_fn(|j| m[i][j] / length)
                });
                for (i, j) in [(0, 1), (0, 2), (1, 2)] {
                    assert!((0..3).map(|k| rows[i][k] * rows[j][k]).sum::<f32>().abs() < 0.000001);
                }
                assert_eq!(m[3][0], 4.0 * t);
            }
            let mut larger = a;
            for row in &mut larger[..3] {
                for value in &mut row[..3] {
                    *value *= 100.0;
                }
            }
            assert!(decompose(larger).is_none());
            let mut unequal = a;
            unequal[1][1] = 1.0;
            unequal[2][2] = 2.0;
            assert!(decompose(unequal).is_some());
        }
        let (s, c) = 0.22_f32.sin_cos();
        let (sx, cx) = 0.51_f32.sin_cos();
        let a = fixed([
            [0.01 * c, 0.01 * s * sx, -0.01 * s * cx, 0.0],
            [0.0, 0.01 * cx, 0.01 * sx, 0.0],
            [0.01 * s, -0.01 * c * sx, 0.01 * c * cx, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ]);
        assert!(rigid(a, a, 0.5).is_some());
    }

    #[test]
    fn small_models_reject_shear_and_reflection() {
        let mut m = identity();
        for (i, row) in m.iter_mut().enumerate().take(3) {
            row[i] = 0.01;
        }
        let mut shear = m;
        shear[0][1] = 0.0001;
        let mut reflection = m;
        reflection[0][0] = -0.01;
        for invalid in [shear, reflection] {
            assert!(decompose(invalid).is_none());
            assert!(rigid(invalid, invalid, 0.5).is_none());
        }
    }

    #[test]
    fn residual_blend_keeps_shear_rigid_rejects() {
        let (s, c) = 0.4_f32.sin_cos();
        let mut a = [
            [0.25 * c, 0.25 * s, 0.0, 0.0],
            [-0.25 * s, 0.25 * c, 0.0, 0.0],
            [0.0, 0.0, 0.25, 0.0],
            [1.0, 2.0, 3.0, 1.0],
        ];
        a[0][2] = 0.01;
        let mut b = a;
        b[3] = [5.0, -2.0, 3.0, 1.0];
        assert!(rigid(a, b, 0.5).is_none());
        for (t, end) in [(0.0, a), (1.0, b)] {
            let m = residual(a, b, t).unwrap();
            for (got, want) in m.iter().flatten().zip(end.iter().flatten()) {
                assert!((got - want).abs() < 1e-6, "{t}: {m:?} != {end:?}");
            }
        }
        let mid = residual(a, b, 0.5).unwrap();
        assert_eq!(mid[3], [3.0, 0.0, 3.0, 1.0]);
        assert!((mid[0][2] - 0.01).abs() < 1e-6);

        let turned = [
            [0.0, 2.0, 0.0, 0.0],
            [-3.0, 0.0, 0.0, 0.0],
            [0.0, 0.0, 4.0, 0.0],
            [8.0, -4.0, 2.0, 1.0],
        ];
        for t in [0.25, 0.5, 0.75] {
            let r = rigid(identity(), turned, t).unwrap();
            let k = residual(identity(), turned, t).unwrap();
            for (r, k) in r.iter().flatten().zip(k.iter().flatten()) {
                assert!((r - k).abs() < 1e-5, "{t}: {r} != {k}");
            }
        }

        let mut reflection = a;
        reflection[2][2] = -0.25;
        assert!(residual(reflection, reflection, 0.5).is_none());
    }

    #[test]
    fn unresolved_quantized_rotations_are_rejected() {
        let q = 1.0_f32 / 65536.0;
        for (scale, off_axis) in [(q, q), (2.0 * q, 2.0 * q), (q, q.powi(6))] {
            let m = [
                [scale, 0.0, 0.0, 0.0],
                [scale, off_axis, 0.0, 0.0],
                [0.0, 0.0, scale, 0.0],
                [0.0, 0.0, 0.0, 1.0],
            ];
            assert!(decompose(m).is_none());
            assert!(rigid(m, m, 0.5).is_none());
        }
    }

    #[test]
    fn billboard_orders_and_fixed_point_endpoints_are_exact() {
        let mut a = BillboardRecipe {
            anchor: [12.0, 4.0, 0.0],
            scale: [2.0, 3.0, 4.0],
            spin: std::f32::consts::FRAC_PI_2,
            basis: [[0.0, 0.0, -1.0], [0.0, 1.0, 0.0], [1.0, 0.0, 0.0]],
            provenance: BillboardProvenance::Cpu,
        };
        let cpu = billboard_model(a).unwrap();
        assert_eq!(
            mul_row_vec4([1.0, 0.0, 0.0, 1.0], cpu),
            [12.0, 7.0, 0.0, 1.0]
        );
        a.provenance = BillboardProvenance::Segment1;
        let segment = billboard_model(a).unwrap();
        assert_eq!(
            mul_row_vec4([1.0, 0.0, 0.0, 1.0], segment),
            [12.0, 4.0, -4.0, 1.0]
        );
        let b = BillboardRecipe {
            anchor: [20.0, 8.0, 4.0],
            scale: [4.0, 5.0, 6.0],
            spin: 0.0,
            basis: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            ..a
        };
        assert_eq!(billboard(a, b, 0.0), Some(segment));
        assert_eq!(billboard(a, b, 1.0), billboard_model(b));
    }
}
