use super::*;

#[test]
fn ordinary_writeback_runs_share_color_and_replay_passes() {
    for fallback in [false, true] {
        for operations in [1, 16] {
            let mut r = renderer(fallback);
            let recorder = crate::profiling::Recorder::new(crate::profiling::Mode::Counters);
            r.inner.profiling = recorder.clone();
            let mut memory = Memory(vec![0; OUTPUT as usize + 64]);
            let mut dl = Vec::new();
            image(&mut dl, FB, 8, 2, 0, 2);
            for _ in 0..operations {
                fill(&mut dl, 0xf801f801, [0, 0, 7, 1]);
            }
            run_compatibility(&mut r, &mut memory, built(DlBuilder::new(), dl));
            assert_words(
                &memory,
                FB,
                &[0xf801; 16],
                crate::FramebufferEncoding::Native,
            );
            let counters = recorder.snapshot().counters;
            assert_eq!(counters["workload.segments"], 1);
            assert_eq!(counters["writeback.replay_passes"], 1);
        }
    }
}

fn load_copy(dl: &mut Vec<Command>, source: u32, width: u32, height: u32) {
    dl.push((0xef2000c0, 0));
    dl.extend(gdp_load_texture_block(
        0, 2, width, height, source, 2, 0, 2, 0,
    ));
}

fn copy_rect(dl: &mut Vec<Command>, x: u32, width: u32, height: u32, s: u32) {
    dl.extend(gsp_texture_rectangle(
        x * 4,
        0,
        (x + width - 1) * 4,
        (height - 1) * 4,
        0,
        s,
        0,
        4096,
        1024,
        false,
    ));
}

fn assert_words(
    memory: &Memory,
    address: u32,
    words: &[u16],
    encoding: crate::FramebufferEncoding,
) {
    let bytes: Vec<_> = words
        .iter()
        .flat_map(|word| match encoding {
            crate::FramebufferEncoding::BigEndian => word.to_be_bytes(),
            crate::FramebufferEncoding::Native => word.to_ne_bytes(),
        })
        .collect();
    assert_eq!(
        &memory.0[address as usize..address as usize + bytes.len()],
        bytes
    );
}

#[test]
fn multi_copy_preserves_words_at_odd_row_tails_after_reload_and_rejection() {
    for fallback in [false, true] {
        for scale in [1, 2] {
            for encoding in [
                crate::FramebufferEncoding::Native,
                crate::FramebufferEncoding::BigEndian,
            ] {
                let mut r = renderer(fallback);
                r.reconfigure(crate::RendererConfig {
                    resolution_multiplier: scale,
                    ..r.config
                });
                r.set_guest_writeback_with_encoding(true, encoding);
                let mut memory = Memory(vec![0; OUTPUT as usize + 64]);
                let mut dl = Vec::new();
                for (address, word) in [
                    (FB, 0x12a31ae4),
                    (FB + 0x1000, 0x5ad77bdf),
                    (FB + 0x2000, 0xf800f800),
                ] {
                    image(&mut dl, address, 4, 2, 0, 2);
                    fill(&mut dl, word, [0, 0, 3, 1]);
                }
                image(&mut dl, OUTPUT, 3, 2, 0, 2);
                fill(&mut dl, 0x003f003f, [0, 0, 2, 1]);
                load_copy(&mut dl, FB, 4, 2);
                copy_rect(&mut dl, 0, 2, 2, 0);
                copy_rect(&mut dl, 2, 1, 2, 64);
                load_copy(&mut dl, FB + 0x1000, 4, 2);
                copy_rect(&mut dl, 1, 1, 2, 0);
                load_copy(&mut dl, FB + 0x2000, 4, 2);
                dl.extend([(0xef2000c0, 1), gdp_set_blend_color(0xffffffff)]);
                copy_rect(&mut dl, 0, 3, 2, 0);
                run_compatibility(&mut r, &mut memory, built(DlBuilder::new(), dl));
                assert_words(
                    &memory,
                    OUTPUT,
                    &[0x12a3, 0x5ad7, 0x12a3, 0x12a3, 0x5ad7, 0x12a3],
                    encoding,
                );
                for pixel in 0..6 {
                    assert_eq!(
                        r.inner
                            .coherence
                            .as_ref()
                            .unwrap()
                            .stored_coverage_code(u64::from(OUTPUT) + pixel * 2),
                        None
                    );
                }
            }
        }
    }
}

