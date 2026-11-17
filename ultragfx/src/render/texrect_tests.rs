use super::guarded_headless_device_forced_fallback as headless_device_forced_fallback;
use super::*;
use crate::hle::{gbi::GbiUcode, mem::RdramImage, SceneOp};
use n64_gbi::encode::*;

#[test]
fn texrect_console_samples() {
    use crate::tests::{
        common::{pixel, render_to_pixels},
        dl_builder::DlBuilder,
    };

    let texture: Vec<_> = (0..32u16)
        .flat_map(|y| {
            (0..32u16).flat_map(move |x| {
                (((x % 2) * 0xf800) | ((y % 2) * 0x07c0) | (((3 * x + 5 * y) % 32) << 1) | 1)
                    .to_be_bytes()
            })
        })
        .collect();
    let (device, queue) = headless_device_forced_fallback();
    // Expected pixels are NTSC console RGBA5551 readbacks from the named cases.
    for (name, cycle, rect, origin, base, step, samples) in [
        (
            "fx-18-bilerp-0555-plain-1",
            0,
            0xe4060060,
            0x00021020,
            0x00180013,
            0x05550555,
            [(9, 8, 0x4c91), (10, 8, 0xec99), (23, 23, 0x6c49)],
        ),
        (
            "fx-24-bilerp-0555-plain-right-3",
            0,
            0xe4063060,
            0x00022020,
            0x00180013,
            0x05550555,
            [(9, 8, 0x948f), (10, 8, 0xb497), (23, 23, 0x1447)],
        ),
        (
            "y-1-flip-1",
            0,
            0xe5060060,
            0x00020021,
            0x00180013,
            0x05550555,
            [(8, 9, 0x1493), (8, 10, 0x949b), (23, 23, 0xb44b)],
        ),
        (
            "signed-flip-0-cycle-0",
            0,
            0xe402602d,
            0x0000500b,
            0x000bfff3,
            0x014dfcf7,
            [(2, 3, 0x8e79), (5, 6, 0x7427), (9, 11, 0xd5c9)],
        ),
        (
            "signed-flip-1-cycle-0",
            0,
            0xe502602d,
            0x0000500b,
            0x000bfff3,
            0x014dfcf7,
            [(2, 3, 0xa7fb), (5, 6, 0x65ab), (9, 11, 0xba95)],
        ),
        (
            "signed-flip-0-cycle-1-combined",
            1,
            0xe402602d,
            0x0000500b,
            0x000bfff3,
            0x014dfcf7,
            [(2, 3, 0x8e79), (5, 6, 0x7427), (9, 11, 0xd5c9)],
        ),
        (
            "signed-flip-1-cycle-1-combined",
            1,
            0xe502602d,
            0x0000500b,
            0x000bfff3,
            0x014dfcf7,
            [(2, 3, 0xa7fb), (5, 6, 0x65ab), (9, 11, 0xba95)],
        ),
        (
            "copy-corner-flip",
            2,
            0xe503c03c,
            0x00020020,
            0x00180018,
            0x10000400,
            [(8, 8, 0x0001), (9, 8, 0xf807), (12, 9, 0x07e3)],
        ),
        (
            "copy-wide-4095-63",
            2,
            0xe40f803c,
            0x00000020,
            0x00010018,
            0x0fff0400,
            [(36, 8, 0x0019), (37, 8, 0xf81f), (62, 15, 0x07fb)],
        ),
        (
            "copy-wide-4095-64",
            2,
            0xe40fc03c,
            0x00000020,
            0x00010018,
            0x0fff0400,
            [(36, 8, 0x0019), (37, 8, 0xf81f), (63, 15, 0xffc1)],
        ),
        (
            "copy-negative-low-bits",
            2,
            0xe40fc03c,
            0x00000020,
            0x00010018,
            0xefff0400,
            [(36, 8, 0x0029), (37, 8, 0xf82f), (63, 15, 0xfff1)],
        ),
        (
            "copy-fractional-x",
            2,
            0xe40fc03c,
            0x00025020,
            0x00010018,
            0x07ff0400,
            [(9, 8, 0x0001), (12, 8, 0xf813), (45, 8, 0x002d)],
        ),
        (
            "copy-fractional-y-plain",
            2,
            0xe40fc03c,
            0x00024021,
            0x00010018,
            0x0fff0555,
            [(9, 8, 0x0001), (13, 8, 0x0019), (45, 9, 0x002d)],
        ),
        (
            "copy-fractional-y-flip",
            2,
            0xe50fc03c,
            0x00024021,
            0x00010018,
            0x0fff0555,
            [(9, 8, 0x0001), (13, 8, 0x0015), (45, 9, 0x0011)],
        ),
        (
            "copy-flip-fractional-step",
            2,
            0xe50fc03c,
            0x00024020,
            0x00180001,
            0x0fff07ff,
            [(45, 8, 0x0035), (46, 8, 0xf83b), (63, 15, 0x0039)],
        ),
    ] {
        let mut builder = DlBuilder::new();
        let address = builder.bytes(8, &texture);
        builder.list(
            "main",
            &[
                gdp_set_color_image(0, 2, 64, 0x100000),
                gdp_set_scissor(0, 0, 0, 256, 256),
                gdp_set_texture_image(0, 2, 32, address),
                gdp_set_tile(0, 2, 8, 0, 7, 0, 0, 0, 0, 0, 0, 0),
                (0xf4000000, 0x0707c07c),
                gdp_set_tile(0, 2, 8, 0, 0, 0, 0, 5, 0, 0, 5, 0),
                gdp_set_tile_size(0, 0, 0, 124, 124),
                if cycle == 2 {
                    (0xef200cf0, 0)
                } else {
                    (0xef002cf0 | (cycle << 20), 0x0f0a4000)
                },
                (0xfcffffff, if cycle == 0 { 0xfffcf279 } else { 0xfffcf238 }),
                (rect, origin),
                (0xe1000000, base),
                (0xf1000000, step),
                gsp_enddl(),
            ],
        );
        let built = builder.finish("main");
        let result = crate::hle::interpret(
            RdramImage::new(&built.rdram),
            u64::from(built.entry),
            GbiUcode::F3dex2,
            crate::DataFormat::Fixed,
            None,
        );
        assert!(result.diags.is_empty(), "{name}: {:?}", result.diags);
        let mut renderer =
            SceneRenderer::new(&device, wgpu::TextureFormat::Rgba8Unorm, 64, 64, false);
        let pixels = render_to_pixels(&device, &queue, &mut renderer, &result.scene, 64, 64);
        for (x, y, expected) in samples {
            let [r, g, b, a] = pixel(&pixels, 64, x, y);
            let packed = (u16::from(r >> 3) << 11)
                | (u16::from(g >> 3) << 6)
                | (u16::from(b >> 3) << 1)
                | u16::from(a >= 128);
            assert_eq!(packed, expected, "{name} ({x},{y})");
        }
    }
}

