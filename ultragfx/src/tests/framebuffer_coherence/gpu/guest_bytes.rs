use super::*;

#[test]
fn guest_bytes_last_writer_and_framebuffer_texture_agree() {
    for fallback in [false, true] {
        for known in [false, true] {
            let coverage = if known {
                CVG_DST_FULL
            } else {
                CVG_DST_SAVE | IM_RD
            };
            let mut r = renderer(fallback);
            r.set_guest_writeback_with_encoding(true, crate::FramebufferEncoding::BigEndian);
            let mut memory = Memory(vec![0; OUTPUT as usize + 320 * 240 * 2]);
            let mut dl = Vec::new();
            image(&mut dl, FB, 4, 4, 0, 2);
            dl.extend([
                (0xef000030 | G_TC_FILT, coverage),
                combine(3, 3),
                gdp_set_prim_color(0, 0, 0x13558dff),
                gdp_fill_rectangle(0, 0, 16, 16),
                (0xef0000f0 | G_TC_FILT, coverage),
                gdp_fill_rectangle(4, 0, 8, 16),
                (0xef000070 | G_TC_FILT, coverage),
            ]);
            image(&mut dl, OUTPUT, 8, 4, 4, 1);
            dl.extend([
                combine(1, 1),
                gdp_set_texture_image(4, 1, 8, FB),
                gdp_set_tile(4, 1, 1, 0, 7, 0, 2, 0, 0, 2, 0, 0),
                gdp_load_sync(),
                (0xf4000000, (7 << 24) | (28 << 12) | 12),
                gdp_pipe_sync(),
                gdp_set_tile(4, 1, 1, 0, 0, 0, 2, 0, 0, 2, 0, 0),
                gdp_set_tile_size(0, 0, 0, 28, 12),
            ]);
            dl.extend(gsp_texture_rectangle(
                0, 0, 32, 16, 0, 0, 0, 1024, 1024, false,
            ));
            run_compatibility(&mut r, &mut memory, built(DlBuilder::new(), dl));
            let expected = *include_bytes!("../../guest_bytes/last-writer.rgba16");
            for pixel in 0..16 {
                assert_eq!(
                    r.inner
                        .coherence
                        .as_ref()
                        .unwrap()
                        .stored_coverage_code(FB as u64 + pixel * 2),
                    known.then_some(7)
                );
            }
            assert_eq!(&memory.0[FB as usize..FB as usize + 32], &expected);
            assert_eq!(&memory.0[OUTPUT as usize..OUTPUT as usize + 32], &expected);
        }
    }
}

#[test]
fn guest_bytes_fill_and_copy_bypass_dither_and_copy_coverage_stays_unknown() {
    let mut r = renderer(true);
    r.set_guest_writeback_with_encoding(true, crate::FramebufferEncoding::BigEndian);
    let mut memory = Memory(vec![0; OUTPUT as usize + 320 * 240 * 2]);
    for selector in 0..4 {
        r.reset();
        let mut b = DlBuilder::new();
        let texture = b.bytes(8, &[0x21, 0x09, 0x21, 0x08].repeat(2));
        let mut dl = Vec::new();
        image(&mut dl, FB, 4, 1, 0, 2);
        dl.extend([
            (0xef300000 | (selector << 6), 0),
            gdp_set_fill_color(0x12a31ae5),
            gdp_fill_rectangle(0, 0, 12, 0),
        ]);
        image(&mut dl, OUTPUT, 4, 1, 0, 2);
        dl.push((0xef200000 | (selector << 6), 0));
        dl.extend(gdp_load_texture_block(0, 2, 4, 1, texture, 2, 0, 2, 0));
        dl.extend(gsp_texture_rectangle(
            0, 0, 12, 0, 0, 0, 0, 4096, 1024, false,
        ));
        run_compatibility(&mut r, &mut memory, built(b, dl));
        assert_eq!(
            &memory.0[FB as usize..FB as usize + 8],
            &[0x12, 0xa3, 0x1a, 0xe5, 0x12, 0xa3, 0x1a, 0xe5]
        );
        assert_eq!(
            &memory.0[OUTPUT as usize..OUTPUT as usize + 8],
            &[0x21, 0x09, 0x21, 0x08, 0x21, 0x09, 0x21, 0x08]
        );
        for pixel in 0..4 {
            assert_eq!(
                r.inner
                    .coherence
                    .as_ref()
                    .unwrap()
                    .stored_coverage_code(OUTPUT as u64 + pixel * 2),
                None
            );
        }
    }
}

