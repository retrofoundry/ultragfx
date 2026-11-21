//! F3DZEX NoN 2.06H's fixed-point vertex write and triangle setup, transcribed instruction by
//! instruction from the Mr-Wiseguy/f3dex2 disassembly (`f3dex2.s` at bd31393, the matching
//! build `F3DZEX_NoN_2.06H`: CFG_G_BRANCH_W, CFG_NoN, CFG_OLD_TRI_WRITE). Vector instruction
//! semantics follow ares' RSP interpreter, except that non-multiply instructions leave the
//! accumulator alone: no multiply chain in these routines spans one. Lighting, texgen and fog
//! generation are not modelled.

// RSP vector semantics and reciprocal table adapted from ares (interpreter-vpu.cpp and
// rsp.cpp; licence checked at ef33a804, original revision unrecorded).
// Copyright (c) 2004-2025 ares team, Near et al
//
// Permission to use, copy, modify, and/or distribute this software for any
// purpose with or without fee is hereby granted, provided that the above
// copyright notice and this permission notice appear in all copies.
//
// THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES
// WITH REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF
// MERCHANTABILITY AND FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR
// ANY SPECIAL, DIRECT, INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES
// WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS, WHETHER IN AN
// ACTION OF CONTRACT, NEGLIGENCE OR OTHER TORTIOUS ACTION, ARISING OUT OF
// OR IN CONNECTION WITH THE USE OR PERFORMANCE OF THIS SOFTWARE.

mod f3d;
mod geometry_tests;

type V = [u16; 8];

const ZERO: V = [0; 8];
const ONE: V = [1; 8];

fn splat(value: u16) -> V {
    [value; 8]
}

/// `vt[n]`: lane `n` broadcast.
fn e(v: V, n: usize) -> V {
    splat(v[n])
}

/// `vt[nh]`: lane `n` of each half.
fn h(v: V, n: usize) -> V {
    std::array::from_fn(|i| v[(i & 4) + n])
}

/// `vt[nq]`: lane `n` of each pair.
fn q(v: V, n: usize) -> V {
    std::array::from_fn(|i| v[(i & 6) + n])
}

fn s(v: u16) -> i64 {
    i64::from(v as i16)
}

fn u(v: u16) -> i64 {
    i64::from(v)
}

fn clamp16(v: i32) -> u16 {
    v.clamp(-0x8000, 0x7fff) as i16 as u16
}

static RECIPROCALS: std::sync::LazyLock<[u16; 512]> = std::sync::LazyLock::new(|| {
    std::array::from_fn(|i| {
        if i == 0 {
            0xffff
        } else {
            (((1u64 << 34) / (i as u64 + 512) + 1) >> 8) as u16
        }
    })
});

#[derive(Default)]
struct Vu {
    acc: [i64; 8],
    vcc: [u8; 2],
    vco: [u8; 2],
    vce: u8,
    div_in: u16,
    div_out: u16,
    div_dp: bool,
}

fn bit(mask: u8, n: usize) -> bool {
    mask >> n & 1 != 0
}

impl Vu {
    fn mac(&mut self, f: impl Fn(u16, u16) -> i64, vs: V, vt: V, add: bool) {
        for n in 0..8 {
            let base = if add { self.acc[n] } else { 0 };
            self.acc[n] = (base + f(vs[n], vt[n])) << 16 >> 16;
        }
    }

    fn high(&self) -> V {
        std::array::from_fn(|n| clamp16((self.acc[n] >> 16).clamp(-0x8000, 0x7fff) as i32))
    }

    fn low(&self) -> V {
        std::array::from_fn(|n| match self.acc[n] >> 16 {
            x if x < -0x8000 => 0,
            x if x > 0x7fff => 0xffff,
            _ => self.acc[n] as u16,
        })
    }

    fn sar(&self, shift: u32) -> V {
        std::array::from_fn(|n| (self.acc[n] >> shift) as u16)
    }

    fn mudl(&mut self, vs: V, vt: V) -> V {
        self.mac(|a, b| (u(a) * u(b)) >> 16, vs, vt, false);
        self.sar(0)
    }

    fn madl(&mut self, vs: V, vt: V) -> V {
        self.mac(|a, b| (u(a) * u(b)) >> 16, vs, vt, true);
        self.low()
    }

    fn mudm(&mut self, vs: V, vt: V) -> V {
        self.mac(|a, b| s(a) * u(b), vs, vt, false);
        self.sar(16)
    }

    fn madm(&mut self, vs: V, vt: V) -> V {
        self.mac(|a, b| s(a) * u(b), vs, vt, true);
        self.high()
    }

    fn mudn(&mut self, vs: V, vt: V) -> V {
        self.mac(|a, b| u(a) * s(b), vs, vt, false);
        self.sar(0)
    }

    fn madn(&mut self, vs: V, vt: V) -> V {
        self.mac(|a, b| u(a) * s(b), vs, vt, true);
        self.low()
    }

    fn mudh(&mut self, vs: V, vt: V) -> V {
        self.mac(|a, b| (s(a) * s(b)) << 16, vs, vt, false);
        self.high()
    }

    fn madh(&mut self, vs: V, vt: V) -> V {
        self.mac(|a, b| (s(a) * s(b)) << 16, vs, vt, true);
        self.high()
    }

    fn rcp(&mut self, vd: &mut V, de: usize, vt: V, lane: usize, low: bool) {
        let input = if low && self.div_dp {
            (u32::from(self.div_in) << 16 | u32::from(vt[lane])) as i32
        } else {
            i32::from(vt[lane] as i16)
        };
        let mask = input >> 31;
        let mut data = input ^ mask;
        if input > -32768 {
            data = data.wrapping_sub(mask);
        }
        let result = if data == 0 {
            0x7fff_ffff
        } else if input == -32768 {
            0xffff_0000u32 as i32
        } else {
            let shift = (data as u32).leading_zeros();
            let index = ((u64::from(data as u32) << shift) & 0x7fc0_0000) >> 22;
            let result = ((0x10000 | u32::from(RECIPROCALS[index as usize])) << 14) as i32;
            ((result as u32) >> (31 - shift)) as i32 ^ mask
        };
        self.div_dp = false;
        self.div_out = (result >> 16) as u16;
        vd[de] = result as u16;
    }

    fn rcph(&mut self, vd: &mut V, de: usize, vt: V, lane: usize) {
        self.div_dp = true;
        self.div_in = vt[lane];
        vd[de] = self.div_out;
    }