#[test]
fn invalid_copy_source_preserves_the_previous_copy_plane() {
    for fallback in [false, true] {
        let mut r = renderer(fallback);
        r.set_guest_writeback_with_encoding(true, crate::FramebufferEncoding::BigEndian);
        let mut memory = Memory(vec![0; OUTPUT as usize + 64]);
        let mut dl = Vec::new();
        for (address, word) in [(FB, 0x12a31ae4), (FB + 0x1000, 0x5ad77bdf)] {
            image(&mut dl, address, 4, 2, 0, 2);
            fill(&mut dl, word, [0, 0, 3, 1]);
        }
        image(&mut dl, OUTPUT, 3, 2, 0, 2);
        load_copy(&mut dl, FB, 4, 2);
        copy_rect(&mut dl, 0, 3, 2, 0);
        load_copy(&mut dl, FB + 0x1000, 4, 2);
        copy_rect(&mut dl, 0, 3, 2, 0);
        let task = built(DlBuilder::new(), dl);
        let mut scene = crate::hle::interpret_rdram(&task.rdram, task.entry).scene;
        for pair in &mut scene.framebuffer_pairs {
            if pair.color_image.addr == u64::from(FB + 0x1000) {
                pair.color_image.addr = u64::from(FB + 0x3000);
            }
        }
        r.inner.coherence.as_mut().unwrap().begin_task();
        r.inner
            .render_into_store(&r.device, &r.queue, &scene, r.config.clear_policy);
        for patch in r.inner.coherence.as_mut().unwrap().finish(&r.device) {
            memory.0[patch.address as usize..patch.address as usize + patch.bytes.len()]
                .copy_from_slice(&patch.bytes);
        }
        assert!(r.inner.diagnostics.iter().any(|d| matches!(
            d.kind,
            crate::DiagKind::UnsupportedFramebufferAccess {
                reason: crate::FramebufferAccess::MissingSource,
                ..
            }
        )));
        assert_words(
            &memory,
            OUTPUT,
            &[0x12a3, 0x1ae4, 0x12a3, 0x12a3, 0x1ae4, 0x12a3],
            crate::FramebufferEncoding::BigEndian,
        );
    }
}

#[test]
fn aliased_source_and_repeated_target_visits_keep_prior_copy_words() {
    for fallback in [false, true] {
        for scale in [1, 2] {
            for clear_policy in [crate::ClearPolicy::Persist, crate::ClearPolicy::PerFrame] {
                let mut r = renderer(fallback);
                r.reconfigure(crate::RendererConfig {
                    resolution_multiplier: scale,
                    clear_policy,
                    ..r.config
                });
                r.set_guest_writeback_with_encoding(true, crate::FramebufferEncoding::BigEndian);
                let mut memory = Memory(vec![0; OUTPUT as usize + 64]);
                let mut dl = Vec::new();
                image(&mut dl, FB, 4, 2, 0, 2);
                fill(&mut dl, 0x12a31ae4, [0, 0, 3, 1]);
                image(&mut dl, OUTPUT, 3, 2, 0, 2);
                load_copy(&mut dl, FB, 4, 2);
                copy_rect(&mut dl, 0, 3, 2, 0);
                image(&mut dl, FB + 2, 3, 2, 0, 2);
                fill(&mut dl, 0x07c107c1, [0, 0, 2, 1]);
                image(&mut dl, OUTPUT, 3, 2, 0, 2);
                load_copy(&mut dl, FB, 4, 2);
                copy_rect(&mut dl, 1, 1, 2, 32);
                let task = built(DlBuilder::new(), dl);
                memory.0[..FB as usize].copy_from_slice(&task.rdram[..FB as usize]);
                let interpreted = interpret_guest(&r, &memory, task.entry.into());
                assert!(interpreted.diags.is_empty(), "{:?}", interpreted.diags);
                run_compatibility(&mut r, &mut memory, task);
                assert!(r.inner.diagnostics.is_empty(), "{:?}", r.inner.diagnostics);
                assert_words(
                    &memory,
                    FB,
                    &[
                        0x12a3, 0x07c1, 0x07c1, 0x07c1, 0x07c1, 0x07c1, 0x07c1, 0x1ae4,
                    ],
                    crate::FramebufferEncoding::BigEndian,
                );
                assert_words(
                    &memory,
                    OUTPUT,
                    &[0x12a3, 0x07c1, 0x12a3, 0x12a3, 0x07c1, 0x12a3],
                    crate::FramebufferEncoding::BigEndian,
                );
            }
        }
    }
}