#[test]
fn guest_bytes_native_snapshot_survives_source_overwrite() {
    let mut r = renderer(true);
    r.set_guest_writeback_with_encoding(true, crate::FramebufferEncoding::BigEndian);
    let mut memory = Memory(vec![0; OUTPUT as usize + 320 * 240 * 2]);
    let mut dl = Vec::new();
    image(&mut dl, FB, 4, 1, 0, 2);
    fill(&mut dl, 0x12a312a3, [0, 0, 3, 0]);
    run(&mut r, &mut memory, built(DlBuilder::new(), dl));
    let mut dl = Vec::new();
    image(&mut dl, OUTPUT, 8, 1, 4, 1);
    dl.extend([
        (0xef000030 | G_TC_FILT, CVG_DST_FULL),
        combine(1, 1),
        gdp_set_texture_image(4, 1, 8, FB),
        gdp_set_tile(4, 1, 1, 0, 7, 0, 2, 0, 0, 2, 0, 0),
        gdp_load_sync(),
        (0xf4000000, (7 << 24) | (28 << 12)),
        gdp_pipe_sync(),
        gdp_set_tile(4, 1, 1, 0, 0, 0, 2, 0, 0, 2, 0, 0),
        gdp_set_tile_size(0, 0, 0, 28, 0),
    ]);
    image(&mut dl, FB, 4, 1, 0, 2);
    fill(&mut dl, 0xf801f801, [0, 0, 3, 0]);
    image(&mut dl, OUTPUT, 8, 1, 4, 1);
    dl.extend([(0xef000030 | G_TC_FILT, CVG_DST_FULL), combine(1, 1)]);
    dl.extend(gsp_texture_rectangle(
        0, 0, 32, 4, 0, 0, 0, 1024, 1024, false,
    ));
    run_compatibility(&mut r, &mut memory, built(DlBuilder::new(), dl));
    assert_eq!(
        &memory.0[OUTPUT as usize..OUTPUT as usize + 8],
        &[0x12, 0xa3, 0x12, 0xa3, 0x12, 0xa3, 0x12, 0xa3]
    );
}

#[test]
fn guest_bytes_copy_presents_modern_color_at_both_scales() {
    for scale in [1, 2] {
        for writeback in [false, true] {
            let mut r = renderer(true);
            r.reconfigure(crate::RendererConfig {
                resolution_multiplier: scale,
                ..r.config
            });
            r.set_guest_writeback_with_encoding(writeback, crate::FramebufferEncoding::BigEndian);
            let mut memory = Memory(vec![0; OUTPUT as usize + 320 * 240 * 2]);
            let mut dl = Vec::new();
            image(&mut dl, FB, 4, 1, 0, 2);
            dl.extend([
                (0xef000030 | G_TC_FILT, CVG_DST_SAVE | IM_RD),
                combine(3, 3),
                gdp_set_prim_color(0, 0, 0x13558dff),
                gdp_fill_rectangle(0, 0, 16, 4),
            ]);
            run_compatibility(&mut r, &mut memory, built(DlBuilder::new(), dl));
            let mut dl = Vec::new();
            image(&mut dl, OUTPUT, 4, 1, 0, 2);
            dl.push((0xef2000c0, 0));
            dl.extend(gdp_load_texture_block(0, 2, 4, 1, FB, 2, 0, 2, 0));
            dl.extend(gsp_texture_rectangle(
                0, 0, 12, 0, 0, 0, 0, 4096, 1024, false,
            ));
            run_compatibility(&mut r, &mut memory, built(DlBuilder::new(), dl));
            let expected = [19, 85, 141, 255].repeat((4 * scale * scale) as usize);
            let source = &r.frame_scenes.last().unwrap().materials[0].texture;
            let crate::hle::texture_request::TextureSource::Framebuffer(request) = source else {
                panic!("expected framebuffer texture");
            };
            assert!(request.image_route());
            assert_eq!(
                pixels(&mut r, OUTPUT, 4 * scale, scale),
                expected,
                "scale={scale}, writeback={writeback}"
            );
        }
    }
}