    fn ge(&mut self, vs: V, vt: V) -> V {
        let mut vcc = 0;
        let out = std::array::from_fn(|n| {
            let (a, b) = (vs[n] as i16, vt[n] as i16);
            let take = a > b || (a == b && !(bit(self.vco[0], n) && bit(self.vco[1], n)));
            vcc |= u8::from(take) << n;
            if take {
                vs[n]
            } else {
                vt[n]
            }
        });
        self.vcc = [vcc, 0];
        self.vco = [0, 0];
        out
    }

    fn lt(&mut self, vs: V, vt: V) -> V {
        let mut vcc = 0;
        let out = std::array::from_fn(|n| {
            let (a, b) = (vs[n] as i16, vt[n] as i16);
            let take = a < b || (a == b && bit(self.vco[0], n) && bit(self.vco[1], n));
            vcc |= u8::from(take) << n;
            if take {
                vs[n]
            } else {
                vt[n]
            }
        });
        self.vcc = [vcc, 0];
        self.vco = [0, 0];
        out
    }

    fn eq(&mut self, vs: V, vt: V) {
        let mut vcc = 0;
        for n in 0..8 {
            vcc |= u8::from(vs[n] == vt[n] && !bit(self.vco[1], n)) << n;
        }
        self.vcc = [vcc, 0];
        self.vco = [0, 0];
    }

    fn mrg(&mut self, vs: V, vt: V) -> V {
        self.vco = [0, 0];
        std::array::from_fn(|n| if bit(self.vcc[0], n) { vs[n] } else { vt[n] })
    }

    fn ch(&mut self, vs: V, vt: V) {
        let (mut vcc, mut vco, mut vce) = ([0u8; 2], [0u8; 2], 0u8);
        for n in 0..8 {
            let (a, b) = (vs[n] as i16, vt[n] as i16);
            let ne = |r: i16| u8::from(r != 0 && vs[n] != !vt[n]) << n;
            if (a ^ b) < 0 {
                let r = a.wrapping_add(b);
                vcc[0] |= u8::from(r <= 0) << n;
                vcc[1] |= u8::from(b < 0) << n;
                vco[0] |= 1 << n;
                vco[1] |= ne(r);
                vce |= u8::from(r == -1) << n;
            } else {
                let r = a.wrapping_sub(b);
                vcc[0] |= u8::from(b < 0) << n;
                vcc[1] |= u8::from(r >= 0) << n;
                vco[1] |= ne(r);
            }
        }
        (self.vcc, self.vco, self.vce) = (vcc, vco, vce);
    }

    fn cl(&mut self, vs: V, vt: V) {
        for n in 0..8 {
            let m = 1 << n;
            if bit(self.vco[0], n) {
                if !bit(self.vco[1], n) {
                    let (sum, carry) = vs[n].overflowing_add(vt[n]);
                    let le = if bit(self.vce, n) {
                        sum == 0 || !carry
                    } else {
                        sum == 0 && !carry
                    };
                    self.vcc[0] = (self.vcc[0] & !m) | (u8::from(le) * m);
                }
            } else if !bit(self.vco[1], n) {
                self.vcc[1] = (self.vcc[1] & !m) | (u8::from(vs[n] >= vt[n]) * m);
            }
        }
        self.vco = [0, 0];
        self.vce = 0;
    }

    fn cr(&mut self, vs: V, vt: V) -> V {
        let mut vcc = [0u8; 2];
        let out = std::array::from_fn(|n| {
            let (a, b) = (vs[n] as i16, vt[n] as i16);
            if (a ^ b) < 0 {
                vcc[1] |= u8::from(b < 0) << n;
                let le = i32::from(a) + i32::from(b) < 0;
                vcc[0] |= u8::from(le) << n;
                if le {
                    !vt[n]
                } else {
                    vs[n]
                }
            } else {
                vcc[0] |= u8::from(b < 0) << n;
                let ge = i32::from(a) - i32::from(b) >= 0;
                vcc[1] |= u8::from(ge) << n;
                if ge {
                    vt[n]
                } else {
                    vs[n]
                }
            }
        });
        self.vcc = vcc;
        self.vco = [0, 0];
        self.vce = 0;
        out
    }

    fn add(&mut self, vs: V, vt: V) -> V {
        let out = std::array::from_fn(|n| {
            clamp16(
                i32::from(vs[n] as i16) + i32::from(vt[n] as i16) + i32::from(bit(self.vco[0], n)),
            )
        });
        self.vco = [0, 0];
        out
    }

    fn addc(&mut self, vs: V, vt: V) -> V {
        let mut carry = 0;
        let out = std::array::from_fn(|n| {
            let (sum, overflow) = vs[n].overflowing_add(vt[n]);
            carry |= u8::from(overflow) << n;
            sum
        });
        self.vco = [carry, 0];
        out
    }

    fn sub(&mut self, vs: V, vt: V) -> V {
        let out = std::array::from_fn(|n| {
            clamp16(
                i32::from(vs[n] as i16) - i32::from(vt[n] as i16) - i32::from(bit(self.vco[0], n)),
            )
        });
        self.vco = [0, 0];
        out
    }

    fn subc(&mut self, vs: V, vt: V) -> V {
        let mut vco = [0u8; 2];
        let out = std::array::from_fn(|n| {
            let r = u32::from(vs[n]).wrapping_sub(u32::from(vt[n]));
            vco[0] |= u8::from(r >> 16 != 0) << n;
            vco[1] |= u8::from(r != 0) << n;
            r as u16
        });
        self.vco = vco;
        out
    }

    fn vcc_word(&self) -> u32 {
        u32::from(self.vcc[1]) << 8 | u32::from(self.vcc[0])
    }
}

/// The DMEM state `G_VTX` reads, as the microcode holds it.
#[derive(Clone, Copy)]
pub(crate) struct VertexState {
    /// The MVP as 16 s15.16 words, row-major (`math::mul_fx` of modelview and projection).
    pub mvp: [i32; 16],
    pub vscale: [i16; 4],
    pub vtrans: [i16; 4],
    pub persp_norm: u16,
    /// `G_MWO_CLIP_RNX`'s low half, which the vertex write scales W by for the clip test.
    pub clip_ratio: u16,
    pub texture_scale: [u16; 2],
}

/// The vertex-buffer fields the triangle setup reads.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Vertex {
    pub position: [i32; 4],
    pub screen: [i16; 2],
    pub screen_z: [u16; 2],
    pub inv_w: [u16; 2],
    pub st: [i16; 2],
    pub color: [u8; 4],
    pub clip: u32,
}

