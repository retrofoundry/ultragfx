//! Row-vector matrix math: position is a row, transformed as `pos * M`.
//! Matches hlslpp (`tfPos = mul(float4(x,y,z,1), mvp)`).

pub type Mat4 = [[f32; 4]; 4];

pub fn identity() -> Mat4 {
    [
        [1.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ]
}

/// The identity as the RSP's 16 s15.16 words, row-major.
pub const FX_IDENTITY: [i32; 16] = [
    0x10000, 0, 0, 0, 0, 0x10000, 0, 0, 0, 0, 0x10000, 0, 0, 0, 0, 0x10000,
];

/// The RSP's 48-bit accumulator.
fn acc48(acc: i64) -> i64 {
    acc << 16 >> 16
}

/// `mtx_multiply` in f3dex2.s: each term floored to s15.16 before the sum (`vmadl` keeps the
/// high half of frac×frac), the integer half read out with `vmadh`'s s16 clamp and the fraction
/// with `vmadn`'s. Rows 2 and 3 read their fraction at the last term's `vmadn`, before its
/// int×int `vmadh`, so it is clamped against that partial sum.
pub fn mul_fx(a: [i32; 16], b: [i32; 16]) -> [i32; 16] {
    std::array::from_fn(|ij| {
        let (i, j) = (ij / 4, ij % 4);
        let full = acc48(
            (0..4)
                .map(|k| (i64::from(a[i * 4 + k]) * i64::from(b[k * 4 + j])) >> 16)
                .sum(),
        );
        let early = if i < 2 {
            full
        } else {
            acc48(full - ((i64::from(a[i * 4 + 3] >> 16) * i64::from(b[12 + j] >> 16)) << 16))
        };
        let frac = match early >> 16 {
            h if h > 0x7fff => 0xffff,
            h if h < -0x8000 => 0,
            _ => early & 0xffff,
        };
        (((full >> 16).clamp(-0x8000, 0x7fff) << 16) | frac) as i32
    })
}

/// `VTX_W_INT`: W summed exactly from the MVP's last column, its integer half clamped to s16.
pub fn w_int_fx(pos: [i16; 3], m: [i32; 16]) -> i16 {
    let acc = i64::from(m[15])
        + (0..3)
            .map(|r| i64::from(pos[r]) * i64::from(m[r * 4 + 3]))
            .sum::<i64>();
    (acc48(acc) >> 16).clamp(i16::MIN.into(), i16::MAX.into()) as i16
}

/// Row-vector * matrix: out[c] = sum_k v[k] * m[k][c].
#[cfg_attr(not(test), allow(dead_code))]
pub fn mul_row_vec4(v: [f32; 4], m: Mat4) -> [f32; 4] {
    let mut out = [0.0f32; 4];
    for c in 0..4 {
        out[c] = v[0] * m[0][c] + v[1] * m[1][c] + v[2] * m[2][c] + v[3] * m[3][c];
    }
    out
}

/// Column-vector product on the upper 3×3: out[i] = sum_k m[i][k] * v[k] (M·v).
///
/// Our matrices are stored row-major and pre-transposed for the row-vector POINT path
/// (`mul_row_vec4(pos, M)` is the forward object→clip transform). For a DIRECTION that must be
/// brought from eye/world space INTO object space (light/lookat dirs, for the object-space N·L),
/// the needed transform is the INVERSE rotation — which, with that storage, is exactly this
/// column-vector multiply (`mul_row_vec4` would instead apply the forward rotation, co-rotating
/// the light with the geometry). Translation (column 3) is intentionally ignored.
pub fn mul_col_vec3(m: Mat4, v: [f32; 3]) -> [f32; 3] {
    [
        m[0][0] * v[0] + m[0][1] * v[1] + m[0][2] * v[2],
        m[1][0] * v[0] + m[1][1] * v[1] + m[1][2] * v[2],
        m[2][0] * v[0] + m[2][1] * v[1] + m[2][2] * v[2],
    ]
}

/// Matrix product a*b (row-vector convention: (v*a)*b == v*(a*b)).
pub fn mul4(a: Mat4, b: Mat4) -> Mat4 {
    let mut out = [[0.0f32; 4]; 4];
    for i in 0..4 {
        for j in 0..4 {
            out[i][j] =
                a[i][0] * b[0][j] + a[i][1] * b[1][j] + a[i][2] * b[2][j] + a[i][3] * b[3][j];
        }
    }
    out
}