fn decode(bounds: [u32; 4], base: [i16; 2], step: [i16; 2], cycle: u32, flip: bool) -> SceneOp {
    let [ulx, uly, lrx, lry] = bounds;
    let mut words = vec![
        gdp_set_color_image(0, 2, 32, 0x10000),
        gdp_set_other_mode_h(20, 2, cycle << 20),
        gdp_set_tile(0, 2, 1, 0, 3, 0, 2, 0, 0, 2, 0, 0),
    ];
    words.extend(gsp_texture_rectangle(
        ulx,
        uly,
        lrx,
        lry,
        3,
        base[0] as u32,
        base[1] as u32,
        step[0] as u32,
        step[1] as u32,
        flip,
    ));
    words.push(gsp_enddl());
    let bytes: Vec<_> = words
        .into_iter()
        .flat_map(|(a, b)| [a.to_be_bytes(), b.to_be_bytes()].concat())
        .collect();
    let result = crate::hle::interpret(
        RdramImage::new(&bytes),
        0,
        GbiUcode::F3dex2,
        crate::DataFormat::Fixed,
        None,
    );
    assert!(result.diags.is_empty(), "{:?}", result.diags);
    result.scene.framebuffer_pairs[0].ops[0].clone()
}

fn quad(op: &SceneOp) -> [OutVertex; 6] {
    let SceneOp::TexRect {
        rect,
        uls,
        ult,
        dsdx,
        dtdy,
        flip,
        copy_mode,
        ..
    } = op
    else {
        panic!("expected rectangle")
    };
    texrect_quad(
        rect,
        (*uls, *ult),
        (*dsdx, *dtdy),
        *flip,
        *copy_mode,
        (32, 32),
    )
}