#[test]
fn guest_bytes_copy_preserves_framebuffer_words() {
    for scale in [1, 2] {
        for native in [false, true] {
            for materialized in [false, true] {
                let mut r = renderer(true);
                r.reconfigure(crate::RendererConfig {
                    resolution_multiplier: scale,
                    ..r.config
                });
                r.set_guest_writeback_with_encoding(true, crate::FramebufferEncoding::BigEndian);
                let mut memory = Memory(vec![0; OUTPUT as usize + 320 * 240 * 2]);
                let mut dl = Vec::new();
                image(&mut dl, FB, 4, 1, 0, 2);
                if native {
                    fill(&mut dl, 0x12a31ae4, [0, 0, 3, 0]);
                    run(&mut r, &mut memory, built(DlBuilder::new(), dl));
                } else {
                    dl.extend([
                        (0xef000030 | G_TC_FILT, CVG_DST_SAVE | IM_RD),
                        combine(3, 3),
                        gdp_set_prim_color(0, 0, 0x13558dff),
                        gdp_fill_rectangle(0, 0, 16, 4),
                    ]);
                    run_compatibility(&mut r, &mut memory, built(DlBuilder::new(), dl));
                }
                let expected: &[u8] = if native {
                    &[0x12, 0xa3, 0x1a, 0xe4, 0x12, 0xa3, 0x1a, 0xe4]
                } else {
                    &[0x1a, 0xe5, 0x12, 0xa3, 0x1a, 0xe5, 0x12, 0xa3]
                };
                assert_eq!(&memory.0[FB as usize..FB as usize + 8], expected);
                let mut dl = Vec::new();
                image(&mut dl, OUTPUT, 4, 1, 0, 2);
                dl.push((0xef2000c0, 0));
                if materialized {
                    dl.extend(gdp_load_texture_block(4, 1, 8, 1, FB, 2, 0, 2, 0));
                    dl.push(gdp_set_tile(0, 2, 1, 0, 0, 0, 2, 0, 0, 2, 0, 0));
                    dl.push(gdp_set_tile_size(0, 0, 0, 12, 0));
                } else {
                    dl.extend(gdp_load_texture_block(0, 2, 4, 1, FB, 2, 0, 2, 0));
                }
                dl.extend(gsp_texture_rectangle(
                    0, 0, 12, 0, 0, 0, 0, 4096, 1024, false,
                ));
                run_compatibility(&mut r, &mut memory, built(DlBuilder::new(), dl));
                assert_eq!(
                    &memory.0[OUTPUT as usize..OUTPUT as usize + 8],
                    expected,
                    "native={native}, materialized={materialized}"
                );
                for pixel in 0..4 {
                    assert_eq!(
                        r.inner
                            .coherence
                            .as_ref()
                            .unwrap()
                            .stored_coverage_code(OUTPUT as u64 + pixel * 2),
                        None
                    );
                }
            }
        }
    }
}