const CLIP_NX: u32 = 1 << 4;
const CLIP_NY: u32 = 1 << 5;
const CLIP_NW: u32 = 1 << 7;
const CLIP_PX: u32 = 1 << 12;
const CLIP_PY: u32 = 1 << 13;
const CLIP_PZ: u32 = 1 << 14;
const CLIP_ALL: u32 = CLIP_NX | CLIP_NY | CLIP_PX | CLIP_PY | CLIP_PZ | CLIP_NW;
const ACTIVE_CLIP_PLANES: u32 = (CLIP_NX | CLIP_NY | CLIP_PX | CLIP_PY) << 16 | CLIP_PZ | CLIP_NW;

fn clip_bits(vcc: u32) -> u32 {
    (vcc << 4) & 0xffff
}

pub(crate) const CLIP_PLANES: [[i16; 4]; 6] = [
    [1, 0, 0, 2],
    [0, 1, 0, 2],
    [1, 0, 0, -2],
    [0, 1, 0, -2],
    [0, 0, 1, -1],
    [0, 0, 0, 1],
];

pub(crate) fn geometry_vertex(f3d: bool, state: &VertexState, position: [i16; 3]) -> Vertex {
    if f3d {
        f3d::vertex(state, position)
    } else {
        vertex(state, position, [0; 2], [0; 4])
    }
}

pub(crate) struct Geometry {
    pub triangles: Vec<[Vertex; 3]>,
    pub clipped: bool,
    pub generated: usize,
}

pub(crate) fn geometry(
    f3d: bool,
    state: &VertexState,
    planes: [[i16; 4]; 6],
    vertices: [Vertex; 3],
    cull: u32,
) -> Geometry {
    let near = if f3d { 1 << 6 } else { CLIP_NW };
    let all = CLIP_NX | CLIP_NY | CLIP_PX | CLIP_PY | CLIP_PZ | near;
    let masks = [
        CLIP_NX << 16,
        CLIP_NY << 16,
        CLIP_PX << 16,
        CLIP_PY << 16,
        CLIP_PZ,
        near,
    ];
    let active = masks.into_iter().fold(0, |a, b| a | b);
    let mut result = Geometry {
        triangles: Vec::new(),
        clipped: false,
        generated: 0,
    };
    if vertices[0].clip & vertices[1].clip & vertices[2].clip & all != 0 {
        return result;
    }
    let mut polygon = vertices.to_vec();
    if vertices.iter().any(|v| v.clip & active != 0) {
        result.clipped = true;
        if f3d {
            polygon.reverse();
        }
        for plane in (0..6).rev() {
            let mut output = Vec::with_capacity(9);
            let mut start = *polygon.last().unwrap();
            for &end in &polygon {
                let start_out = start.clip & masks[plane] != 0;
                let end_out = end.clip & masks[plane] != 0;
                if start_out != end_out {
                    let (inside, outside) = if start_out {
                        (end, start)
                    } else {
                        (start, end)
                    };
                    output.push(if f3d {
                        f3d::intersection(state, planes[plane], inside, outside)
                    } else {
                        intersection(state, planes[plane], inside, outside)
                    });
                    result.generated += 1;
                }
                if !end_out {
                    output.push(end);
                }
                start = end;
            }
            if output.len() < 3 {
                return result;
            }
            assert!(output.len() <= 9, "clip polygon exceeds source buffer");
            polygon = output;
        }
    }
    for i in 1..polygon.len() - 1 {
        let fan = if f3d && result.clipped {
            [polygon[i + 1], polygon[i], polygon[0]]
        } else {
            [polygon[i - 1], polygon[i], *polygon.last().unwrap()]
        };
        if fan[0].clip & fan[1].clip & fan[2].clip & all != 0 {
            continue;
        }
        let d = |i: usize, j: usize, axis: usize| {
            i64::from(
                (i32::from(fan[i].screen[axis]) - i32::from(fan[j].screen[axis]))
                    .clamp(-32768, 32767),
            )
        };
        let area = d(0, 1, 0) * d(0, 2, 1) + d(0, 2, 0) * d(1, 0, 1);
        if area == 0 || cull == 3 || cull == 1 && area < 0 || cull == 2 && area > 0 {
            continue;
        }
        result.triangles.push(fan);
    }
    result
}

/// `vertices_process_pair` and `vertices_store` (f3dex2.s 1293-1477) for one unlit vertex.
pub(crate) fn vertex(
    state: &VertexState,
    position: [i16; 3],
    st: [i16; 2],
    color: [u8; 4],
) -> Vertex {
    let mut vu = Vu::default();
    let row = |r: usize, frac: bool| -> V {
        std::array::from_fn(|n| {
            let word = state.mvp[r * 4 + n % 4];
            if frac {
                word as u16
            } else {
                (word >> 16) as u16
            }
        })
    };
    let [x, y, z] = position.map(|c| c as u16);
    let v20: V = [x, y, z, 0, x, y, z, 0];

    vu.mudn(row(3, true), ONE);
    vu.madh(row(3, false), ONE);
    vu.madn(row(0, true), h(v20, 0));
    vu.madh(row(0, false), h(v20, 0));
    vu.madn(row(1, true), h(v20, 1));
    vu.madh(row(1, false), h(v20, 1));
    let pos_f = vu.madn(row(2, true), h(v20, 2));
    let pos_i = vu.madh(row(2, false), h(v20, 2));
    let scaled = |c: i16, scale: u16| ((i32::from(c) * i32::from(scale)) >> 16) as i16;
    store_vertex(
        state,
        pos_i,
        pos_f,
        [
            scaled(st[0], state.texture_scale[0]),
            scaled(st[1], state.texture_scale[1]),
        ],
        color,
    )
}