fn bounds(vertices: &[OutVertex; 6]) -> [i32; 4] {
    let tl = vertices[0].position;
    let br = vertices[2].position;
    [
        ((tl[0] + 1.0) * 16.0) as i32,
        ((1.0 - tl[1]) * 16.0) as i32,
        ((br[0] + 1.0) * 16.0) as i32,
        ((1.0 - br[1]) * 16.0) as i32,
    ]
}

fn assert_mask(vertices: &[OutVertex; 6], expected: [i32; 4]) {
    let [l, t, r, b] = bounds(vertices);
    let [el, et, er, eb] = expected;
    for y in 0..32 {
        for x in 0..32 {
            assert_eq!(
                (l..r).contains(&x) && (t..b).contains(&y),
                (el..er).contains(&x) && (et..eb).contains(&y),
                "coverage at ({x},{y}); bounds {:?}",
                bounds(vertices)
            );
        }
    }
}

#[test]
fn texrect_lr_is_exclusive_in_one_and_two_cycle() {
    for cycle in [0, 1] {
        let q = quad(&decode([0, 0, 36, 36], [0, 0], [1024, 1024], cycle, false));
        assert_mask(&q, [0, 0, 9, 9]);
        assert_eq!(q[2].uv, [9.0, 9.0]);
    }
}

#[test]
fn texrect_copy_lr_is_inclusive() {
    let q = quad(&decode([3, 3, 36, 36], [0, 0], [4096, 1024], 2, false));
    assert_mask(&q, [0, 0, 10, 10]);
    assert_eq!(q[0].uv, [0.0, 0.0]);
    assert_eq!(q[2].uv, [10.0, 10.0]);
}

#[test]
fn texrect_adjacent_edges_have_no_extra_pixel() {
    for cycle in [0, 1, 2] {
        let end = if cycle == 2 { 32 } else { 36 };
        let a = quad(&decode(
            [0, 0, end, end],
            [0, 0],
            [1024, 1024],
            cycle,
            false,
        ));
        let b = quad(&decode(
            [36, 0, end + 36, end],
            [0, 0],
            [1024, 1024],
            cycle,
            false,
        ));
        assert_mask(&a, [0, 0, 9, 9]);
        assert_mask(&b, [9, 0, 18, 9]);
        assert_eq!(bounds(&a)[2], bounds(&b)[0]);
    }
}

#[test]
fn texrect_fractional_bounds_follow_console() {
    for cycle in [0, 1] {
        for x in 5..=7 {
            let [s0, s1, t0, t1] = match x {
                5 => [2407.0, 13063.0, -3995.0, -28859.0],
                6 => [2074.0, 12730.0, -3218.0, -28082.0],
                _ => [1741.0, 12397.0, -2441.0, -27305.0],
            }
            .map(|v| v / 4096.0);
            for y in 9..=11 {
                let q = quad(&decode(
                    [x, y, 38, 45],
                    [11, -13],
                    [333, -777],
                    cycle,
                    false,
                ));
                assert_mask(&q, [2, 3, 10, 12]);
                assert_eq!(q[0].uv, [s0, -1193.0 / 1024.0]);
                assert_eq!(q[2].uv, [s1, -8186.0 / 1024.0]);
                let q = quad(&decode([x, y, 38, 45], [11, -13], [333, -777], cycle, true));
                assert_mask(&q, [2, 3, 10, 12]);
                assert_eq!(q[0].uv, [685.0 / 1024.0, t0]);
                assert_eq!(q[5].uv, [685.0 / 1024.0, t1]);
                assert_eq!(q[2].uv, [3682.0 / 1024.0, t1]);
                assert_eq!(q[1].uv, [3682.0 / 1024.0, t0]);
            }
        }
    }
}