#[test]
fn guest_bytes_copy_only_target_resolves_pending_source_words() {
    for scale in [1, 2] {
        let mut r = renderer(true);
        r.reconfigure(crate::RendererConfig {
            resolution_multiplier: scale,
            ..r.config
        });
        r.set_guest_writeback_with_encoding(true, crate::FramebufferEncoding::BigEndian);
        let mut memory = Memory(vec![0; OUTPUT as usize + 320 * 240 * 2]);
        let mut dl = Vec::new();
        image(&mut dl, FB, 4, 1, 0, 2);
        dl.extend([
            (0xef000030 | G_TC_FILT, CVG_DST_SAVE | IM_RD),
            combine(3, 3),
            gdp_set_prim_color(0, 0, 0x13558dff),
            gdp_fill_rectangle(0, 0, 16, 4),
        ]);
        image(&mut dl, FB + 0x1000, 4, 1, 0, 2);
        dl.extend(gdp_load_texture_block(0, 2, 4, 1, FB, 2, 0, 2, 0));
        image(&mut dl, OUTPUT, 4, 1, 0, 2);
        dl.push((0xef2000c0, 0));
        dl.extend(gsp_texture_rectangle(
            0, 0, 12, 0, 0, 0, 0, 4096, 1024, false,
        ));
        let task = built(DlBuilder::new(), dl);
        let mut scene = crate::hle::interpret_rdram(&task.rdram, task.entry).scene;
        let pair = scene
            .framebuffer_pairs
            .iter()
            .position(|pair| {
                pair.ops
                    .iter()
                    .any(|op| matches!(op, crate::scene::SceneOp::FramebufferCopy(_)))
            })
            .unwrap();
        let index = scene.framebuffer_pairs[pair]
            .ops
            .iter()
            .position(|op| matches!(op, crate::scene::SceneOp::FramebufferCopy(_)))
            .unwrap();
        let copy = scene.framebuffer_pairs[pair].ops.remove(index);
        let mut load = scene.framebuffer_pairs[pair].clone();
        load.color_image.addr = u64::from(FB + 0x1000);
        load.ops = vec![copy];
        scene.framebuffer_pairs.insert(pair + 1, load);
        r.inner.coherence.as_mut().unwrap().begin_task();
        r.inner
            .render_into_store(&r.device, &r.queue, &scene, r.config.clear_policy);
        for patch in r.inner.coherence.as_mut().unwrap().finish(&r.device) {
            memory.0[patch.address as usize..patch.address as usize + patch.bytes.len()]
                .copy_from_slice(&patch.bytes);
        }
        let expected = [0x1a, 0xe5, 0x12, 0xa3, 0x1a, 0xe5, 0x12, 0xa3];
        assert_eq!(&memory.0[FB as usize..FB as usize + 8], &expected);
        assert_eq!(&memory.0[OUTPUT as usize..OUTPUT as usize + 8], &expected);
    }
}

#[test]
fn guest_bytes_copy_to_eight_bit_targets_uses_guest_samples() {
    for fmt in 2..=4 {
        let mut r = renderer(true);
        r.set_guest_writeback_with_encoding(true, crate::FramebufferEncoding::BigEndian);
        let mut memory = Memory(vec![0; OUTPUT as usize + 320 * 240 * 2]);
        let mut dl = Vec::new();
        image(&mut dl, FB, 4, 1, 0, 2);
        dl.extend([
            (0xef000030 | G_TC_FILT, CVG_DST_SAVE | IM_RD),
            combine(3, 3),
            gdp_set_prim_color(0, 0, 0x13558dff),
            gdp_fill_rectangle(0, 0, 16, 4),
        ]);
        run_compatibility(&mut r, &mut memory, built(DlBuilder::new(), dl));
        let mut dl = Vec::new();
        image(&mut dl, OUTPUT, 4, 1, fmt, 1);
        dl.push((0xef2000c0, 0));
        dl.extend(gdp_load_texture_block(0, 2, 4, 1, FB, 2, 0, 2, 0));
        dl.extend(gsp_texture_rectangle(
            0, 0, 12, 0, 0, 0, 0, 4096, 1024, false,
        ));
        run_compatibility(&mut r, &mut memory, built(DlBuilder::new(), dl));
        assert_eq!(
            &memory.0[OUTPUT as usize..OUTPUT as usize + 4],
            &[24, 82, 24, 82],
            "fmt={fmt}"
        );
        assert_eq!(
            pixels(&mut r, OUTPUT, 4, 1),
            [19, 85, 141, 255].repeat(4),
            "fmt={fmt}"
        );
    }
}

