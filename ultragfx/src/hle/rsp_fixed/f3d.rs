//! Position transform and clipping from `n64decomp/sm64`'s `rsp/fast3d.s` (CC0-1.0).

use super::*;

pub(super) fn vertex(state: &VertexState, position: [i16; 3]) -> Vertex {
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
    vu.mudn(row(0, true), splat(position[0] as u16));
    vu.madh(row(0, false), splat(position[0] as u16));
    vu.madn(row(1, true), splat(position[1] as u16));
    vu.madh(row(1, false), splat(position[1] as u16));
    vu.madn(row(2, true), splat(position[2] as u16));
    vu.madh(row(2, false), splat(position[2] as u16));
    let pos_f = vu.madn(row(3, true), ONE);
    let pos_i = vu.madh(row(3, false), ONE);
    store(state, pos_i, pos_f)
}

fn reciprocal(vu: &mut Vu, i: V, f: V) -> (V, V) {
    let (mut inv_i, mut inv_f) = (ZERO, ZERO);
    vu.rcph(&mut inv_i, 3, i, 3);
    vu.rcp(&mut inv_f, 3, f, 3, true);
    vu.rcph(&mut inv_i, 3, i, 7);
    vu.rcp(&mut inv_f, 7, f, 7, true);
    vu.rcph(&mut inv_i, 7, ZERO, 0);
    vu.mudn(inv_f, splat(2));
    inv_i = vu.madh(inv_i, splat(2));
    inv_f = vu.madn(ZERO, ZERO);
    vu.mudl(inv_f, f);
    vu.madm(inv_i, f);
    let product_f = vu.madn(inv_f, i);
    let product_i = vu.madh(inv_i, i);
    let error_f = vu.subc(ZERO, product_f);
    let error_i = vu.sub(splat(2), product_i);
    vu.mudl(inv_f, error_f);
    vu.madm(inv_i, error_f);
    inv_f = vu.madn(inv_f, error_i);
    inv_i = vu.madh(inv_i, error_i);
    (inv_i, inv_f)
}

fn store(state: &VertexState, pos_i: V, pos_f: V) -> Vertex {
    let mut vu = Vu::default();
    let ratio_f = vu.mudn(pos_f, splat(state.clip_ratio));
    let ratio_i = vu.madh(pos_i, splat(state.clip_ratio));
    vu.ch(pos_i, h(pos_i, 3));
    vu.cl(pos_f, h(pos_f, 3));
    let unscaled = clip_bits(vu.vcc_word()) & 0x7070;
    vu.ch(pos_i, h(ratio_i, 3));
    vu.cl(pos_f, h(ratio_f, 3));
    let scaled = clip_bits(vu.vcc_word()) & 0x7070;
    let pn = splat(state.persp_norm);
    vu.mudl(pos_f, pn);
    let norm_i = vu.madm(pos_i, pn);
    let norm_f = vu.madn(ZERO, ZERO);
    let (inv_i, inv_f) = reciprocal(&mut vu, norm_i, norm_f);
    vu.ge(inv_i, ZERO);
    let positive_inv_i = vu.mrg(inv_i, splat(0x7fff));
    vu.mudl(pos_f, h(inv_f, 3));
    vu.madm(pos_i, h(inv_f, 3));
    let projected_f = vu.madn(pos_f, h(positive_inv_i, 3));
    let projected_i = vu.madh(pos_i, h(positive_inv_i, 3));
    vu.mudl(projected_f, pn);
    let projected_i = vu.madm(projected_i, pn);
    let projected_f = vu.madn(ZERO, ZERO);
    let scale: V = std::array::from_fn(|n| match n % 4 {
        1 => clamp16(-i32::from(state.vscale[1])),
        k => state.vscale[k] as u16,
    });
    let offset: V = std::array::from_fn(|n| state.vtrans[n % 4] as u16);
    vu.mudh(offset, ONE);
    let screen_f = vu.madn(projected_f, scale);
    let screen_i = vu.madh(projected_i, scale);
    Vertex {
        position: std::array::from_fn(|n| word(pos_i[n], pos_f[n]) as i32),
        screen: [screen_i[0] as i16, screen_i[1] as i16].map(|v| v.clamp(-4090, 4090)),
        screen_z: [screen_i[2].min(0x7fff), screen_f[2]],
        inv_w: [inv_i[3], inv_f[3]],
        clip: scaled << 16 | unscaled,
        st: [0; 2],
        color: [0; 4],
    }
}