#[test]
fn texrect_flip_and_fractional_copy_step() {
    let q = quad(&decode([36, 32, 252, 60], [24, 1], [4095, 2047], 2, true));
    assert_mask(&q, [9, 8, 64, 16]);
    assert_eq!(q[0].uv, [0.75, 0.03125]);
    assert_eq!(q[5].uv, [0.75, 112713.0 / 4096.0]);
    assert_eq!(q[2].uv, [33528.0 / 1024.0, 112713.0 / 4096.0]);
    assert_eq!(q[1].uv, [33528.0 / 1024.0, 0.03125]);
}

#[test]
fn texrect_preserves_raw_fixed_and_float_bounds() {
    let op = decode([5, 11, 38, 45], [0, 0], [0, 0], 0, false);
    let SceneOp::TexRect { rect, .. } = op else {
        unreachable!()
    };
    assert_eq!([rect.ulx, rect.uly, rect.lrx, rect.lry], [5, 11, 38, 45]);
    let words: [(u32, u32); 5] = [
        gdp_set_color_image(0, 2, 32, 0x10000),
        (0xe4000026, 0x0300002d),
        (0xe1fffffb, 0),
        (0xf1fffff5, 0),
        gsp_enddl(),
    ];
    let bytes: Vec<_> = words
        .into_iter()
        .flat_map(|(a, b)| [a.to_be_bytes(), b.to_be_bytes()].concat())
        .collect();
    let result = crate::hle::interpret(
        RdramImage::new(&bytes),
        0,
        GbiUcode::F3dex2,
        crate::DataFormat::Float,
        None,
    );
    assert!(result.diags.is_empty(), "{:?}", result.diags);
    let SceneOp::TexRect { rect, .. } = &result.scene.framebuffer_pairs[0].ops[0] else {
        unreachable!()
    };
    assert_eq!([rect.ulx, rect.uly, rect.lrx, rect.lry], [-5, -11, 38, 45]);
}

#[test]
fn texrect_uniform_uses_texel_units() {
    let mut rdp = crate::hle::rdp::Rdp {
        other_mode_h: 2 << 20,
        ..Default::default()
    };
    rdp.tiles[0] = crate::hle::rdp::TileDescriptor {
        fmt: 0,
        siz: 2,
        width: 16,
        height: 8,
        line: 4,
        ..Default::default()
    };
    let mut mat = crate::hle::combiner::build_rect_material(
        &rdp,
        &crate::hle::rsp::Rsp::default(),
        0,
        &mut Vec::new(),
        0,
    )
    .unwrap();
    mat.tex_enable = true;
    let rm = crate::hle::blender::decode_render_mode(0, 0, 0);
    let uniform = CombinerUniform::from_rect(&mat, &rm, [0; 4]);
    assert_eq!(uniform.inv_tex_size, [0.0625, 0.125, 1.0, 0.0]);
}