fn texel_quad(corners: [[i32; 3]; 4]) -> [VtxColored; 4] {
    let st = [[0, 0], [0, 512], [1024, 512], [1024, 0]];
    std::array::from_fn(|i| VtxColored {
        x: corners[i][0] as i16,
        y: corners[i][1] as i16,
        z: corners[i][2] as i16,
        flag: 0,
        s: st[i][0],
        t: st[i][1],
        r: 255,
        g: 255,
        b: 255,
        a: 255,
    })
}

fn ortho_quad(x: i32, y: i32) -> [VtxColored; 4] {
    texel_quad(
        [[x, y], [x, y + 32], [x + 64, y + 32], [x + 64, y]].map(|[x, y]| [x - 160, 120 - y, 0]),
    )
}

fn depth_quad(x: i32, y: i32, z: [i32; 4]) -> [VtxColored; 4] {
    let screen = [[x, y], [x, y + 32], [x + 64, y + 32], [x + 64, y]];
    texel_quad(std::array::from_fn(|i| {
        let [x, y] = screen[i];
        [(x - 160) * z[i] / 128, (120 - y) * z[i] / 128, z[i]]
    }))
}

fn receding_quad(x: i32, y: i32, left: i32, right: i32) -> [VtxColored; 4] {
    let half = 16 * left / 128;
    let (xl, xr) = ((x - 160) * left / 128, (x + 64 - 160) * right / 128);
    let (yl, yr) = ((104 - y) * left / 128, (104 - y) * right / 128);
    texel_quad([
        [xl, yl + half, left],
        [xl, yl - half, left],
        [xr, yr - half, right],
        [xr, yr + half, right],
    ])
}