fn store_vertex(state: &VertexState, pos_i: V, pos_f: V, st: [i16; 2], color: [u8; 4]) -> Vertex {
    let mut vu = Vu::default();
    let pn = splat(state.persp_norm);
    let ratio = splat(state.clip_ratio);
    let scale: V = {
        let s = state.vscale.map(|c| c as u16);
        let neg = clamp16(-i32::from(state.vscale[1]));
        [s[0], neg, s[2], 0, s[0], neg, s[2], 0]
    };
    let offset: V = {
        let t = state.vtrans.map(|c| c as u16);
        [t[0], t[1], t[2], 0, t[0], t[1], t[2], 0]
    };

    vu.mudl(pos_f, pn);
    let mut v2 = vu.madm(pos_i, pn);
    let mut v21 = vu.madn(ZERO, ZERO);
    let v7 = vu.mudn(pos_f, ratio);
    let v6 = vu.madh(pos_i, ratio);
    let (mut v4, mut v5, mut scratch) = (ZERO, ZERO, ZERO);
    vu.rcph(&mut scratch, 0, v2, 3);
    vu.rcp(&mut v5, 3, v21, 3, true);
    vu.rcph(&mut v4, 3, v2, 7);
    vu.ch(pos_i, h(pos_i, 3));
    vu.cl(pos_f, h(pos_f, 3));
    let screen_clip = clip_bits(vu.vcc_word());
    vu.mudl(v21, v5);
    vu.madm(v2, v5);
    v21 = vu.madn(v21, v4);
    v2 = vu.madh(v2, v4);
    vu.ge(pos_i, ZERO);
    vu.mudh(ONE, splat(4));
    let v26 = vu.madn(v21, splat(-4i16 as u16));
    let v25 = vu.madh(v2, splat(-4i16 as u16));
    v2 = vu.mrg(ZERO, splat(0x7fff));
    vu.ch(pos_i, h(v6, 3));
    vu.mudl(v26, v5);
    vu.madm(v25, v5);
    v5 = vu.madn(v26, v4);
    v4 = vu.madh(v25, v4);
    v2 = vu.madh(v2, splat(0x7fff));
    vu.cl(pos_f, h(v7, 3));
    let scaled_clip = clip_bits(vu.vcc_word());
    vu.mudl(pos_f, h(v5, 3));
    vu.madm(pos_i, h(v5, 3));
    let v26 = vu.madn(pos_f, h(v2, 3));
    let v25 = vu.madh(pos_i, h(v2, 3));
    vu.mudl(v26, pn);
    let v25 = vu.madm(v25, pn);
    let v26 = vu.madn(ZERO, ZERO);
    vu.mudh(offset, ONE);
    let v26 = vu.madn(v26, scale);
    let v25 = vu.madh(v25, scale);
    let z = vu.ge(v25, ZERO);

    Vertex {
        position: std::array::from_fn(|n| word(pos_i[n], pos_f[n]) as i32),
        screen: [v25[0] as i16, v25[1] as i16],
        screen_z: [z[2], v26[2]],
        inv_w: [v4[3], v5[3]],
        st,
        color,
        clip: scaled_clip << 16 | screen_clip,
    }
}

pub(crate) enum Triangle {
    /// Wholly past one screen plane, back-facing, or of zero area by the fixed-point cross
    /// product: the RSP sends nothing.
    Culled,
    /// Crossing an active plane: run `clipped_triangles` with the live viewport and clip planes.
    Clipped,
    /// Geometry-mode bits index beyond the microcode's cull table.
    Unsupported,
    /// The RDP command words: edges, then shade and texture coefficients as enabled. Depth
    /// coefficients are not modelled.
    Drawn(Vec<u32>),
}

/// Overlay 3 (f3dex2.s 1074-1252), after `triangle` requests clipping.
pub(crate) fn clipped_triangles(
    state: &VertexState,
    planes: [[i16; 4]; 6],
    vertices: [Vertex; 3],
    geometry_mode: u32,
    texture: Option<u8>,
) -> Vec<([Vertex; 3], Vec<u32>)> {
    let masks = [
        CLIP_NX << 16,
        CLIP_NY << 16,
        CLIP_PX << 16,
        CLIP_PY << 16,
        CLIP_PZ,
        CLIP_NW,
    ];
    let mut polygon = vertices.to_vec();
    for plane in (0..6).rev() {
        let mut output = Vec::new();
        let mut start = *polygon.last().unwrap();
        for &end in &polygon {
            let start_outside = start.clip & masks[plane] != 0;
            let end_outside = end.clip & masks[plane] != 0;
            if start_outside != end_outside {
                let (inside, outside) = if start_outside {
                    (end, start)
                } else {
                    (start, end)
                };
                output.push(intersection(state, planes[plane], inside, outside));
            }
            if !end_outside {
                output.push(end);
            }
            start = end;
        }
        if output.len() < 3 {
            return Vec::new();
        }
        polygon = output;
    }
    if geometry_mode & G_SHADING_SMOOTH == 0 {
        // orig_v1_addr survives overlay 3; alpha still comes from each fan vertex.
        for vertex in &mut polygon {
            vertex.color[..3].copy_from_slice(&vertices[0].color[..3]);
        }
    }
    let mut triangles = Vec::new();
    for pair in polygon[..polygon.len() - 1].windows(2) {
        let fan = [pair[0], pair[1], *polygon.last().unwrap()];
        if let Triangle::Drawn(words) = setup_triangle(fan, geometry_mode, texture, 0) {
            triangles.push((fan, words));
        }
    }
    triangles
}