#[test]
fn decal_readers_and_copy_observe_immediate_depth_replay() {
    for fallback in [false, true] {
        for scale in [1, 2] {
            let mut r = renderer(fallback);
            r.reconfigure(crate::RendererConfig {
                resolution_multiplier: scale,
                ..r.config
            });
            r.set_guest_writeback_with_encoding(true, crate::FramebufferEncoding::BigEndian);
            let mut memory = Memory(vec![0; OUTPUT as usize + 128]);
            let mut dl = vec![gdp_set_depth_image(Z)];
            image(&mut dl, Z, 8, 8, 0, 2);
            fill(&mut dl, 0x40004000, [0, 0, 7, 7]);
            image(&mut dl, FB, 8, 8, 0, 2);
            fill(&mut dl, 0x00010001, [0, 0, 7, 7]);
            for (z, color, write) in [
                (0x6003, 0xff0000ff, true),
                (0x6006, 0x00ff00ff, false),
                (0x6006, 0x00ff00ff, false),
            ] {
                mode(
                    &mut dl,
                    0,
                    G_ZS_PRIM | ZMODE_DEC | Z_CMP | if write { Z_UPD } else { 0 },
                );
                dl.extend([
                    combine(3, 3),
                    gdp_set_prim_color(0, 0, color),
                    gdp_set_prim_depth(z, 0),
                    gdp_fill_rectangle(8, 8, 24, 24),
                ]);
            }
            load_copy(&mut dl, FB, 8, 8);
            image(&mut dl, OUTPUT, 8, 8, 0, 2);
            copy_rect(&mut dl, 0, 8, 8, 0);
            image(&mut dl, FB, 8, 8, 0, 2);
            for (z, color, write) in [(0x6006, 0x0000ffff, true), (0x6009, 0xffff00ff, false)] {
                mode(
                    &mut dl,
                    0,
                    G_ZS_PRIM | ZMODE_DEC | Z_CMP | if write { Z_UPD } else { 0 },
                );
                dl.extend([
                    combine(3, 3),
                    gdp_set_prim_color(0, 0, color),
                    gdp_set_prim_depth(z, 0),
                    gdp_fill_rectangle(8, 8, 24, 24),
                ]);
            }
            run_compatibility(&mut r, &mut memory, built(DlBuilder::new(), dl));
            assert_words(
                &memory,
                OUTPUT + 45 * 2,
                &[0x07c1],
                crate::FramebufferEncoding::BigEndian,
            );
            assert_words(
                &memory,
                FB + 45 * 2,
                &[0xffc1],
                crate::FramebufferEncoding::BigEndian,
            );
            let (_, _, depth) = crate::tests::depth_persistence::depth_pixels(
                r.device(),
                r.queue(),
                &r.inner,
                Z.into(),
            );
            assert_eq!(depth[45], 196656.0 / 262144.0);
        }
    }
}