#[test]
fn texrect_coverage_pixels() {
    // The readback helper needs 256-byte rows, so the target is 64 wide.
    use crate::hle::{ColorImage, FramebufferPair, Scissor};
    use crate::tests::common::{pixel, render_to_pixels, scene_from_fixture};
    let (device, queue) = headless_device_forced_fallback();
    let mut renderer = SceneRenderer::new(&device, wgpu::TextureFormat::Rgba8Unorm, 64, 64, false);
    for (cycle, raw, expected) in [
        (0, [0, 0, 36, 36], [0, 0, 9, 9]),
        (1, [0, 0, 36, 36], [0, 0, 9, 9]),
        (2, [3, 3, 36, 36], [0, 0, 10, 10]),
        (0, [5, 11, 38, 45], [2, 3, 10, 12]),
    ] {
        let mut scene = scene_from_fixture("framebuffer-extent--white1");
        scene.materials[0].tex_enable = true;
        scene.materials[0].texture = vec![255; 4].into();
        scene.materials[0].tex_w = 1;
        scene.materials[0].tex_h = 1;
        scene.framebuffer_pairs = vec![FramebufferPair {
            color_image: ColorImage {
                fmt: 0,
                siz: 2,
                width: 64,
                addr: 0x10000,
            },
            ops: vec![decode(raw, [0, 0], [0, 0], cycle, false)],
            active_scissor: Scissor {
                lrx: 64,
                lry: 64,
                ..Default::default()
            },
            size_extent: (64, 64),
            ..Default::default()
        }];
        let pixels = render_to_pixels(&device, &queue, &mut renderer, &scene, 64, 64);
        let [l, t, r, b] = expected;
        for y in 0..64 {
            for x in 0..64 {
                let expected = if (l..r).contains(&x) && (t..b).contains(&y) {
                    [255; 4]
                } else {
                    [13, 13, 20, 255]
                };
                assert_eq!(
                    pixel(&pixels, 64, x, y),
                    expected,
                    "cycle {cycle}, ({x},{y})"
                );
            }
        }
    }
}

#[test]
fn coverage_rect_sampling_origin() {
    use crate::hle::{ColorImage, FramebufferPair, Scissor};
    use crate::tests::common::{pixel, render_to_pixels, scene_from_fixture};

    let (device, queue) = headless_device_forced_fallback();
    let mut renderer = SceneRenderer::new(&device, wgpu::TextureFormat::Rgba8Unorm, 64, 64, false);
    for cycle in [0, 1, 2] {
        for flip in [false, true] {
            let mut scene = scene_from_fixture("framebuffer-extent--white1");
            let mat = &mut scene.materials[0];
            mat.tex_enable = true;
            mat.tex_w = 16;
            mat.tex_h = 16;
            mat.cycle_type = cycle;
            mat.filter_mode = 0;
            mat.texture = (0..16)
                .flat_map(|y| (0..16).flat_map(move |x| [x * 8, y * 8, 64, 255]))
                .collect::<Vec<_>>()
                .into();
            mat.sampling = crate::hle::tile_sampling::TileSampling::from_tile(
                &crate::hle::rdp::TileDescriptor {
                    width: 16,
                    height: 16,
                    cms: 2,
                    cmt: 2,
                    ..Default::default()
                },
                0,
            );
            let color = |d| CcPass {
                a: 31,
                b: 31,
                c: 31,
                d,
            };
            let alpha = |d| CcPass {
                a: 7,
                b: 7,
                c: 7,
                d,
            };
            let (combine_l, combine_h) = gdp_set_combine_lerp(
                color(1),
                alpha(1),
                color(u32::from(cycle == 0)),
                alpha(u32::from(cycle == 0)),
            );
            mat.selectors = crate::hle::combiner::decode_combine(combine_l, combine_h);
            let end = if cycle == 2 { 60 } else { 64 };
            let step = if cycle == 2 { 4096 } else { 1024 };
            scene.framebuffer_pairs = vec![FramebufferPair {
                color_image: ColorImage {
                    fmt: 0,
                    siz: 3,
                    width: 64,
                    addr: 0x10000,
                },
                ops: vec![decode(
                    [32, 32, end, end],
                    [24, 24],
                    [step, 1024],
                    cycle,
                    flip,
                )],
                active_scissor: Scissor {
                    lrx: 64,
                    lry: 64,
                    ..Default::default()
                },
                size_extent: (64, 64),
                ..Default::default()
            }];
            let pixels = render_to_pixels(&device, &queue, &mut renderer, &scene, 64, 64);
            for y in 0..64 {
                for x in 0..64 {
                    let expected = if (8..16).contains(&x) && (8..16).contains(&y) {
                        let (s, t) = if flip && cycle == 2 {
                            ((4 * (y - 8) + (x - 8) % 4).min(15), (x - 8) / 4)
                        } else if flip {
                            (y - 8, x - 8)
                        } else {
                            (x - 8, y - 8)
                        };
                        [(s * 8) as u8, (t * 8) as u8, 64, 255]
                    } else {
                        [13, 13, 20, 255]
                    };
                    assert_eq!(
                        pixel(&pixels, 64, x, y),
                        expected,
                        "cycle {cycle}, flip {flip}, ({x},{y})"
                    );
                }
            }
        }
    }
}