fn intersection(state: &VertexState, plane: [i16; 4], inside: Vertex, outside: Vertex) -> Vertex {
    let mut vu = Vu::default();
    let mut v2 = std::array::from_fn(|n| plane[n % 4] as u16);
    let mut v3 = vu.mudh(v2, splat(0xffff));
    let v4 = std::array::from_fn(|n| inside.position[n % 4] as u16);
    let v5 = std::array::from_fn(|n| (inside.position[n % 4] >> 16) as u16);
    let v6 = std::array::from_fn(|n| outside.position[n % 4] as u16);
    let v7 = std::array::from_fn(|n| (outside.position[n % 4] >> 16) as u16);
    let mut v8 = vu.mudn(v4, v2);
    let mut v9 = vu.madh(v5, v2);
    let mut v10 = vu.madn(v6, v3);
    let mut v11 = vu.madh(v7, v3);
    v8 = vu.addc(v8, q(v8, 0));
    v9 = vu.add(v9, q(v9, 0));
    v10 = vu.addc(v10, q(v10, 0));
    v11 = vu.add(v11, q(v11, 0));
    v8 = vu.addc(v8, h(v8, 1));
    v9 = vu.add(v9, h(v9, 1));
    v10 = vu.addc(v10, h(v10, 1));
    v11 = vu.add(v11, h(v11, 1));
    let sign = if ((v11[3] | 1) as i16) < 0 {
        -2i16 as u16
    } else {
        2
    };
    vu.rcph(&mut v3, 3, v11, 3);
    vu.rcp(&mut v2, 3, v10, 3, true);
    vu.rcph(&mut v3, 3, ZERO, 0);
    v2 = vu.mudn(v2, splat(sign));
    v3 = vu.madh(v3, splat(sign));
    vu.eq(v3, ZERO);
    v2 = vu.mrg(v2, splat(0xffff));
    vu.mudl(v10, e(v2, 3));
    v11 = vu.madm(v11, e(v2, 3));
    v10 = vu.madn(ZERO, ZERO);
    let (mut v12, mut v13) = (ZERO, ZERO);
    vu.rcph(&mut v13, 3, v11, 3);
    vu.rcp(&mut v12, 3, v10, 3, true);
    vu.rcph(&mut v13, 3, ZERO, 0);
    vu.mudl(v12, v10);
    vu.madm(v13, v10);
    v10 = vu.madn(v12, v11);
    v11 = vu.madh(v13, v11);
    vu.mudh(ONE, splat(4));
    v10 = vu.madn(v10, splat(-4i16 as u16));
    v11 = vu.madh(v11, splat(-4i16 as u16));
    vu.mudl(v12, v10);
    vu.madm(v13, v10);
    v12 = vu.madn(v12, v11);
    v13 = vu.madh(v13, v11);
    vu.mudl(v8, v12);
    vu.madm(v9, v12);
    v10 = vu.madn(v8, v13);
    v11 = vu.madh(v9, v13);
    vu.mudl(v10, e(v2, 3));
    v11 = vu.madm(v11, e(v2, 3));
    v10 = vu.madn(v10, ZERO);
    v11 = vu.lt(v11, ONE);
    v10 = vu.mrg(v10, splat(0xffff));
    vu.subc(v10, ONE);
    vu.ge(v11, ZERO);
    v10 = vu.mrg(v10, ONE);
    v2 = vu.mudn(v10, splat(0xffff));
    vu.mudl(v6, e(v10, 3));
    vu.madm(v7, e(v10, 3));
    vu.madl(v4, e(v2, 3));
    let pos_i = vu.madm(v5, e(v2, 3));
    let pos_f = vu.madn(ZERO, ZERO);
    let attributes = |v: Vertex| -> V {
        let [r, g, b, a] = v.color.map(|c| u16::from(c) << 7);
        [r, g, b, a, v.st[0] as u16, v.st[1] as u16, 0, 0]
    };
    vu.mudm(attributes(outside), e(v10, 3));
    let attrs = vu.madm(attributes(inside), e(v2, 3));
    store_vertex(
        state,
        pos_i,
        pos_f,
        [attrs[4] as i16, attrs[5] as i16],
        std::array::from_fn(|n| (attrs[n] >> 7) as u8),
    )
}

const G_SHADE: u32 = 0x4;
const G_SHADING_SMOOTH: u32 = 0x0020_0000;
const CULL_MAGIC: [u16; 5] = [0xffff, 0x8000, 0x0000, 0x0000, 0x8000];
const V30: V = [
    0x7ffc,
    0x1400,
    0x01cc,
    0x0200,
    -16i16 as u16,
    0x0010,
    0x0020,
    0x0100,
];

fn word(i: u16, f: u16) -> u32 {
    u32::from(i) << 16 | u32::from(f)
}

/// `tri_to_rdp` (f3dex2.s 1515-1955) for vertices in command order. `texture` is the
/// `G_TEXTURE` tile and level byte when texturing is on.
pub(crate) fn triangle(v: [Vertex; 3], geometry_mode: u32, texture: Option<u8>) -> Triangle {
    setup_triangle(v, geometry_mode, texture, ACTIVE_CLIP_PLANES)
}