pub(super) fn intersection(
    state: &VertexState,
    plane: [i16; 4],
    inside: Vertex,
    outside: Vertex,
) -> Vertex {
    // fast3d.s:1532–1589: interpolate position only, omitting fog, ST and shade.
    let mut vu = Vu::default();
    let plane = std::array::from_fn(|n| plane[n % 4] as u16);
    let negative = vu.mudh(plane, splat(0xffff));
    let v4 = std::array::from_fn(|n| (inside.position[n % 4] >> 16) as u16);
    let v5 = std::array::from_fn(|n| inside.position[n % 4] as u16);
    let v9 = std::array::from_fn(|n| (outside.position[n % 4] >> 16) as u16);
    let v10 = std::array::from_fn(|n| outside.position[n % 4] as u16);
    vu.mudn(v5, plane);
    let mut v11 = vu.madh(v4, plane);
    let mut v12 = vu.madn(ZERO, ZERO);
    vu.madn(v10, negative);
    let mut v29 = vu.madh(v9, negative);
    let v28 = vu.madn(ZERO, ZERO);
    let v26 = vu.addc(v28, q(v28, 0));
    let v27 = vu.add(v29, q(v29, 0));
    let mut v28 = vu.addc(v26, h(v26, 1));
    v29 = vu.add(v27, h(v27, 1));
    let (mut v3, mut v7) = (ZERO, ZERO);
    vu.rcph(&mut v7, 3, v29, 3);
    vu.rcp(&mut v3, 3, v28, 3, true);
    vu.rcph(&mut v7, 3, ZERO, 0);
    v3 = vu.mudn(v3, splat(2));
    v7 = vu.madh(v7, splat(2));
    if (v29[3] as i16) < 0 {
        v3 = vu.mudn(v3, splat(0xffff));
        v7 = vu.madh(v7, splat(0xffff));
    }
    vu.eq(v7, ZERO);
    v3 = vu.mrg(v3, splat(0xffff));
    vu.mudl(v28, e(v3, 3));
    v29 = vu.madm(v29, e(v3, 3));
    v28 = vu.madn(ZERO, ZERO);
    let (v27, v26) = reciprocal(&mut vu, v29, v28);
    v28 = vu.addc(v12, q(v12, 0));
    v29 = vu.add(v11, q(v11, 0));
    v12 = vu.addc(v28, h(v28, 1));
    v11 = vu.add(v29, h(v29, 1));
    vu.mudl(v12, v26);
    vu.madm(v11, v26);
    let v15 = vu.madn(v12, v27);
    let v8 = vu.madh(v11, v27);
    vu.mudl(ZERO, splat(4));
    vu.madl(v15, e(v3, 3));
    let v8 = vu.madm(v8, e(v3, 3));
    let mut v15 = vu.madn(ZERO, ZERO);
    vu.eq(v8, ZERO);
    v15 = vu.mrg(v15, splat(0xffff));
    let outside_weight = v15[3].max(1);
    let inside_weight = outside_weight.wrapping_neg();
    vu.mudl(v5, splat(inside_weight));
    vu.madm(v4, splat(inside_weight));
    vu.madl(v10, splat(outside_weight));
    let pos_i = vu.madm(v9, splat(outside_weight));
    let pos_f = vu.madn(ZERO, ZERO);
    store(state, pos_i, pos_f)
}