fn tp_none_task() -> Built {
    let mut b = DlBuilder::new();
    let texels = (0..32u16)
        .flat_map(|t| {
            (0..64u16).flat_map(move |s| {
                let blue = if s >= 32 { 31 } else { 0 };
                ((s & 31) << 11 | t << 6 | blue << 1 | 1).to_be_bytes()
            })
        })
        .collect::<Vec<_>>();
    let texture = b.bytes(16, &texels);
    let viewport = b.viewport(Vp {
        vscale: [512, 512, 511, 0],
        vtrans: [640, 480, 511, 0],
    });
    let ortho = b.matrix([
        [1.0 / 128.0, 0.0, 0.0, 0.0],
        [0.0, 1.0 / 128.0, 0.0, 0.0],
        [0.0; 4],
        [0.0, 0.0, 0.0, 1.0],
    ]);
    let perspective = b.matrix([
        [0.25, 0.0, 0.0, 0.0],
        [0.0, 0.25, 0.0, 0.0],
        [0.0, 0.0, 0.0, 0.25],
        [0.0; 4],
    ]);
    let identity = b.matrix([
        [1.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ]);
    let persp_flat = b.vertices(&ortho_quad(40, 16));
    let none_flat = b.vertices(&ortho_quad(128, 16));
    let persp_receding = b.vertices(&receding_quad(216, 16, 256, 768));
    let none_receding = b.vertices(&receding_quad(40, 64, 256, 768));
    let none_origin = b.vertices(&ortho_quad(216, 64));
    let none_equal_w = b.vertices(&receding_quad(40, 112, 400, 400));
    let none_split = b.vertices(&depth_quad(128, 112, [256, 768, 768, 768]));
    let quad = |address| [gsp_vertex(0, 4, address), gsp_2triangles(0, 1, 2, 0, 2, 3)];
    // The words of the display list the console ran; see guest_bytes/README.md.
    let mut dl = vec![
        (0xff10013f, FB),
        (0xed000000, 0x005003c0),
        (0xef300000, 0),
        (0xf7000000, 0x001f001f),
        (0xf64fc3bc, 0),
        (0xe7000000, 0),
        (0xef080cf0, 0x0f0a4000),
        (0xfcffffff, 0xfffcf279),
        (0xfd100000, texture),
        (0xf5100000, 0x07014060),
        (0xe6000000, 0),
        (0xf3000000, 0x077ff080),
        (0xe7000000, 0),
        (0xf5102000, 0x00014060),
        (0xf2000000, 0x000fc07c),
        (0xd9000000, 0),
        (0xd7000002, 0xffffffff),
        gsp_persp_normalize(0xffff),
        gsp_viewport(viewport),
        gsp_matrix(ortho, true, true, false),
        gsp_matrix(identity, false, true, false),
    ];
    dl.extend(quad(persp_flat));
    dl.extend([(0xe7000000, 0), (0xe3000c00, 0)]);
    dl.extend(quad(none_flat));
    dl.extend([
        (0xe4300180, 0x00200100),
        (0xe1000000, 0),
        (0xf1000000, 0x02000200),
        (0xe7000000, 0),
        (0xf2020010, 0x0011c08c),
    ]);
    dl.extend(quad(none_origin));
    dl.extend([
        (0xe7000000, 0),
        (0xf2000000, 0x000fc07c),
        gsp_matrix(perspective, true, true, false),
    ]);
    dl.extend(quad(none_receding));
    dl.extend(quad(none_equal_w));
    dl.extend([
        gsp_vertex(0, 4, none_split),
        gsp_2triangles(0, 1, 3, 1, 2, 3),
        (0xe7000000, 0),
        (0xe3000c00, 0x00080000),
    ]);
    dl.extend(quad(persp_receding));
    built(b, dl)
}

#[test]
fn guest_bytes_tp_none_triangles_sample_halved_screen_linear_coordinates() {
    let expected = include_bytes!("../../guest_bytes/tex-persp.rgba16");
    for scale in [1, 2, 4] {
        let mut r = renderer(true);
        r.reconfigure(crate::RendererConfig {
            resolution_multiplier: scale,
            ..r.config
        });
        r.set_guest_writeback_with_encoding(true, crate::FramebufferEncoding::BigEndian);
        let mut memory = Memory(vec![0; OUTPUT as usize + 320 * 240 * 2]);
        run(&mut r, &mut memory, tp_none_task());
        // T and cases A, C, D and G differ from the console by sub-texel phases that G_TP_NONE
        // handling doesn't cause (A shows the T one under G_TP_PERSP). E is the rectangle control.
        for (case, [x0, y0], mask, diagonal) in [
            (1, [128, 16], 0xf83e, [0, 0, 64, 32]),
            (4, [128, 64], 0xffff, [0, 0, 64, 32]),
            (5, [216, 64], 0xf83e, [0, 0, 64, 32]),
            (7, [128, 112], 0xffff, [0, 32, 64, 0]),
        ] {
            let mut differing = 0;
            for y in 1..31 {
                for x in 1..63 {
                    let [ax, ay, bx, by] = diagonal.map(f64::from);
                    let (cx, cy) = (f64::from(x) + 0.5, f64::from(y) + 0.5);
                    let cross = (bx - ax) * (cy - ay) - (by - ay) * (cx - ax);
                    if cross.abs() < (bx - ax).hypot(by - ay) {
                        continue;
                    }
                    let at = FB as usize + (((y0 + y) * 320 + x0 + x) * 2) as usize;
                    let word = u16::from_be_bytes([memory.0[at], memory.0[at + 1]]);
                    let i = (case * 2048 + y * 64 + x) as usize * 2;
                    let console = u16::from_be_bytes([expected[i], expected[i + 1]]);
                    differing += usize::from(word & mask != console & mask);
                }
            }
            assert_eq!(differing, 0, "case {case}, scale {scale}");
        }
    }
}