fn setup_triangle(
    v: [Vertex; 3],
    geometry_mode: u32,
    texture: Option<u8>,
    active_planes: u32,
) -> Triangle {
    let mut vu = Vu::default();
    let position = |k: usize| -> V {
        let [x, y] = v[k].screen;
        [x as u16, y as u16, 0, 0, 0, 0, k as u16, 0]
    };
    let color = |k: usize| -> V {
        let [r, g, b, a] = v[k].color;
        let [s, t] = v[k].st.map(|c| (c as u16).to_be_bytes());
        [r, g, b, a, s[0], s[1], t[0], t[1]].map(|byte| u16::from(byte) << 8)
    };
    let id = |p: V| usize::from(p[6]);
    let v0x4000 = splat(0x4000);
    let mut a1f = splat(0x8000);
    let (p1, p2, p3) = (position(0), position(1), position(2));
    let mut a2f = splat(0x8000);
    let mut a3f = splat(0x8000);
    let v2 = vu.add(ZERO, e(p1, 1));
    let d12 = vu.sub(p1, p2);
    let d21 = vu.sub(p2, p1);
    let d13 = vu.sub(p1, p3);
    let v13 = vu.lt(v2, e(p2, 1));
    let v14 = vu.mrg(p1, p2);
    if v[0].clip & v[1].clip & v[2].clip & CLIP_ALL != 0 {
        return Triangle::Culled;
    }
    vu.mudh(d12, e(d13, 1));
    let cross = vu.madh(d13, e(d21, 1));
    let v2 = vu.ge(v2, e(p2, 1));
    let v10 = vu.mrg(p1, p2);
    let v6 = vu.ge(v13, e(p3, 1));
    let v4 = vu.mrg(v14, p3);
    let pos_h = vu.mrg(p3, v14);
    if (v[0].clip | v[1].clip | v[2].clip) & active_planes != 0 {
        return Triangle::Clipped;
    }
    let cull = (geometry_mode >> 8 & 0xff) as usize / 2;
    let Some(&[high, low]) = CULL_MAGIC.get(cull..cull + 2) else {
        return Triangle::Unsupported;
    };
    let magic = word(high, low);
    if (i32::from(cross[0] as i16)).wrapping_add(magic as i32) >= 0 || cross[0] == 0 {
        return Triangle::Culled;
    }
    vu.lt(v6, v2);
    let pos_m = vu.mrg(v4, v10);
    let pos_l = vu.mrg(v10, v4);
    let (top, mid, low) = (v[id(pos_h)], v[id(pos_m)], v[id(pos_l)]);
    let y_spx_f = vu.mudn(pos_h, v0x4000);
    let diff_mh = vu.sub(pos_m, pos_h);
    let diff_lh = vu.sub(pos_l, pos_h);
    let diff_hm = vu.sub(pos_h, pos_m);
    let diff_hl = vu.sub(pos_h, pos_l);
    let mut dx = vu.sub(pos_l, pos_m);
    let diff_lm = dx;
    vu.mudh(diff_mh, e(diff_lh, 0));
    vu.madh(diff_lh, e(diff_hm, 0));
    let cross_i = vu.sar(32);
    let cross_f = vu.sar(16);

    let (mut inv_dy_f, mut inv_dy_i, mut inv_cross_f, mut inv_cross_i) = (ZERO, ZERO, ZERO, ZERO);
    let alpha = |mut attrs: V, k: usize| {
        attrs[3] = u16::from(v[k].color[3]) << 8;
        attrs
    };
    let (mut a1i, mut a2i, mut a3i) = if geometry_mode & G_SHADING_SMOOTH != 0 {
        (color(id(pos_h)), color(id(pos_m)), color(id(pos_l)))
    } else {
        let flat = |k| alpha(color(0), k);
        (flat(id(pos_h)), flat(id(pos_m)), flat(id(pos_l)))
    };
    dx[2] = diff_mh[0];
    vu.rcp(&mut inv_dy_f, 0, diff_lm, 1, false);
    vu.rcph(&mut inv_dy_i, 0, cross_i, 1);
    vu.rcp(&mut inv_cross_f, 1, cross_f, 1, true);
    vu.rcph(&mut inv_cross_i, 1, ZERO, 0);
    vu.rcp(&mut inv_dy_f, 2, diff_mh, 1, false);
    vu.rcph(&mut inv_dy_i, 2, diff_mh, 1);
    vu.rcp(&mut inv_dy_f, 3, diff_lh, 1, false);
    vu.rcph(&mut inv_dy_i, 3, diff_lh, 1);
    a1i = vu.mudl(a1i, splat(0x100));
    a2i = vu.mudl(a2i, splat(0x100));
    a3i = vu.mudl(a3i, splat(0x100));
    dx[3] = diff_lh[0];
    vu.mudl(inv_dy_f, splat(8));
    let inv_dy_hi = vu.madm(inv_dy_i, splat(8));
    let inv_dy_lo = vu.madn(ZERO, ZERO);
    let dx_hi = vu.mudm(dx, splat(0x4000));
    let dx_lo = vu.madn(ZERO, ZERO);
    let y_spx_f = vu.subc(ZERO, y_spx_f);
    let y_spx_i = vu.sub(ZERO, ZERO);
    vu.mudm(dx_hi, inv_dy_lo);
    vu.madl(dx_lo, inv_dy_lo);
    let dxdy_f = vu.madn(dx_lo, inv_dy_hi);
    let dxdy_i = vu.madh(dx_hi, inv_dy_hi);
    let left = u32::from(cross_i[1] & 0x80);
    let tile_level = u32::from(texture.unwrap_or(0));
    a2i[7] = mid.screen_z[0];
    a3i[7] = low.screen_z[0];
    vu.mudl(inv_cross_f, cross_f);
    vu.madm(inv_cross_i, cross_f);
    let nr_f = vu.madn(inv_cross_f, cross_i);
    let nr_i = vu.madh(inv_cross_i, cross_i);
    a2f[7] = mid.screen_z[1];
    a3f[7] = low.screen_z[1];
    let command = (geometry_mode | 0xc8 | if texture.is_some() { 2 } else { 0 }) & 0xff;
    let dxdy_i = vu.cr(dxdy_i, splat(0x01cc));
    vu.mudh(ONE, splat(16));
    let nr_f = vu.madn(nr_f, splat(-16i16 as u16));
    let nr_i = vu.madh(nr_i, splat(-16i16 as u16));
    vu.mudn(v0x4000, e(pos_h, 0));
    vu.madl(dxdy_f, e(y_spx_f, 1));
    vu.madm(dxdy_i, e(y_spx_f, 1));
    let xhm_f = vu.madn(dxdy_f, e(y_spx_i, 1));
    let xhm_i = vu.madh(dxdy_i, e(y_spx_i, 1));

    if texture.is_some() {
        let inv_w_all: V = [
            top.inv_w[0],
            top.inv_w[1],
            0,
            0,
            mid.inv_w[0],
            mid.inv_w[1],
            low.inv_w[0],
            low.inv_w[1],
        ];
        let [w1, w2, w3] = [top, mid, low].map(|vertex| word(vertex.inv_w[0], vertex.inv_w[1]));
        let max = |a: u32, b: u32| {
            let d = a.wrapping_sub(b);
            a.wrapping_sub(d & ((d as i32 >> 31) as u32))
        };
        let max_inv_w = max(max(w1, w2), w3);
        let max_inv_w: V = [(max_inv_w >> 16) as u16, max_inv_w as u16, 0, 0, 0, 0, 0, 0];
        let mut scratch = ZERO;
        let (mut inv_max_f, mut inv_max_i) = (ZERO, ZERO);
        vu.rcph(&mut scratch, 0, max_inv_w, 0);
        vu.rcp(&mut inv_max_f, 0, max_inv_w, 1, true);
        let inv_w_all_f = vu.add(ZERO, q(inv_w_all, 1));
        vu.rcph(&mut inv_max_i, 0, ZERO, 0);
        let mut stw12_i = splat(0x7fff);
        vu.mudm(inv_w_all, e(inv_max_f, 0));
        vu.madl(inv_w_all_f, e(inv_max_f, 0));
        [stw12_i[0], stw12_i[1]] = top.st.map(|c| c as u16);
        let norm_f = vu.madn(inv_w_all_f, e(inv_max_i, 0));
        [stw12_i[4], stw12_i[5]] = mid.st.map(|c| c as u16);
        let norm_i = vu.madh(inv_w_all, e(inv_max_i, 0));
        let mut stw3_i = splat(0x7fff);
        vu.ge(V30, e(V30, 7));
        [stw3_i[4], stw3_i[5]] = low.st.map(|c| c as u16);
        vu.mudm(stw12_i, h(norm_f, 0));
        let stw12_i = vu.madh(stw12_i, h(norm_i, 0));
        let stw12_f = vu.madn(ZERO, ZERO);
        vu.mudm(stw3_i, e(norm_f, 6));
        let stw3_i = vu.madh(stw3_i, e(norm_i, 6));
        let stw3_f = vu.madn(ZERO, ZERO);
        a2i = vu.mrg(a2i, stw12_i);
        a2f = vu.mrg(a2f, stw12_f);
        a1i[4..].copy_from_slice(&stw12_i[..4]);
        a1f[4..].copy_from_slice(&stw12_f[..4]);
        a3i = vu.mrg(a3i, stw3_i);
        a3f = vu.mrg(a3f, stw3_f);
    }

    vu.mudl(nr_f, inv_cross_f);
    vu.madm(nr_i, inv_cross_f);
    let inv_dx_f = vu.madn(nr_f, inv_cross_i);
    a1f[7] = top.screen_z[1];
    a1i[7] = top.screen_z[0];
    let inv_dx_i = vu.madh(nr_i, inv_cross_i);
    let xl = (i32::from(mid.screen[0]) << 14) as u32;
    let dah_f = vu.subc(a3f, a1f);
    let dah_i = vu.sub(a3i, a1i);
    let dam_f = vu.subc(a2f, a1f);
    let dam_i = vu.sub(a2i, a1i);
    vu.mudn(dah_f, e(diff_mh, 1));
    vu.madh(dah_i, e(diff_mh, 1));
    vu.madn(dam_f, e(diff_hl, 1));
    vu.madh(dam_i, e(diff_hl, 1));
    let dax_f = vu.sar(16);
    let dax_i = vu.sar(32);
    vu.mudn(dam_f, e(diff_lh, 0));
    vu.madh(dam_i, e(diff_lh, 0));
    vu.madn(dah_f, e(diff_hm, 0));
    vu.madh(dah_i, e(diff_hm, 0));
    let day_f = vu.sar(16);
    let day_i = vu.sar(32);
    vu.mudl(dax_f, e(inv_dx_f, 1));
    vu.madm(dax_i, e(inv_dx_f, 1));
    let dadx_f = vu.madn(dax_f, e(inv_dx_i, 1));
    let dadx_i = vu.madh(dax_i, e(inv_dx_i, 1));
    vu.mudl(day_f, e(inv_dx_f, 1));
    vu.madm(day_i, e(inv_dx_f, 1));
    let dady_f = vu.madn(day_f, e(inv_dx_i, 1));
    let dady_i = vu.madh(day_i, e(inv_dx_i, 1));
    vu.madl(dadx_f, e(dxdy_f, 3));
    vu.madm(dadx_i, e(dxdy_f, 3));
    let dade_f = vu.madn(dadx_f, e(dxdy_i, 3));
    let dade_i = vu.madh(dadx_i, e(dxdy_i, 3));
    vu.mudn(a1f, ONE);
    vu.madh(a1i, ONE);
    vu.madl(dade_f, e(y_spx_f, 1));
    vu.madm(dade_i, e(y_spx_f, 1));
    let a1f = vu.madn(dade_f, e(y_spx_i, 1));
    let a1i = vu.madh(dade_i, e(y_spx_i, 1));

    let mut words = vec![
        command << 24 | (left | tile_level) << 16 | u32::from(pos_l[1]),
        word(pos_m[1], pos_h[1]),
        xl,
        word(dxdy_i[0], dxdy_f[0]),
        word(xhm_i[3], xhm_f[3]),
        word(dxdy_i[3], dxdy_f[3]),
        word(xhm_i[2], xhm_f[2]),
        word(dxdy_i[2], dxdy_f[2]),
    ];
    let block = |lanes: std::ops::Range<usize>| -> Vec<u32> {
        let halves: Vec<u16> = [a1i, dadx_i, a1f, dadx_f, dade_i, dady_i, dade_f, dady_f]
            .iter()
            .flat_map(|v| v[lanes.clone()].to_vec())
            .collect();
        halves
            .chunks(2)
            .map(|pair| word(pair[0], pair[1]))
            .collect()
    };
    if geometry_mode & G_SHADE != 0 {
        words.extend(block(0..4));
    }
    if texture.is_some() {
        words.extend(block(4..8));
    }
    Triangle::Drawn(words)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ORTHO: [i32; 16] = [0x200, 0, 0, 0, 0, 0x200, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x10000];
    const ORTHO4: [i32; 16] = [0x80, 0, 0, 0, 0, 0x80, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x10000];
    const PERSP: [i32; 16] = [
        0x4000, 0, 0, 0, 0, 0x4000, 0, 0, 0, 0, 0, 0x4000, 0, 0, 0, 0,
    ];

    #[test]
    fn clipped_triangle_matches_the_console() {
        let state = VertexState {
            mvp: PERSP,
            vscale: [512, 512, 511, 0],
            vtrans: [640, 480, 511, 0],
            persp_norm: 0xffff,
            clip_ratio: 1,
            texture_scale: [0xffff, 0xffff],
        };
        let vertices = [
            ([128, -80, 256], [0, 0]),
            ([384, -432, 768], [0, 512]),
            ([1440, -336, 768], [2816, 256]),
        ]
        .map(|(p, st)| vertex(&state, p, st, [255; 4]));
        assert!(matches!(triangle(vertices, 0, Some(0)), Triangle::Clipped));
        let mut planes = CLIP_PLANES;
        for (plane, ratio) in planes.iter_mut().zip([1, 1, -1, -1]) {
            plane[3] = ratio;
        }
        let output = clipped_triangles(&state, planes, vertices, 0, Some(0));
        let expected = [
            [
                0xca8002e8, 0x02970280, 0x011fc000, 0x00000000, 0x00dfc000, 0x00027600, 0x00dfc000,
                0x000b2140, 0x00000000, 0x3fff01ff, 0x0002ffff, 0xffe00000, 0x00000000, 0x00010000,
                0xa917fee9, 0x8f2f0000, 0x00060002, 0xfe5b0000, 0x00000002, 0xfea90000, 0x8c9cabb1,
                0xb8680000, 0x007eae60, 0x180b0000,
            ],
            [
                0xca8002ff, 0x02e80280, 0x011fc000, 0xfff4dec0, 0x00dfc000, 0x00000000, 0x00dfc000,
                0x00027600, 0x00000000, 0x3fff01ff, 0x0002ffff, 0xffe10000, 0x00000000, 0x00010000,
                0xa94dff0e, 0x13ca0000, 0x00000002, 0xfea70000, 0x00000002, 0xfea70000, 0x0000adfe,
                0xd1090000, 0x0000adfe, 0xd1090000,
            ],
        ];
        assert_eq!(output.len(), expected.len(), "{output:?}");
        for ((vertices, words), expected) in output.iter().zip(expected) {
            assert_eq!(*words, expected, "{vertices:?}");
        }
    }

    /// MVP, vertices as (x, y, z, s, t) in command order, and the console's command words.
    type Case = ([i32; 16], [[i16; 5]; 3], [u32; 24]);

    /// Triangle commands a real NTSC N64 wrote running OoT NTSC 1.0's F3DZEX NoN 2.06H on
    /// synthetic lists: viewport 320x240, perspNorm and texture scale 0xFFFF, texture on, no
    /// shade or depth.
    #[test]
    fn triangles_match_the_console() {
        let cases: [Case; 7] = [
            // B: every x edge lands a quarter pixel left, at 127.75
            (
                ORTHO,
                [
                    [-32, 104, 0, 0, 0],
                    [32, 72, 0, 1024, 512],
                    [32, 104, 0, 1024, 0],
                ],
                [
                    0xca0000c0, 0x00400040, 0x007fc000, 0x0001ffc0, 0x00bfc000, 0x00000000,
                    0x00bfc000, 0xff000040, 0x01ff0000, 0x3fff01ff, 0x00070000, 0x00000000,
                    0x7c010000, 0x00010000, 0xfdf60000, 0x00000000, 0x00000007, 0x00000000,
                    0x00000007, 0x00000000, 0x0000fbf6, 0x00000000, 0x0000fbf6, 0x00000000,
                ],
            ),
            // D: W 64 to 192
            (
                PERSP,
                [
                    [-240, 112, 256, 0, 0],
                    [-336, 208, 768, 1024, 512],
                    [-336, 272, 768, 1024, 0],
                ],
                [
                    0xca800155, 0x012a0100, 0x00680000, 0x00000000, 0x00280000, 0x00030300,
                    0x00280000, 0x00061840, 0x00000000, 0x3fff01ff, 0x0002fffe, 0xff550000,
                    0x00000000, 0x00010000, 0xa951b39e, 0x40410000, 0x00080004, 0xfdfd0000,
                    0x00000007, 0x00000000, 0x03ee010d, 0xc0830000, 0x0000ea19, 0x00000000,
                ],
            ),
            // G: equal W 100, bottom edge at 143.75
            (
                PERSP,
                [
                    [-375, 25, 400, 0, 0],
                    [-375, -75, 400, 0, 512],
                    [-175, -75, 400, 1024, 512],
                ],
                [
                    0xca00023f, 0x023f01c0, 0x00280000, 0x00ffffc0, 0x00280000, 0x00020400,
                    0x00280000, 0x00000000, 0x00000000, 0x3fff01ff, 0x00070000, 0x00000000,
                    0x00000000, 0x00010000, 0xfdfb0000, 0x00000000, 0x00100008, 0x00000000,
                    0x00000008, 0x00000000, 0x1bed0c17, 0x00000000, 0x00000c17, 0x00000000,
                ],
            ),
            // C: W 64 to 192, the other half
            (
                PERSP,
                [
                    [112, 208, 256, 0, 0],
                    [112, 144, 256, 0, 512],
                    [720, 496, 768, 1024, 512],
                ],
                [
                    0xca8000c0, 0x00950040, 0x0117c000, 0xfffa0c00, 0x00d7c000, 0x00000000,
                    0x00d7c000, 0x00030300, 0x00000000, 0x3fff01ff, 0x0002fffe, 0xff550000,
                    0x00000000, 0x00010000, 0xa948ada8, 0x41580000, 0x00000007, 0x00000000,
                    0x00000007, 0x00000000, 0x0000fbf8, 0x00000000, 0x0000fbf8, 0x00000000,
                ],
            ),
            // K1: a quarter pixel off the grid
            (
                ORTHO4,
                [
                    [-479, 223, 0, 0, 0],
                    [-223, 95, 0, 1024, 512],
                    [-223, 223, 0, 1024, 0],
                ],
                [
                    0xca000181, 0x01010101, 0x00280000, 0x0001ffc0, 0x00680000, 0x00000000,
                    0x00a7fff0, 0xff000040, 0x01fffffe, 0x3fff01ff, 0x00070000, 0x00000000,
                    0x7c010102, 0x00010000, 0xfdf60000, 0x00000000, 0x00000007, 0x00000000,
                    0x00000007, 0x00000000, 0x0000fbf6, 0x00000000, 0x0000fbf6, 0x00000000,
                ],
            ),
            // R: equal W 400
            (
                PERSP,
                [
                    [-400, -500, 1600, 0, 0],
                    [400, -900, 1600, 1024, 512],
                    [400, -500, 1600, 1024, 0],
                ],
                [
                    0xca0002fe, 0x027f027f, 0x00800000, 0x000201fc, 0x00bfc000, 0x00000000,
                    0x017effd0, 0xff01003f, 0x01fffff9, 0x3fff01ff, 0x00080000, 0x00000000,
                    0x7c01f6f1, 0x00010000, 0x06010000, 0x00000000, 0x00000008, 0x00000000,
                    0x00000008, 0x00000000, 0x00000c13, 0x00000000, 0x00000c13, 0x00000000,
                ],
            ),
            // V: unclipped under FRUSTRATIO_2
            (
                PERSP,
                [
                    [128, -176, 256, 0, 0],
                    [384, -720, 768, 0, 512],
                    [1440, -624, 768, 2816, 256],
                ],
                [
                    0xca8003bf, 0x037f0340, 0x018fc000, 0xfff500b0, 0x00dfc000, 0x00000000,
                    0x00dfc000, 0x000b2cb0, 0x00000000, 0x3fff01ff, 0x00020000, 0xffe00000,
                    0x00000000, 0x00010000, 0xa9b7005d, 0xb5d70000, 0x00000002, 0xfea70000,
                    0x00000002, 0xfea70000, 0x0000adfd, 0xd0300000, 0x0000adfd, 0xd0300000,
                ],
            ),
        ];
        for (mvp, vertices, expected) in cases {
            let state = VertexState {
                mvp,
                vscale: [512, 512, 511, 0],
                vtrans: [640, 480, 511, 0],
                persp_norm: 0xffff,
                clip_ratio: 2,
                texture_scale: [0xffff, 0xffff],
            };
            let vertices =
                vertices.map(|[x, y, z, s, t]| vertex(&state, [x, y, z], [s, t], [255; 4]));
            let Triangle::Drawn(words) = triangle(vertices, 0, Some(0)) else {
                panic!("not drawn: {vertices:?}");
            };
            assert_eq!(words, expected, "{vertices:?}");
        }
    }
}