#[test]
fn texrect_filter_rounding_console_samples() {
    use crate::tests::{
        common::{pixel, render_to_pixels},
        dl_builder::DlBuilder,
    };

    let texture: Vec<_> = (0..32u16)
        .flat_map(|y| {
            (0..32u16).flat_map(move |x| {
                (((x % 2) * 0xf800) | ((y % 2) * 0x07c0) | (((3 * x + 5 * y) % 32) << 1) | 1)
                    .to_be_bytes()
            })
        })
        .collect();
    let (device, queue) = headless_device_forced_fallback();
    // Expected words are literal NTSC console RGBA5551 readbacks.
    for (name, rect, origin, base, step, combine, primitive, samples) in [
        (
            "fx-20-bilerp-0555-plain-3",
            0xe4060060,
            0x00023020,
            0x00180013,
            0x05550555,
            (0xfcffffff, 0xfffcf279),
            0xffffffff,
            [(22, 8, 0x6cb7), (17, 10, 0x4de9), (20, 13, 0x4de9)],
        ),
        (
            "tie-159-half-filtered, primitive 170",
            0xe4078078,
            0x00070070,
            0x01770068,
            0,
            (0xfc11fe23, 0xfffff3f9),
            0xaaaaaaff,
            [(28, 28, 0x33db), (29, 28, 0x33db), (28, 29, 0x33db)],
        ),
    ] {
        let mut builder = DlBuilder::new();
        let address = builder.bytes(8, &texture);
        builder.list(
            "main",
            &[
                gdp_set_color_image(0, 2, 64, 0x100000),
                gdp_set_scissor(0, 0, 0, 256, 256),
                gdp_set_texture_image(0, 2, 32, address),
                gdp_set_tile(0, 2, 8, 0, 7, 0, 0, 0, 0, 0, 0, 0),
                (0xf4000000, 0x0707c07c),
                gdp_set_tile(0, 2, 8, 0, 0, 0, 0, 5, 0, 0, 5, 0),
                gdp_set_tile_size(0, 0, 0, 124, 124),
                (0xef002cf0, 0x0f0a4000),
                (0xfa000000, primitive),
                combine,
                (rect, origin),
                (0xe1000000, base),
                (0xf1000000, step),
                gsp_enddl(),
            ],
        );
        let built = builder.finish("main");
        let result = crate::hle::interpret(
            RdramImage::new(&built.rdram),
            u64::from(built.entry),
            GbiUcode::F3dex2,
            crate::DataFormat::Fixed,
            None,
        );
        assert!(result.diags.is_empty(), "{name}: {:?}", result.diags);
        let mut renderer =
            SceneRenderer::new(&device, wgpu::TextureFormat::Rgba8Unorm, 64, 64, false);
        let pixels = render_to_pixels(&device, &queue, &mut renderer, &result.scene, 64, 64);
        for (x, y, expected) in samples {
            let [r, g, b, a] = pixel(&pixels, 64, x, y);
            let packed = (u16::from(r >> 3) << 11)
                | (u16::from(g >> 3) << 6)
                | (u16::from(b >> 3) << 1)
                | u16::from(a >= 128);
            assert_eq!(packed, expected, "{name} ({x},{y})");
        }
    }
}
