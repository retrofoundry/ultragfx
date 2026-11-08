use super::*;
use crate::{
    annotation::*,
    hle::math::{identity, Mat4},
    scene::{ColorImage, FramebufferPair, Rect, Scissor},
    tests::{
        common::{pixel, pixels_from_render},
        dl_builder::{Built, DlBuilder},
    },
    Hardware, PresentTarget, RdramImage, Renderer, RendererConfig,
};
use n64_gbi::encode::*;

const INTERPOLATED: PhaseResult = PhaseResult::Interpolated {
    restrictions: Vec::new(),
};

struct Image(Vec<u8>);
impl Hardware for Image {
    fn rdram(&self) -> impl crate::Rdram + '_ {
        RdramImage::new(&self.0)
    }
}
struct Fixture {
    built: Built,
    annotations: TaskAnnotations,
}
fn projection() -> Mat4 {
    [
        [1.0 / 32.0, 0.0, 0.0, 0.0],
        [0.0, 1.0 / 24.0, 0.0, 0.0],
        [0.0, 0.0, 0.01, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ]
}
fn transform(degrees: f32, x: f32, y: f32) -> Mat4 {
    let (s, c) = degrees.to_radians().sin_cos();
    [
        [c, s, 0.0, 0.0],
        [-s, c, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
        [x, y, 0.0, 1.0],
    ]
}
fn fixture(models: &[Mat4], view: Mat4, p: Mat4) -> Fixture {
    fixture_with_lighting(models, view, p, false)
}
fn fixture_with_lighting(models: &[Mat4], view: Mat4, p: Mat4, lit: bool) -> Fixture {
    let mut b = DlBuilder::new();
    let vp = b.viewport(Vp {
        vscale: [128, 96, 511, 0],
        vtrans: [128, 96, 511, 0],
    });
    let pa = b.matrix(p);
    let va = b.matrix(view);
    let vertices = b.vertices(&[(-3, -3), (3, -3), (0, 3)].map(|(x, y)| VtxColored {
        x,
        y,
        z: 0,
        flag: 0,
        s: 0,
        t: 0,
        r: 255,
        g: 255,
        b: 255,
        a: 255,
    }));
    let mut commands = vec![
        gdp_set_scissor(0, 0, 0, 256, 192),
        gdp_set_cycle_type(3),
        gdp_set_render_mode(0, 0),
        gdp_set_depth_image(0x20000),
        gdp_set_color_image(0, 2, 64, 0x20000),
        gdp_set_fill_color(0xfffcfffc),
        gdp_fill_rectangle(0, 0, 252, 188),
        gdp_set_color_image(0, 2, 64, 0x10000),
        gdp_set_fill_color(0x00010001),
        gdp_fill_rectangle(0, 0, 252, 188),
        gdp_set_cycle_type(0),
        gdp_set_render_mode(
            n64_gbi::consts::rdp::G_RM_OPA_SURF,
            n64_gbi::consts::rdp::G_RM_OPA_SURF2,
        ),
        gsp_clear_geometrymode(u32::MAX),
        gsp_set_geometrymode(n64_gbi::consts::rsp_f3dex2::G_SHADE),
        gdp_set_combine_lerp(
            CcPass {
                a: 0,
                b: 0,
                c: 0,
                d: 4,
            },
            CcPass {
                a: 0,
                b: 0,
                c: 0,
                d: 4,
            },
            CcPass {
                a: 0,
                b: 0,
                c: 0,
                d: 4,
            },
            CcPass {
                a: 0,
                b: 0,
                c: 0,
                d: 4,
            },
        ),
        gsp_viewport(vp),
        gsp_matrix(pa, true, true, false),
        gsp_matrix(va, true, false, false),
    ];
    if lit {
        commands.extend(b.set_lights(
            [0; 3],
            &[crate::tests::dl_builder::Light {
                color: [255, 0, 0],
                direction: [127, 0, 0],
            }],
        ));
        commands.extend(b.set_look_at(([127, 0, 0], [0, 127, 0])));
        commands.push(gsp_set_geometrymode(
            n64_gbi::consts::rsp_f3dex2::G_LIGHTING | n64_gbi::consts::rsp_f3dex2::G_TEXTURE_GEN,
        ));
    }
    let mut specs = Vec::new();
    for (part, &model) in models.iter().enumerate() {
        let ma = b.matrix(model);
        let leaf = b.list(
            [
                "part0", "part1", "part2", "part3", "part4", "part5", "part6",
            ][part],
            &[
                gsp_matrix(ma, false, true, false),
                gsp_vertex(0, 3, vertices),
                gsp_1triangle(0, 1, 2),
                gsp_enddl(),
            ],
        );
        let draw = commands.len();
        commands.push(gsp_displaylist(leaf));
        specs.push((draw, part));
    }
    commands.push(gsp_enddl());
    let entry = b.list("main", &commands);
    let annotations = TaskAnnotations {
        draws: specs
            .into_iter()
            .map(|(draw, part)| DrawAnnotation {
                scope: InvocationSelector {
                    address: u64::from(entry) + draw as u64 * 8,
                    occurrence: None,
                },
                identity: DrawIdentity {
                    namespace: 7,
                    instance: 11,
                    generation: 1,
                    part: part as u64,
                    subdraw: 0,
                    pass: 0,
                },
                policy: TransformPolicy::Interpolate,
                matrices: [(va, MatrixRole::View), (pa, MatrixRole::Projection)]
                    .map(|(address, role)| MatrixBinding {
                        operand: u64::from(address),
                        role,
                    })
                    .to_vec(),
                palette: None,
            })
            .collect(),
    };
    Fixture {
        built: b.finish("main"),
        annotations,
    }
}
fn renderer(scale: u32) -> Renderer {
    let (device, queue, _) = crate::render::headless_device();
    Renderer::with_device(
        device,
        queue,
        PresentTarget::Headless {
            format: wgpu::TextureFormat::Rgba8Unorm,
            width: 64,
            height: 48,
        },
        RendererConfig {
            resolution_multiplier: scale,
            clear_policy: crate::ClearPolicy::Persist,
            sample_count: 1,
            present_mode: wgpu::PresentMode::Fifo,
            format: None,
            power_preference: wgpu::PowerPreference::LowPower,
        },
    )
}
fn task(
    r: &mut Renderer,
    fixture: Fixture,
    cookie: u64,
    epoch: u64,
    annotated: bool,
) -> TaskHandle {
    r.begin_frame();
    let mut diags = Vec::new();
    let task = r.submit_task(
        &Image(fixture.built.rdram),
        u64::from(fixture.built.entry),
        Microcode::F3dex2,
        &mut diags,
        TaskSubmission {
            info: TaskInfo {
                cookie,
                source: cookie,
                epoch,
                producer_sample: cookie * 10,
            },
            annotations: annotated.then_some(&fixture.annotations),
            microcode_bindings: &[],
        },
    );
    assert!(diags.is_empty(), "{diags:?}");
    assert!(task.summary.renderable);
    task
}
fn source(r: &mut Renderer, task: TaskHandle) -> SourceFrame {
    r.commit_source(
        std::slice::from_ref(&task),
        SourceScanout {
            task_cookie: task.info.cookie,
            address: 0x10000,
            rows: None,
            visible_at: task.info.cookie * 100,
        },
    )
    .unwrap()
}
fn pixels(r: &mut Renderer, draw: impl FnOnce(&mut Renderer, &wgpu::TextureView)) -> Vec<u8> {
    pixels_from_render(
        &r.device().clone(),
        &r.queue().clone(),
        64,
        48,
        wgpu::TextureFormat::Rgba8Unorm,
        |view| draw(r, view),
    )
}
fn phase(
    r: &mut Renderer,
    a: &SourceFrame,
    b: &SourceFrame,
    t: f32,
    expected: PhaseResult,
) -> Vec<u8> {
    pixels(r, |r, view| {
        assert_eq!(r.render_phase(a, b, t, view).unwrap(), expected);
    })
}
fn canonical(r: &mut Renderer) -> Vec<u8> {
    pixels(r, |r, view| r.present_host_to(Some(0x10000), view))
}

#[test]
fn held_presents_preserve_scratch_renderer_and_pair_cache() {
    let mut r = renderer(1);
    let ta = task(
        &mut r,
        fixture(&[identity()], identity(), projection()),
        1,
        1,
        true,
    );
    let a = source(&mut r, ta);
    let tb = task(
        &mut r,
        fixture(&[transform(0.0, 12.0, 0.0)], identity(), projection()),
        2,
        1,
        true,
    );
    let b = source(&mut r, tb);
    let tc = task(
        &mut r,
        fixture(&[identity()], identity(), projection()),
        3,
        2,
        true,
    );
    let cut = source(&mut r, tc);
    let mut inherited = fixture(&[identity()], identity(), projection());
    omit_clears(&mut inherited);
    let td = task(&mut r, inherited, 4, 1, true);
    let ineligible = source(&mut r, td);
    assert!(ineligible
        .ineligibility()
        .iter()
        .any(|r| r.reason == Ineligibility::InheritedAttachmentContents));
    let expected = phase(&mut r, &a, &b, 0.5, INTERPOLATED);
    let state = r.phases.as_ref().unwrap();
    let scratch = &state.renderer as *const IsolatedRenderer;
    let draws = state
        .pair
        .as_ref()
        .unwrap()
        .correspondence
        .as_ref()
        .unwrap()
        .draws
        .as_ptr();
    for held in [&a, &cut, &ineligible] {
        phase(&mut r, &a, held, 0.5, PhaseResult::Held);
        let state = r
            .phases
            .as_ref()
            .expect("held presents keep the scratch renderer");
        assert_eq!(&state.renderer as *const IsolatedRenderer, scratch);
        assert_eq!(phase(&mut r, &a, &b, 0.5, INTERPOLATED), expected);
        assert_eq!(
            r.phases
                .as_ref()
                .unwrap()
                .pair
                .as_ref()
                .unwrap()
                .correspondence
                .as_ref()
                .unwrap()
                .draws
                .as_ptr(),
            draws
        );
    }
}

#[test]
fn opposite_determinant_signs_hold_the_draw_until_b() {
    let mut r = renderer(1);
    let ta = task(
        &mut r,
        fixture(&[identity()], identity(), projection()),
        1,
        1,
        true,
    );
    let canonical_a = canonical(&mut r);
    let a = source(&mut r, ta);
    let mut reflected = transform(0.0, 12.0, 0.0);
    reflected[0][0] = -1.0;
    let tb = task(
        &mut r,
        fixture(&[reflected], identity(), projection()),
        2,
        1,
        true,
    );
    let canonical_b = canonical(&mut r);
    let b = source(&mut r, tb);
    for t in [0.25, 0.5, 0.75] {
        assert_eq!(
            phase(
                &mut r,
                &a,
                &b,
                t,
                restricted(
                    RestrictionReason::DecompositionRejected,
                    &[a.data
                        .task
                        .scene
                        .annotations
                        .as_ref()
                        .unwrap()
                        .sidecar
                        .draws[0]
                        .identity]
                )
            ),
            canonical_a
        );
    }
    assert_eq!(
        phase(&mut r, &a, &b, 1.0, PhaseResult::Endpoint),
        canonical_b
    );
}

#[test]
fn source_commit_rejects_mixed_source_ids_and_epochs() {
    let mut r = renderer(1);
    let a = task(
        &mut r,
        fixture(&[identity()], identity(), projection()),
        1,
        1,
        true,
    );
    for info in [
        TaskInfo {
            cookie: 2,
            source: 2,
            ..a.info
        },
        TaskInfo {
            cookie: 2,
            epoch: 2,
            ..a.info
        },
    ] {
        let mut b = a.clone();
        b.info = info;
        assert!(matches!(
            r.commit_source(
                &[a.clone(), b],
                SourceScanout {
                    task_cookie: 2,
                    address: 0x10000,
                    rows: None,
                    visible_at: 200,
                }
            ),
            Err(SourceError::SourceMismatch)
        ));
    }
}

#[test]
fn rigid_limb_landmarks_and_discrete_endpoint_ownership() {
    let mut r = renderer(1);
    let ta = task(
        &mut r,
        fixture(
            &[transform(0.0, -14.0, -6.0), transform(0.0, 10.0, 7.0)],
            identity(),
            projection(),
        ),
        1,
        1,
        true,
    );
    let ca = canonical(&mut r);
    let a = source(&mut r, ta);
    let tb = task(
        &mut r,
        fixture(
            &[transform(90.0, -6.0, -6.0), transform(-90.0, 18.0, 7.0)],
            identity(),
            projection(),
        ),
        2,
        1,
        true,
    );
    let cb = canonical(&mut r);
    let b = source(&mut r, tb);
    assert!(a.ineligibility().is_empty(), "{:?}", a.ineligibility());
    assert!(b.ineligibility().is_empty(), "{:?}", b.ineligibility());
    for t in [0.25, 0.5, 0.75] {
        let (ascene, bscene) = (&a.data.task.scene, &b.data.task.scene);
        let geometry = phase_scene(ascene, bscene, &correspondence(ascene, bscene).unwrap(), t);
        for (part, (base_x, base_y, direction)) in [(-14.0, -6.0, 1.0), (10.0, 7.0, -1.0)]
            .into_iter()
            .enumerate()
        {
            let (s, c) = (direction * 90.0 * t).to_radians().sin_cos();
            for (corner, [x, y]) in [[-3.0, -3.0], [3.0, -3.0], [0.0, 3.0]]
                .into_iter()
                .enumerate()
            {
                let v = geometry.indices[part * 3 + corner] as usize;
                let clip = crate::hle::math::mul_row_vec4(
                    [x, y, 0.0, 1.0],
                    geometry.mvp_table[geometry.mtx_index[v] as usize],
                );
                assert!(
                    (32.0 + clip[0] * 32.0 - (32.0 + base_x + 8.0 * t + x * c - y * s)).abs() < 1.0
                );
                assert!((24.0 - clip[1] * 24.0 - (24.0 - base_y - x * s - y * c)).abs() < 1.0);
            }
        }
        let image = phase(&mut r, &a, &b, t, INTERPOLATED);
        for (x, y) in [(-14.0 + 8.0 * t, -6.0), (10.0 + 8.0 * t, 7.0)] {
            let (px, py) = ((32.0 + x).round() as u32, (24.0f32 - y).round() as u32);
            assert_eq!(
                pixel(&image, 64, px, py),
                [255; 4],
                "phase {t}, ({px},{py})"
            );
        }
    }
    assert_eq!(phase(&mut r, &a, &b, 0.0, PhaseResult::Endpoint), ca);
    assert_eq!(phase(&mut r, &a, &b, 1.0, PhaseResult::Endpoint), cb);
}

#[test]
fn camera_orbit_projects_landmarks_within_one_native_pixel() {
    let yaw = |degrees: f32| {
        let (s, c) = degrees.to_radians().sin_cos();
        [
            [c, 0.0, -s, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [s, 0.0, c, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ]
    };
    let mut r = renderer(1);
    let ta = task(
        &mut r,
        fixture(&[transform(0.0, 18.0, 5.0)], yaw(-15.0), projection()),
        11,
        1,
        true,
    );
    let a = source(&mut r, ta);
    let tb = task(
        &mut r,
        fixture(&[transform(0.0, 18.0, 5.0)], yaw(15.0), projection()),
        12,
        1,
        true,
    );
    let b = source(&mut r, tb);
    for t in [0.25, 0.5, 0.75] {
        let at = &a.data.task.scene;
        let bt = &b.data.task.scene;
        let scene = phase_scene(at, bt, &correspondence(at, bt).unwrap(), t);
        let angle = (-15.0 + 30.0 * t).to_radians();
        for (corner, [x, y]) in [[-3.0, -3.0], [3.0, -3.0], [0.0, 3.0]]
            .into_iter()
            .enumerate()
        {
            let vertex = scene.indices[corner] as usize;
            let mvp = scene.mvp_table[scene.mtx_index[vertex] as usize];
            let clip = crate::hle::math::mul_row_vec4([x, y, 0.0, 1.0], mvp);
            let want_x = 32.0 + (18.0 + x) * angle.cos();
            let want_y = 24.0 - (5.0 + y);
            assert!((32.0 + clip[0] / clip[3] * 32.0 - want_x).abs() <= 1.0);
            assert!((24.0 - clip[1] / clip[3] * 24.0 - want_y).abs() <= 1.0);
        }
        let image = phase(&mut r, &a, &b, t, INTERPOLATED);
        let x = (32.0 + 18.0 * angle.cos()).floor() as u32;
        assert_eq!(pixel(&image, 64, x, 19), [255; 4]);
    }
}

#[test]
fn identity_generation_projection_and_discrete_content_hold() {
    let mut r = renderer(1);
    let ta = task(
        &mut r,
        fixture(
            &[transform(0.0, 0.0, 0.0), transform(0.0, 0.0, 0.0)],
            identity(),
            projection(),
        ),
        21,
        1,
        true,
    );
    let a = source(&mut r, ta);
    let tb = task(
        &mut r,
        fixture(
            &[transform(0.0, -10.0, 0.0), transform(0.0, 10.0, 0.0)],
            identity(),
            projection(),
        ),
        22,
        1,
        true,
    );
    let b = source(&mut r, tb);
    let image = phase(&mut r, &a, &b, 0.5, INTERPOLATED);
    assert_eq!(pixel(&image, 64, 27, 24), [255; 4]);
    assert_eq!(pixel(&image, 64, 37, 24), [255; 4]);
    let old = phase(&mut r, &a, &b, 0.0, PhaseResult::Endpoint);
    let mut changed_generation = fixture(&[transform(0.0, 12.0, 0.0)], identity(), projection());
    changed_generation.annotations.draws[0].identity.generation = 2;
    let tc = task(&mut r, changed_generation, 23, 1, true);
    let c = source(&mut r, tc);
    assert_eq!(
        phase(
            &mut r,
            &a,
            &c,
            0.5,
            restricted(
                RestrictionReason::NoMatch,
                &a.data
                    .task
                    .scene
                    .annotations
                    .as_ref()
                    .unwrap()
                    .sidecar
                    .draws
                    .iter()
                    .map(|d| d.identity)
                    .collect::<Vec<_>>()
            )
        ),
        old
    );
    let mut p = projection();
    p[0][0] *= 1.5;
    let td = task(
        &mut r,
        fixture(&[transform(0.0, 12.0, 0.0)], identity(), p),
        24,
        1,
        true,
    );
    let d = source(&mut r, td);
    assert_eq!(phase(&mut r, &a, &d, 0.5, PhaseResult::Held), old);
    let te = task(
        &mut r,
        fixture(&[transform(0.0, 12.0, 0.0)], identity(), projection()),
        25,
        2,
        true,
    );
    let e = source(&mut r, te);
    assert_eq!(phase(&mut r, &a, &e, 0.75, PhaseResult::Held), old);
    assert_ne!(phase(&mut r, &a, &e, 1.0, PhaseResult::Held), old);
}

#[test]
fn endpoints_use_their_own_topology_and_phase_history_is_weak() {
    let mut r = renderer(1);
    let ta = task(
        &mut r,
        fixture(&[transform(0.0, -12.0, 0.0)], identity(), projection()),
        31,
        1,
        true,
    );
    let ca = canonical(&mut r);
    let a = source(&mut r, ta);
    let tb = task(
        &mut r,
        fixture(
            &[transform(0.0, 0.0, 0.0), transform(0.0, 12.0, 0.0)],
            identity(),
            projection(),
        ),
        32,
        1,
        true,
    );
    let cb = canonical(&mut r);
    let b = source(&mut r, tb);
    assert!(r.phases.is_none());
    assert_eq!(phase(&mut r, &a, &b, 0.0, PhaseResult::Endpoint), ca);
    r.phases = None;
    assert_eq!(phase(&mut r, &a, &b, 1.0, PhaseResult::Endpoint), cb);
    let weak_a = Arc::downgrade(&a.data);
    let weak_b = Arc::downgrade(&b.data);
    drop(a);
    drop(b);
    assert!(weak_a.upgrade().is_none());
    assert!(weak_b.upgrade().is_none());
}

fn native(r: &mut Renderer) -> Vec<u8> {
    pixels(r, |r, view| {
        let mut encoder = r.device.create_command_encoder(&Default::default());
        r.inner.scanout_native(
            &r.device,
            &mut encoder,
            view,
            TargetId::Guest(0x10000),
            None,
        );
        r.queue.submit([encoder.finish()]);
    })
}

#[test]
fn phases_preserve_native_bytes_writeback_ownership_rdp_and_serials() {
    let mut r = renderer(1);
    r.set_guest_writeback_with_encoding(true, crate::FramebufferEncoding::BigEndian);
    let mut guest = vec![0; 0x50000];
    let seed = writeback_task(&mut r, dependent_fixture(false), 38, &mut guest);
    let copy = writeback_task(&mut r, history_fixture(History::GuestWords), 39, &mut guest);
    let ta = writeback_task(&mut r, decal_fixture(-8.0, false), 41, &mut guest);
    let a = source(&mut r, ta);
    let tb = writeback_task(&mut r, decal_fixture(8.0, false), 42, &mut guest);
    let b = source(&mut r, tb);
    guest[0x30002] ^= 0x40;
    writeback_task(&mut r, strict_fixture(), 43, &mut guest);
    r.inner
        .framebuffer_copies
        .assert_guest_copy(&r.device, &r.queue);
    assert!(!r
        .inner
        .coherence
        .as_ref()
        .unwrap()
        .owners
        .foreign
        .is_empty());
    assert!(r
        .inner
        .coherence
        .as_ref()
        .unwrap()
        .native_fill_pixels(&r.device, &r.queue)
        .is_some());
    let planes = r.inner.depthbuffers[&TargetId::Guest(0x20000)].test_planes(&r.device, &r.queue);
    assert!(planes[1]
        .as_chunks::<4>()
        .0
        .iter()
        .any(|code| *code != [0; 4]));
    assert_eq!(planes[0], planes[2]);
    let guest_state = r.inner.test_guest_state(&r.device, &r.queue);
    let device = r.device.clone();
    let queue = r.queue.clone();
    let staged = r.inner.stage_strict_task(
        &device,
        &queue,
        TargetId::Guest(0x40000),
        Some(TargetId::Guest(0x20000)),
    );
    let staged_state = r.inner.test_guest_state(&device, &queue);
    let saved_state = staged.test_state(&device, &queue);
    assert!(b.ineligibility().is_empty(), "{:?}", b.ineligibility());
    let before = native(&mut r);
    let writes = r.guest_writes.clone();
    assert!(!writes.is_empty());
    let rdp = r.rdp.clone();
    let p75 = phase(&mut r, &a, &b, 0.75, INTERPOLATED);
    for t in [0.25, 0.0, 1.0, 0.5, 0.25, 0.75]
        .into_iter()
        .cycle()
        .take(24)
    {
        phase(
            &mut r,
            &a,
            &b,
            t,
            if t == 0.0 || t == 1.0 {
                PhaseResult::Endpoint
            } else {
                INTERPOLATED
            },
        );
    }
    assert_eq!(phase(&mut r, &a, &b, 0.75, INTERPOLATED), p75);
    assert_eq!(native(&mut r), before);
    assert_eq!(r.guest_writes, writes);
    assert_eq!(r.rdp, rdp);
    assert_eq!(r.inner.test_guest_state(&device, &queue), staged_state);
    assert_eq!(staged.test_state(&device, &queue), saved_state);
    r.inner.discard_strict_task(staged);
    assert_eq!(r.inner.test_guest_state(&device, &queue), guest_state);
    drop((seed, copy));
}

#[test]
fn optional_sidecar_records_nothing_and_preserves_all_canonical_readbacks() {
    let mut r = renderer(1);
    let f = fixture(&[transform(0.0, 4.0, 0.0)], identity(), projection());
    let hw = Image(f.built.rdram.clone());
    r.begin_frame();
    r.process_dl(
        &hw,
        u64::from(f.built.entry),
        Microcode::F3dex2,
        &mut crate::NopSink,
    );
    let old = native(&mut r);
    let old_scene = r.frame_scenes[0].clone();
    r.reset();
    let task = task(&mut r, f, 51, 1, false);
    assert!(task.retained.is_none());
    assert!(r.frame_scenes[0].annotations.is_none());
    assert_eq!(r.frame_scenes[0], old_scene);
    assert_eq!(native(&mut r), old);
    assert!(matches!(
        r.commit_source(
            &[task],
            SourceScanout {
                task_cookie: 51,
                address: 0x10000,
                rows: None,
                visible_at: 50
            }
        ),
        Err(SourceError::UnrecordedTask)
    ));
}

#[test]
fn light_and_lookat_refolding_uses_model_without_view() {
    let mut r = renderer(1);
    let ta = task(
        &mut r,
        fixture_with_lighting(&[identity()], transform(90.0, 0.0, 0.0), projection(), true),
        61,
        1,
        true,
    );
    let a = source(&mut r, ta);
    let tb = task(
        &mut r,
        fixture_with_lighting(
            &[transform(90.0, 0.0, 0.0)],
            transform(90.0, 0.0, 0.0),
            projection(),
            true,
        ),
        62,
        1,
        true,
    );
    let b = source(&mut r, tb);
    let (a, b) = (&a.data.task.scene, &b.data.task.scene);
    let phase = phase_scene(a, b, &correspondence(a, b).unwrap(), 0.5);
    let vertex = phase.indices[0] as usize;
    let light = phase.lights_table[phase.light_index[vertex] as usize].0;
    let lookat = phase.lookat_table[phase.lookat_index[vertex] as usize];
    let root = std::f32::consts::FRAC_1_SQRT_2;
    for (got, want) in light.into_iter().zip([root, -root, 0.0]) {
        assert!((got - want).abs() < 0.0001, "{got} != {want}");
    }
    for (got, want) in lookat.0.into_iter().zip([root, -root, 0.0]) {
        assert!((got - want).abs() < 0.0001);
    }
    for (got, want) in lookat.1.into_iter().zip([root, root, 0.0]) {
        assert!((got - want).abs() < 0.0001);
    }
}

fn dependent_fixture(load: bool) -> Fixture {
    let mut commands = vec![
        gdp_set_color_image(0, 2, 64, 0x30000),
        gdp_set_scissor(0, 0, 0, 256, 192),
        gdp_set_cycle_type(3),
        gdp_set_fill_color(0xf801f801),
        gdp_fill_rectangle(0, 0, 252, 188),
        gdp_set_color_image(0, 2, 64, 0x10000),
        gdp_set_cycle_type(2),
        gdp_set_texture_image(0, 2, 64, 0x30000),
        gdp_set_tile(0, 2, 16, 0, 0, 0, 2, 0, 0, 2, 0, 0),
        gdp_set_tile_size(0, 0, 0, 252, 188),
    ];
    if load {
        commands.push(gdp_load_block(0, 0, 0, 15, 0));
    }
    commands.extend(gsp_texture_rectangle(
        0, 0, 256, 192, 0, 0, 0, 1024, 1024, false,
    ));
    commands.push(gsp_enddl());
    let mut b = DlBuilder::new();
    b.list("main", &commands);
    Fixture {
        built: b.finish("main"),
        annotations: TaskAnnotations::default(),
    }
}
fn omit_clears(f: &mut Fixture) {
    let start = f.built.entry as usize;
    for command in f.built.rdram[start..].as_chunks_mut::<8>().0 {
        if command[0] == 0xf6 {
            let (a, b) = gdp_pipe_sync();
            command[..4].copy_from_slice(&a.to_be_bytes());
            command[4..].copy_from_slice(&b.to_be_bytes());
        }
    }
}

#[test]
fn framebuffer_load_copy_alias_and_inherited_sources_repeat_frozen_endpoints() {
    let mut r = renderer(1);
    for load in [false, true] {
        r.reset();
        let ta = task(
            &mut r,
            fixture(&[identity()], identity(), projection()),
            71,
            1,
            true,
        );
        let ca = canonical(&mut r);
        let a = source(&mut r, ta);
        let tb = task(&mut r, dependent_fixture(load), 72, 1, true);
        let cb = canonical(&mut r);
        let b = source(&mut r, tb);
        let reason = if load {
            Ineligibility::FramebufferTextureLoad
        } else {
            Ineligibility::LiveFramebufferAlias
        };
        assert!(
            b.ineligibility().iter().any(|r| r.reason == reason),
            "{:?}",
            b.ineligibility()
        );
        if load {
            assert!(b
                .ineligibility()
                .iter()
                .any(|r| r.reason == Ineligibility::FramebufferCopy));
        }
        assert_eq!(phase(&mut r, &a, &b, 0.5, PhaseResult::Held), ca);
        assert_eq!(phase(&mut r, &a, &b, 1.0, PhaseResult::Held), cb);
    }
    r.reset();
    let ta = task(
        &mut r,
        fixture(&[transform(0.0, -12.0, 0.0)], identity(), projection()),
        73,
        1,
        true,
    );
    let ca = canonical(&mut r);
    let a = source(&mut r, ta);
    let mut inherited = fixture(&[transform(0.0, 12.0, 0.0)], identity(), projection());
    omit_clears(&mut inherited);
    let tb = task(&mut r, inherited, 74, 1, true);
    let b = source(&mut r, tb);
    assert!(b
        .ineligibility()
        .iter()
        .any(|r| r.reason == Ineligibility::InheritedAttachmentContents));
    assert_eq!(phase(&mut r, &a, &b, 0.75, PhaseResult::Held), ca);
}

#[test]
fn cpu_import_and_multitask_sources_hold_and_visibility_uses_exact_cookie() {
    let mut r = renderer(1);
    r.set_guest_writeback_with_encoding(true, crate::FramebufferEncoding::BigEndian);
    let mut fa = fixture(&[identity()], identity(), projection());
    fa.built.rdram.resize(0x22000, 0);
    let ta = task(&mut r, fa, 81, 1, true);
    let ca = canonical(&mut r);
    let a = source(&mut r, ta.clone());
    let mut fb = fixture(&[transform(0.0, 12.0, 0.0)], identity(), projection());
    omit_clears(&mut fb);
    fb.built.rdram.resize(0x22000, 0);
    for patch in r.take_guest_writes() {
        fb.built.rdram[patch.address as usize..patch.address as usize + patch.bytes.len()]
            .copy_from_slice(&patch.bytes);
    }
    fb.built.rdram[0x10000] ^= 0xff;
    let mut tb = task(&mut r, fb, 82, 1, true);
    let b = source(&mut r, tb.clone());
    assert!(b
        .ineligibility()
        .iter()
        .any(|r| r.reason == Ineligibility::CpuImportDependency));
    assert_eq!(phase(&mut r, &a, &b, 0.5, PhaseResult::Held), ca);
    tb.info.source = ta.info.source;
    let multi = r
        .commit_source(
            &[ta, tb],
            SourceScanout {
                task_cookie: 82,
                address: 0x10000,
                rows: Some(0..48),
                visible_at: 123456,
            },
        )
        .unwrap();
    assert!(multi
        .ineligibility()
        .iter()
        .any(|r| r.reason == Ineligibility::Multitask));
    assert_eq!(multi.visible_at(), 123456);
    assert_eq!(phase(&mut r, &a, &multi, 0.5, PhaseResult::Held), ca);
}

#[test]
fn phases_compose_with_scale_reduction_and_crop() {
    let mut r = renderer(2);
    r.set_reduction_factor(2).unwrap();
    let ta = task(
        &mut r,
        fixture(&[transform(0.0, -8.0, 0.0)], identity(), projection()),
        91,
        1,
        true,
    );
    let a = r
        .commit_source(
            &[ta],
            SourceScanout {
                task_cookie: 91,
                address: 0x10000,
                rows: Some(4..44),
                visible_at: 900,
            },
        )
        .unwrap();
    let tb = task(
        &mut r,
        fixture(&[transform(0.0, 8.0, 0.0)], identity(), projection()),
        92,
        1,
        true,
    );
    let b = r
        .commit_source(
            &[tb],
            SourceScanout {
                task_cookie: 92,
                address: 0x10000,
                rows: Some(4..44),
                visible_at: 901,
            },
        )
        .unwrap();
    let native_before = native(&mut r);
    let canonical_crop = pixels(&mut r, |r, view| {
        let mut encoder = r.device.create_command_encoder(&Default::default());
        r.inner.scanout(
            &r.device,
            &mut encoder,
            view,
            TargetId::Guest(0x10000),
            Some(4..44),
        );
        r.queue.submit([encoder.finish()]);
    });
    assert_eq!(
        phase(&mut r, &a, &b, 1.0, PhaseResult::Endpoint),
        canonical_crop
    );
    let middle = phase(&mut r, &a, &b, 0.5, INTERPOLATED);
    assert_eq!(pixel(&middle, 64, 32, 24), [255; 4]);
    assert_eq!(native(&mut r), native_before);
}

#[test]
fn canonical_diagnostics_make_a_source_ineligible() {
    let mut r = renderer(1);
    let mut f = fixture(&[identity()], identity(), projection());
    for command in f.built.rdram[f.built.entry as usize..]
        .as_chunks_mut::<8>()
        .0
    {
        if command[0] == 0xe2 {
            command[4..].fill(0);
        }
    }
    r.begin_frame();
    let task = r.submit_task(
        &Image(f.built.rdram),
        u64::from(f.built.entry),
        Microcode::F3dex2,
        &mut crate::NopSink,
        TaskSubmission {
            info: TaskInfo {
                cookie: 101,
                source: 101,
                epoch: 1,
                producer_sample: 100,
            },
            annotations: Some(&f.annotations),
            microcode_bindings: &[],
        },
    );
    assert!(task.summary.warns > 0);
    let frame = source(&mut r, task);
    assert!(!frame.ineligibility().is_empty());
}

#[test]
fn fractional_scissor_cannot_establish_inherited_presentation_columns() {
    let mut r = renderer(2);
    let mut fa = fixture(&[identity()], identity(), projection());
    for command in fa.built.rdram[fa.built.entry as usize..]
        .as_chunks_mut::<8>()
        .0
    {
        if command[0] == 0xf7 && command[4..] == 0x00010001u32.to_be_bytes() {
            command[4..].copy_from_slice(&0x07c107c1u32.to_be_bytes());
        }
    }
    let ta = task(&mut r, fa, 111, 1, true);
    let a = source(&mut r, ta);
    let mut fb = fixture(&[identity()], identity(), projection());
    let start = fb.built.entry as usize;
    let (w0, w1) = gdp_set_scissor(0, 1, 0, 256, 192);
    fb.built.rdram[start..start + 4].copy_from_slice(&w0.to_be_bytes());
    fb.built.rdram[start + 4..start + 8].copy_from_slice(&w1.to_be_bytes());
    let tb = task(&mut r, fb, 112, 1, true);
    let canonical = canonical(&mut r);
    let b = source(&mut r, tb);
    assert!(
        phase(&mut r, &a, &b, 1.0, PhaseResult::Held) == canonical,
        "presentation columns retain the prior target until established"
    );
    assert!(b
        .ineligibility()
        .iter()
        .any(|r| r.reason == Ineligibility::InheritedAttachmentContents));
}

fn context() -> IsolatedContext {
    IsolatedContext {
        frame_serial: 1,
        dither_seed: 3,
        render_scale: 1,
        legacy_extent: (32, 24),
    }
}
fn clear_scene() -> Scene {
    Scene {
        framebuffer_pairs: vec![FramebufferPair {
            color_image: ColorImage {
                fmt: 0,
                siz: 2,
                width: 32,
                addr: 0x1000,
            },
            size_extent: (32, 24),
            active_scissor: Scissor {
                ulx: 0,
                uly: 0,
                lrx: 32,
                lry: 24,
                mode: 0,
            },
            ops: vec![SceneOp::FillRect {
                rect: Rect {
                    ulx: 0,
                    uly: 0,
                    lrx: 31,
                    lry: 23,
                },
                color_raw: 0,
                convert: [0; 6],
                key: Default::default(),
            }],
            ..Default::default()
        }],
        ..Default::default()
    }
}
#[test]
fn old_layout_and_color_fills_cannot_establish_new_attachments() {
    let mut scene = clear_scene();
    let mut changed = scene.framebuffer_pairs[0].clone();
    changed.color_image.siz = 3;
    changed.ops.clear();
    scene.framebuffer_pairs.push(changed);
    assert!(eligibility(&scene, context(), false, &HashMap::new())
        .iter()
        .any(|r| r.reason == Ineligibility::InheritedAttachmentContents));

    let mut scene = clear_scene();
    let mut draw = scene.framebuffer_pairs[0].clone();
    scene
        .render_modes
        .push(crate::hle::decode_render_mode(0, 0, 0));
    draw.color_image.addr = 0x3000;
    draw.depth_image = Some(0x1000);
    draw.ops.push(SceneOp::Tris(crate::scene::DrawRun {
        material_index: 0,
        render_mode_index: 0,
        fog_color: [0; 4],
        prim_depth: Default::default(),
        cull: crate::scene::CullKind::None,
        unclipped_depth: false,
        index_count: 3,
        index_start: 0,
    }));
    scene.draw_origins.push(crate::scene::DrawOrigin {
        pc: 0,
        depth_image: Some(0x1000),
        scissor: draw.active_scissor,
        presentation_scissor: draw.active_scissor,
        indices: 0..3,
        rectangle: None,
    });
    scene.framebuffer_pairs.push(draw);
    assert!(eligibility(&scene, context(), false, &HashMap::new())
        .iter()
        .any(|r| r.reason == Ineligibility::InheritedAttachmentContents));
}

#[test]
fn inherited_attachment_and_imports_are_rejected() {
    let mut scene = clear_scene();
    assert!(eligibility(&scene, context(), false, &HashMap::new()).is_empty());
    scene.framebuffer_pairs[0].ops.clear();
    assert!(eligibility(&scene, context(), false, &HashMap::new())
        .iter()
        .any(|r| r.reason == Ineligibility::InheritedAttachmentContents));
    assert!(
        eligibility(&clear_scene(), context(), true, &HashMap::new())
            .iter()
            .any(|r| r.reason == Ineligibility::CpuImportDependency)
    );
}

fn append_commands(f: &mut Fixture, commands: &[(u32, u32)]) {
    f.built.rdram.truncate(f.built.rdram.len() - 8);
    for (a, b) in commands.iter().copied().chain([gsp_enddl()]) {
        f.built.rdram.extend_from_slice(&a.to_be_bytes());
        f.built.rdram.extend_from_slice(&b.to_be_bytes());
    }
}

fn decal_fixture(x: f32, inherited: bool) -> Fixture {
    use n64_gbi::consts::*;
    let mut f = fixture(&[transform(0.0, x, 0.0)], identity(), projection());
    if inherited {
        let start = f.built.entry as usize;
        let command = f.built.rdram[start..]
            .as_chunks_mut::<8>()
            .0
            .iter_mut()
            .find(|c| c[0] == 0xf6)
            .unwrap();
        let (a, b) = gdp_pipe_sync();
        command[..4].copy_from_slice(&a.to_be_bytes());
        command[4..].copy_from_slice(&b.to_be_bytes());
    }
    let prim = CcPass {
        a: ZERO_C,
        b: ZERO_C,
        c: ZERO_C,
        d: 3,
    };
    let alpha = CcPass {
        a: ZERO_A,
        b: ZERO_A,
        c: ZERO_A,
        d: 3,
    };
    append_commands(
        &mut f,
        &[
            gdp_pipe_sync(),
            gdp_set_combine_lerp(prim, alpha, prim, alpha),
            gdp_set_prim_color(0, 0, 0xff0000ff),
            gdp_set_prim_depth(0x7000, 32),
            gdp_set_render_mode(
                G_ZS_PRIM | if inherited { 0 } else { Z_UPD } | CVG_DST_FULL,
                0,
            ),
            gdp_fill_rectangle(0, 0, 48, 48),
            gdp_pipe_sync(),
            gdp_set_prim_color(0, 0, 0x00ff00ff),
            gdp_set_prim_depth(0x7020, 1),
            gdp_set_render_mode(G_ZS_PRIM | Z_CMP | ZMODE_DEC | CVG_DST_FULL, 0),
            gdp_fill_rectangle(0, 0, 48, 48),
        ],
    );
    f
}

fn writeback_task(r: &mut Renderer, mut f: Fixture, cookie: u64, guest: &mut [u8]) -> TaskHandle {
    guest[..f.built.rdram.len()].copy_from_slice(&f.built.rdram);
    f.built.rdram = guest.to_vec();
    let task = task(r, f, cookie, 1, true);
    for patch in &r.guest_writes {
        guest[patch.address as usize..patch.address as usize + patch.bytes.len()]
            .copy_from_slice(&patch.bytes);
    }
    task
}

#[test]
fn in_frame_decal_depth_is_eligible_and_uncached_endpoints_match_with_writeback() {
    for copy in [false, true] {
        let mut r = renderer(1);
        let mut control = renderer(1);
        r.set_guest_writeback_with_encoding(true, crate::FramebufferEncoding::BigEndian);
        control.set_guest_writeback_with_encoding(true, crate::FramebufferEncoding::BigEndian);
        let mut guest = vec![0; 0x40000];
        let mut control_guest = guest.clone();
        let mut frames = Vec::new();
        let mut expected = Vec::new();
        for (cookie, x) in [(121, -8.0), (122, 8.0)] {
            let make = || {
                if copy {
                    let mut f = dependent_fixture(true);
                    if cookie == 122 {
                        for command in f.built.rdram[f.built.entry as usize..]
                            .as_chunks_mut::<8>()
                            .0
                        {
                            if command[0] == 0xf7 {
                                command[4..].copy_from_slice(&0x07c107c1u32.to_be_bytes());
                            }
                        }
                    }
                    f
                } else {
                    decal_fixture(x, false)
                }
            };
            let task = writeback_task(&mut r, make(), cookie, &mut guest);
            writeback_task(&mut control, make(), cookie, &mut control_guest);
            expected.push(canonical(&mut r));
            assert_eq!(expected.last().unwrap(), &canonical(&mut control));
            frames.push(source(&mut r, task));
        }
        assert_ne!(expected[0], expected[1]);
        for (t, index) in [(0.0, 0), (1.0, 1)] {
            let frame = &frames[index];
            if copy {
                assert!(frame
                    .ineligibility()
                    .iter()
                    .any(|r| r.reason == Ineligibility::FramebufferCopy));
            } else {
                assert!(
                    frame.ineligibility().is_empty(),
                    "{:?}",
                    frame.ineligibility()
                );
                assert_eq!(pixel(&expected[index], 64, 5, 5), [0, 255, 0, 255]);
            }
            r.phases = None;
            assert_eq!(
                phase(
                    &mut r,
                    &frames[0],
                    &frames[1],
                    t,
                    if copy {
                        PhaseResult::Held
                    } else {
                        PhaseResult::Endpoint
                    }
                ),
                expected[index]
            );
            let mut isolated = IsolatedRenderer::new(&r.device, r.surface_format, 64, 48, false);
            let image = isolated
                .render(
                    &r.device,
                    &r.queue,
                    &frame.data.task.scene,
                    frame.data.task.context,
                    frame.data.target,
                )
                .unwrap();
            let actual = pixels(&mut r, |r, view| {
                let mut encoder = r.device.create_command_encoder(&Default::default());
                r.inner
                    .scanout_snapshot(&image, None, &r.device, &mut encoder, view);
                r.queue.submit([encoder.finish()]);
            });
            assert_eq!(actual, expected[index], "copy {copy}, t={t}");
        }
        assert_eq!(guest, control_guest);
        assert_eq!(r.guest_writes, control.guest_writes);
        let followup = || {
            if copy {
                history_fixture(History::GuestWords)
            } else {
                decal_fixture(0.0, true)
            }
        };
        writeback_task(&mut r, followup(), 123, &mut guest);
        writeback_task(&mut control, followup(), 123, &mut control_guest);
        assert_eq!(guest, control_guest);
        assert_eq!(r.guest_writes, control.guest_writes);
    }
}

#[test]
fn inherited_decal_depth_holds_but_in_frame_snapshots_do_not() {
    let mut r = renderer(1);
    let ta = task(&mut r, decal_fixture(-8.0, false), 131, 1, true);
    let ca = canonical(&mut r);
    let a = source(&mut r, ta);
    assert!(a.ineligibility().is_empty());
    let tb = task(&mut r, decal_fixture(8.0, true), 132, 1, true);
    let cb = canonical(&mut r);
    let b = source(&mut r, tb);
    assert_eq!(pixel(&cb, 64, 5, 5), [0, 255, 0, 255]);
    assert!(
        b.ineligibility()
            .iter()
            .any(|r| r.reason == Ineligibility::InheritedDecalDepth),
        "{:?}",
        b.ineligibility()
    );
    assert_eq!(phase(&mut r, &a, &b, 0.5, PhaseResult::Held), ca);
    assert_eq!(phase(&mut r, &a, &b, 1.0, PhaseResult::Held), cb);
}

fn strict_fixture() -> Fixture {
    let mut b = DlBuilder::new();
    b.list(
        "main",
        &[
            gdp_set_depth_image(0x20000),
            gdp_set_color_image(0, 2, 64, 0x40000),
            gdp_set_scissor(0, 0, 0, 256, 192),
            gdp_set_cycle_type(3),
            gdp_set_render_mode(0, 0),
            gdp_set_fill_color(0x07c107c1),
            gdp_fill_rectangle(0, 0, 252, 188),
            gsp_enddl(),
        ],
    );
    Fixture {
        built: b.finish("main"),
        annotations: TaskAnnotations::default(),
    }
}

#[derive(Clone, Copy)]
enum History {
    PauseBackground,
    VisCvg,
    Ia16ToI8,
    TextureCopy,
    GuestWords,
}

fn history_fixture(history: History) -> Fixture {
    use n64_gbi::consts::*;
    let mut commands = vec![
        gdp_set_color_image(0, 2, 64, 0x10000),
        gdp_set_scissor(0, 0, 0, 256, 192),
        gdp_set_render_mode(0, 0),
        gdp_set_cycle_type(3),
    ];
    if !matches!(history, History::VisCvg) {
        commands.extend([
            gdp_set_fill_color(0x00010001),
            gdp_fill_rectangle(0, 0, 252, 188),
        ]);
    }
    match history {
        History::VisCvg => {
            let viscvg = IM_RD
                | FORCE_BL
                | gbl_c1(CLR_IN, A_0, CLR_BL, B_A_MEM)
                | gbl_c2(CLR_IN, A_0, CLR_BL, B_A_MEM);
            commands.extend([
                gdp_set_cycle_type(0),
                gdp_set_render_mode(G_ZS_PRIM | viscvg, 0),
                gdp_set_blend_color(0xffffff08),
                gdp_set_prim_depth(0xffff, 0xffff),
                gdp_fill_rectangle(0, 0, 256, 192),
            ]);
        }
        _ => {
            let ia = matches!(history, History::Ia16ToI8);
            let fmt = if ia { 3 } else { 0 };
            if ia {
                commands.push(gdp_set_color_image(4, 1, 64, 0x10000));
            }
            commands.extend([
                gdp_set_cycle_type(2),
                gdp_set_texture_image(fmt, 2, 64, 0x30000),
                gdp_set_tile(fmt, 2, 16, 0, 0, 0, 2, 0, 0, 2, 0, 0),
            ]);
            if !matches!(history, History::PauseBackground) {
                commands.push((0xf4000000 | (4 << 12) | 4, (32 << 12) | 8));
            }
            commands.push(gdp_set_tile_size(0, 0, 0, 28, 4));
            commands.extend(gsp_texture_rectangle(
                0, 0, 28, 4, 0, 0, 0, 4096, 1024, false,
            ));
        }
    }
    commands.push(gsp_enddl());
    let mut b = DlBuilder::new();
    b.list("main", &commands);
    Fixture {
        built: b.finish("main"),
        annotations: TaskAnnotations::default(),
    }
}

fn assert_history_holds(history: History, reason: Ineligibility) {
    let mut r = renderer(1);
    let writeback = matches!(
        history,
        History::GuestWords | History::VisCvg | History::Ia16ToI8
    );
    r.set_guest_writeback_with_encoding(writeback, crate::FramebufferEncoding::BigEndian);
    let mut guest = vec![0; 0x40000];
    let seed = writeback_task(&mut r, dependent_fixture(false), 141, &mut guest);
    let ta = writeback_task(&mut r, decal_fixture(-8.0, false), 142, &mut guest);
    let ca = canonical(&mut r);
    let a = source(&mut r, ta);
    assert!(a.ineligibility().is_empty(), "{:?}", a.ineligibility());
    let tb = writeback_task(&mut r, history_fixture(history), 143, &mut guest);
    let cb = canonical(&mut r);
    let b = source(&mut r, tb);
    assert!(
        b.ineligibility().iter().any(|r| r.reason == reason),
        "{:?}",
        b.ineligibility()
    );
    if matches!(history, History::GuestWords) {
        r.inner
            .framebuffer_copies
            .assert_guest_copy(&r.device, &r.queue);
    }
    for t in [0.0, 0.25, 0.75] {
        assert_eq!(phase(&mut r, &a, &b, t, PhaseResult::Held), ca);
    }
    assert_eq!(phase(&mut r, &a, &b, 1.0, PhaseResult::Held), cb);
    drop(seed);
}

#[test]
fn prerender_pause_background_history_holds() {
    assert_history_holds(
        History::PauseBackground,
        Ineligibility::LiveFramebufferAlias,
    );
}

#[test]
fn viscvg_coverage_history_holds() {
    assert_history_holds(History::VisCvg, Ineligibility::InheritedAttachmentContents);
}

#[test]
fn ia16_to_i8_framebuffer_copy_history_holds() {
    assert_history_holds(History::Ia16ToI8, Ineligibility::FramebufferTextureLoad);
}

#[test]
fn modern_framebuffer_texture_history_holds() {
    assert_history_holds(History::TextureCopy, Ineligibility::FramebufferCopy);
}

#[test]
fn guest_word_framebuffer_texture_history_holds() {
    assert_history_holds(History::GuestWords, Ineligibility::FramebufferTextureLoad);
}

#[test]
fn hold_model_keeps_a_world_pose_under_the_phase_camera() {
    let yaw = |s: f32, c: f32| {
        [
            [c, 0.0, -s, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [s, 0.0, c, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ]
    };
    let mut r = renderer(1);
    let mut held_a = transform(0.0, 12.0, 5.0);
    held_a[3][2] = 8.0;
    let mut held_b = transform(90.0, -18.0, 12.0);
    held_b[3][2] = -8.0;
    let mut landmark = transform(0.0, -12.0, -5.0);
    landmark[3][2] = -4.0;
    let mut fa = fixture(
        &[held_a, landmark],
        yaw(-0.5, 30.0_f32.to_radians().cos()),
        projection(),
    );
    fa.annotations.draws[0].policy = TransformPolicy::HoldModel;
    let mut fb = fixture(
        &[held_b, landmark],
        yaw(0.5, 30.0_f32.to_radians().cos()),
        projection(),
    );
    fb.annotations.draws[0].policy = TransformPolicy::HoldModel;
    let ta = task(&mut r, fa, 1, 1, true);
    let ca = canonical(&mut r);
    let a = source(&mut r, ta);
    let tb = task(&mut r, fb, 2, 1, true);
    let cb = canonical(&mut r);
    let b = source(&mut r, tb);
    assert!(a.ineligibility().is_empty());
    assert!(b.ineligibility().is_empty());
    for t in [0.25, 0.5, 0.75] {
        let at = &a.data.task.scene;
        let bt = &b.data.task.scene;
        let scene = phase_scene(at, bt, &correspondence(at, bt).unwrap(), t);
        for (triangle, center) in [[12.0, 5.0, 8.0], [-12.0, -5.0, -4.0]]
            .into_iter()
            .enumerate()
        {
            for (corner, local) in [[-3.0, -3.0], [3.0, -3.0], [0.0, 3.0]]
                .into_iter()
                .enumerate()
            {
                let vertex = scene.indices[triangle * 3 + corner] as usize;
                let clip = crate::hle::math::mul_row_vec4(
                    [local[0], local[1], 0.0, 1.0],
                    scene.mvp_table[scene.mtx_index[vertex] as usize],
                );
                let expected = [
                    32.0 + (center[0] + local[0]) * 30.0_f32.to_radians().cos()
                        + center[2] * (t - 0.5),
                    24.0 - center[1] - local[1],
                ];
                assert!((32.0 + clip[0] * 32.0 - expected[0]).abs() <= 1.0);
                assert!((24.0 - clip[1] * 24.0 - expected[1]).abs() <= 1.0);
            }
        }
        let image = phase(&mut r, &a, &b, t, INTERPOLATED);
        assert_eq!(
            pixel(&image, 64, (42.3923 + 8.0 * (t - 0.5)).floor() as u32, 19),
            [255; 4]
        );
        assert_eq!(
            pixel(&image, 64, (21.6077 - 4.0 * (t - 0.5)).floor() as u32, 29),
            [255; 4]
        );
    }
    assert_eq!(phase(&mut r, &a, &b, 0.0, PhaseResult::Endpoint), ca);
    assert_eq!(phase(&mut r, &a, &b, 1.0, PhaseResult::Endpoint), cb);
}

#[test]
fn source_reasons_and_held_primitive_counts_are_committed() {
    let mut r = renderer(1);
    let ta = task(
        &mut r,
        fixture(&[identity()], identity(), projection()),
        1,
        1,
        true,
    );
    let a = source(&mut r, ta);
    for (case, reason) in [
        Ineligibility::UnresolvedScope,
        Ineligibility::UnresolvedBinding,
        Ineligibility::UnknownWorldDraw,
        Ineligibility::UnsupportedTopology,
    ]
    .into_iter()
    .enumerate()
    {
        let mut f = fixture(&[identity(), identity()], identity(), projection());
        match case {
            0 => f.annotations.draws[1].scope.address = 0xdead,
            1 => f.annotations.draws[1].matrices[0].operand = 0xdead,
            2 => {
                f.annotations.draws.pop();
            }
            3 => f.annotations.draws[1].identity = f.annotations.draws[0].identity,
            _ => unreachable!(),
        }
        let tb = task(&mut r, f, 2 + case as u64, 1, true);
        let b = source(&mut r, tb);
        assert!(
            b.ineligibility()
                .iter()
                .any(|entry| entry.reason == reason && entry.count == 1),
            "{:?}",
            b.ineligibility()
        );
        assert_eq!(b.counts().triangles, 2);
        assert_eq!(b.counts().fills, 2);
        assert_eq!(b.counts().rectangles, 0);
        assert_eq!(
            phase(&mut r, &a, &b, 0.5, PhaseResult::Held),
            phase(&mut r, &a, &b, 0.0, PhaseResult::Held)
        );
    }
}

fn billboard_fixture(
    degrees: f32,
    late_view: f32,
    anchor: [f32; 3],
    scale: f32,
    spin: f32,
) -> Fixture {
    let (s, c) = degrees.to_radians().sin_cos();
    let (vs, vc) = (-degrees + late_view).to_radians().sin_cos();
    let (zs, zc) = spin.to_radians().sin_cos();
    let model = [
        [zc * c * scale, zs * scale, -zc * s * scale, 0.0],
        [-zs * c * scale, zc * scale, zs * s * scale, 0.0],
        [s * scale, 0.0, c * scale, 0.0],
        [anchor[0], anchor[1], anchor[2], 1.0],
    ];
    let view = [
        [vc, 0.0, -vs, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [vs, 0.0, vc, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ];
    let mut f = fixture(&[model], view, projection());
    f.annotations.draws[0].policy = TransformPolicy::BillboardHold(BillboardRecipe {
        anchor,
        scale: [scale; 3],
        spin: spin.to_radians(),
        basis: [[c, 0.0, -s], [0.0, 1.0, 0.0], [s, 0.0, c]],
        provenance: BillboardProvenance::Cpu,
    });
    f
}

#[test]
fn billboard_orbit_and_late_view_rewrite_keep_exact_endpoints() {
    let mut r = renderer(1);
    for late in [0.0, 7.0] {
        let ta = task(
            &mut r,
            billboard_fixture(20.0, late, [8.0, 4.0, 0.0], 1.0, 170.0),
            1,
            1,
            true,
        );
        let ca = canonical(&mut r);
        let a = source(&mut r, ta);
        let tb = task(
            &mut r,
            billboard_fixture(-20.0, late, [16.0, 8.0, 0.0], 2.0, -170.0),
            2,
            1,
            true,
        );
        let cb = canonical(&mut r);
        let b = source(&mut r, tb);
        for source in [&a, &b] {
            assert!(
                source.ineligibility().is_empty(),
                "{:?}",
                source.ineligibility()
            );
            let records = source.data.task.scene.annotations.as_ref().unwrap();
            let TransformPolicy::BillboardHold(recipe) = records.sidecar.draws[0].policy else {
                unreachable!()
            };
            assert_eq!(
                math::billboard_model(recipe)
                    .unwrap()
                    .map(|row| row.map(f32::to_bits)),
                records.draws[0].vertices[0]
                    .transform
                    .unwrap()
                    .model
                    .map(|row| row.map(f32::to_bits))
            );
        }
        assert_eq!(phase(&mut r, &a, &b, 0.0, PhaseResult::Endpoint), ca);
        assert_eq!(phase(&mut r, &a, &b, 1.0, PhaseResult::Endpoint), cb);
        for t in [0.25, 0.5, 0.75] {
            let at = &a.data.task.scene;
            let bt = &b.data.task.scene;
            let scene = phase_scene(at, bt, &correspondence(at, bt).unwrap(), t);
            let (s, c) = (20.0 - 40.0 * t).to_radians().sin_cos();
            let (zs, zc) = (170.0 + 20.0 * t).to_radians().sin_cos();
            let (sa, ca) = (-20.0 + late).to_radians().sin_cos();
            let (sb, cb) = (20.0 + late).to_radians().sin_cos();
            let (vs, vc) = (sa + (sb - sa) * t, ca + (cb - ca) * t);
            for (corner, [x, y]) in [[-3.0, -3.0], [3.0, -3.0], [0.0, 3.0]]
                .into_iter()
                .enumerate()
            {
                let scale = 1.0 + t;
                let world = [
                    (x * zc - y * zs) * c * scale + 8.0 + 8.0 * t,
                    (x * zs + y * zc) * scale + 4.0 + 4.0 * t,
                    -(x * zc - y * zs) * s * scale,
                ];
                let want = [32.0 + world[0] * vc + world[2] * vs, 24.0 - world[1]];
                let vertex = scene.indices[corner] as usize;
                let clip = crate::hle::math::mul_row_vec4(
                    [x, y, 0.0, 1.0],
                    scene.mvp_table[scene.mtx_index[vertex] as usize],
                );
                assert!((32.0 + clip[0] * 32.0 - want[0]).abs() <= 1.0);
                assert!((24.0 - clip[1] * 24.0 - want[1]).abs() <= 1.0);
            }
            let image = phase(&mut r, &a, &b, t, INTERPOLATED);
            assert_eq!(
                pixel(
                    &image,
                    64,
                    (32.0 + (8.0 + 8.0 * t) * vc).floor() as u32,
                    (20.0 - 4.0 * t).floor() as u32
                ),
                [255; 4]
            );
        }
    }
}

#[test]
fn billboard_recipe_that_does_not_reconstruct_the_consumed_model_holds() {
    let mut r = renderer(1);
    let ta = task(
        &mut r,
        billboard_fixture(0.0, 0.0, [8.0, 4.0, 0.0], 1.0, 0.0),
        1,
        1,
        true,
    );
    let a = source(&mut r, ta);
    let mut f = billboard_fixture(0.0, 0.0, [12.0, 4.0, 0.0], 1.0, 0.0);
    let TransformPolicy::BillboardHold(ref mut recipe) = f.annotations.draws[0].policy else {
        unreachable!()
    };
    recipe.anchor[0] += 1.0;
    let tb = task(&mut r, f, 2, 1, true);
    let b = source(&mut r, tb);
    assert_eq!(
        b.ineligibility(),
        [ReasonCount {
            reason: Ineligibility::UnsafeBillboard,
            count: 1
        }]
    );
    assert_eq!(
        phase(&mut r, &a, &b, 0.5, PhaseResult::Held),
        phase(&mut r, &a, &b, 0.0, PhaseResult::Held)
    );
}

#[test]
fn one_unmatched_world_draw_holds_its_model_while_another_interpolates() {
    let mut r = renderer(1);
    let ta = task(
        &mut r,
        fixture(
            &[transform(0.0, -12.0, 0.0), transform(0.0, 12.0, 0.0)],
            identity(),
            projection(),
        ),
        1,
        1,
        true,
    );
    let a = source(&mut r, ta);
    let mut f = fixture(
        &[transform(0.0, -4.0, 0.0), transform(0.0, 4.0, 0.0)],
        transform(0.0, 5.0, 0.0),
        projection(),
    );
    f.annotations.draws[1].identity.generation += 1;
    let tb = task(&mut r, f, 2, 1, true);
    let b = source(&mut r, tb);
    let identity = a
        .data
        .task
        .scene
        .annotations
        .as_ref()
        .unwrap()
        .sidecar
        .draws[1]
        .identity;
    let image = phase(
        &mut r,
        &a,
        &b,
        0.5,
        restricted(RestrictionReason::NoMatch, &[identity]),
    );
    assert_eq!(pixel(&image, 64, 26, 24), [255; 4]);
    assert_eq!(pixel(&image, 64, 46, 24), [255; 4]);
    assert_eq!(pixel(&image, 64, 42, 24), [0, 0, 0, 255]);
}

#[test]
fn rectangles_fills_and_screen_space_triangles_are_counted_when_eligible() {
    let mut r = renderer(1);
    let mut f = fixture(&[identity()], identity(), projection());
    f.annotations.draws[0].policy = TransformPolicy::ScreenSpace;
    append_commands(
        &mut f,
        &gsp_texture_rectangle(80, 80, 96, 96, 0, 0, 0, 1024, 1024, false),
    );
    let ta = task(&mut r, f, 1, 1, true);
    let a = source(&mut r, ta);
    assert!(a.ineligibility().is_empty(), "{:?}", a.ineligibility());
    assert_eq!(
        a.counts(),
        SourceCounts {
            triangles: 1,
            held_triangles: 1,
            rectangles: 1,
            fills: 2
        }
    );
}

#[test]
fn world_screen_coordinate_overrides_hold_as_unsupported_topology() {
    let mut r = renderer(1);
    let ta = task(
        &mut r,
        fixture(&[identity()], identity(), projection()),
        1,
        1,
        true,
    );
    let a = source(&mut r, ta);
    for where_ in [0x18, 0x1c] {
        let mut f = fixture(&[identity()], transform(0.0, 7.0, 0.0), projection());
        let call = f.annotations.draws[0].scope.address as usize;
        let leaf =
            u32::from_be_bytes(f.built.rdram[call + 4..call + 8].try_into().unwrap()) as usize;
        let tail = f.built.rdram.len() as u32;
        for (w0, w1) in [gsp_1triangle(0, 1, 2), gsp_enddl()] {
            f.built.rdram.extend(w0.to_be_bytes());
            f.built.rdram.extend(w1.to_be_bytes());
        }
        for (offset, (w0, w1)) in [
            (16, gsp_modifyvertex(0, where_, 0)),
            (24, gsp_branchlist(tail)),
        ] {
            f.built.rdram[leaf + offset..leaf + offset + 4].copy_from_slice(&w0.to_be_bytes());
            f.built.rdram[leaf + offset + 4..leaf + offset + 8].copy_from_slice(&w1.to_be_bytes());
        }
        let tb = task(&mut r, f, 2, 1, true);
        let b = source(&mut r, tb);
        assert_eq!(
            b.ineligibility(),
            [ReasonCount {
                reason: Ineligibility::UnsupportedTopology,
                count: 1
            }]
        );
        phase(&mut r, &a, &b, 0.5, PhaseResult::Held);
    }
}

fn double_triangle(f: &mut Fixture, scope: usize) {
    let call = f.annotations.draws[scope].scope.address as usize;
    let leaf = u32::from_be_bytes(f.built.rdram[call + 4..call + 8].try_into().unwrap()) as usize;
    let (w0, w1) = gsp_2triangles(0, 1, 2, 0, 2, 1);
    f.built.rdram[leaf + 16..leaf + 20].copy_from_slice(&w0.to_be_bytes());
    f.built.rdram[leaf + 20..leaf + 24].copy_from_slice(&w1.to_be_bytes());
}

#[test]
fn changing_room_and_held_counts_keep_a_geometry_under_the_phase_camera() {
    for policy in [TransformPolicy::StaticWorld, TransformPolicy::HoldModel] {
        let mut r = renderer(1);
        let model = if policy == TransformPolicy::StaticWorld {
            identity()
        } else {
            transform(0.0, 8.0, 0.0)
        };
        let mut fa = fixture(&[model], identity(), projection());
        fa.annotations.draws[0].policy = policy;
        double_triangle(&mut fa, 0);
        let ta = task(&mut r, fa, 1, 1, true);
        let ca = canonical(&mut r);
        let a = source(&mut r, ta);
        let mut fb = fixture(&[model], transform(0.0, 12.0, 0.0), projection());
        fb.annotations.draws[0].policy = policy;
        let tb = task(&mut r, fb, 2, 1, true);
        let cb = canonical(&mut r);
        let b = source(&mut r, tb);
        for t in [0.25, 0.5, 0.75] {
            let image = phase(&mut r, &a, &b, t, INTERPOLATED);
            let x =
                32 + if policy == TransformPolicy::StaticWorld {
                    0
                } else {
                    8
                } + (12.0 * t) as u32;
            assert_eq!(pixel(&image, 64, x, 24), [255; 4]);
            assert_ne!(image, ca);
        }
        assert_eq!(phase(&mut r, &a, &b, 0.0, PhaseResult::Endpoint), ca);
        assert_eq!(phase(&mut r, &a, &b, 1.0, PhaseResult::Endpoint), cb);
    }
}

#[test]
fn despawned_held_scope_follows_camera_until_b() {
    for policy in [
        TransformPolicy::StaticWorld,
        TransformPolicy::HoldModel,
        TransformPolicy::ScreenSpace,
    ] {
        let mut r = renderer(1);
        let mut fa = fixture(
            &[identity(), transform(0.0, 14.0, 0.0)],
            identity(),
            projection(),
        );
        fa.annotations.draws[0].policy = policy;
        let ta = task(&mut r, fa, 1, 1, true);
        let a = source(&mut r, ta);
        let mut fb = fixture(
            &[transform(0.0, 14.0, 0.0)],
            transform(0.0, 12.0, 0.0),
            projection(),
        );
        fb.annotations.draws[0].identity.part = 1;
        let tb = task(&mut r, fb, 2, 1, true);
        let cb = canonical(&mut r);
        let b = source(&mut r, tb);
        let image = phase(&mut r, &a, &b, 0.5, INTERPOLATED);
        let x = if matches!(policy, TransformPolicy::ScreenSpace | TransformPolicy::Hold) {
            32
        } else {
            38
        };
        assert_eq!(pixel(&image, 64, x, 24), [255; 4]);
        assert_eq!(phase(&mut r, &a, &b, 1.0, PhaseResult::Endpoint), cb);
    }
}

fn restricted(reason: RestrictionReason, identities: &[DrawIdentity]) -> PhaseResult {
    PhaseResult::Interpolated {
        restrictions: vec![DrawRestriction {
            reason,
            count: identities.len() as u64,
            identities: identities.to_vec(),
        }],
    }
}

#[test]
fn interpolate_restrictions_keep_only_the_affected_scope_in_a_world_pose() {
    for (case, reason) in [
        RestrictionReason::TopologyChanged,
        RestrictionReason::TopologyChanged,
        RestrictionReason::TopologyChanged,
        RestrictionReason::PolicyChanged,
        RestrictionReason::DecompositionRejected,
    ]
    .into_iter()
    .enumerate()
    {
        let mut r = renderer(1);
        let mut fa = fixture(
            &[transform(0.0, -12.0, 0.0), transform(0.0, 12.0, 0.0)],
            identity(),
            projection(),
        );
        let mut fb = fixture(
            &[transform(0.0, -4.0, 0.0), transform(0.0, 4.0, 0.0)],
            transform(0.0, 4.0, 0.0),
            projection(),
        );
        match case {
            0 => double_triangle(&mut fb, 0),
            1 => {
                let call = fb.annotations.draws[0].scope.address as usize;
                let leaf =
                    u32::from_be_bytes(fb.built.rdram[call + 4..call + 8].try_into().unwrap())
                        as usize;
                let vertices =
                    u32::from_be_bytes(fb.built.rdram[leaf + 12..leaf + 16].try_into().unwrap())
                        as usize;
                let mut changed = fb.built.rdram[vertices..vertices + 48].to_vec();
                changed[..2].copy_from_slice(&(-4_i16).to_be_bytes());
                let address = fb.built.rdram.len() as u32;
                fb.built.rdram.extend_from_slice(&changed);
                fb.built.rdram[leaf + 12..leaf + 16].copy_from_slice(&address.to_be_bytes());
            }
            2 => {
                for (f, part) in [(&mut fa, 17), (&mut fb, 18)] {
                    let call = f.annotations.draws[0].scope.address as usize;
                    let leaf =
                        u32::from_be_bytes(f.built.rdram[call + 4..call + 8].try_into().unwrap())
                            as usize;
                    let model =
                        u32::from_be_bytes(f.built.rdram[leaf + 4..leaf + 8].try_into().unwrap());
                    f.annotations.draws[0].palette = Some(MatrixPalette {
                        base: u64::from(model),
                        parts: vec![part],
                    });
                }
            }
            3 => fb.annotations.draws[0].policy = TransformPolicy::ScreenSpace,
            4 => {
                let call = fa.annotations.draws[0].scope.address as usize;
                let leaf =
                    u32::from_be_bytes(fa.built.rdram[call + 4..call + 8].try_into().unwrap())
                        as usize;
                let model =
                    u32::from_be_bytes(fa.built.rdram[leaf + 4..leaf + 8].try_into().unwrap())
                        as usize;
                let mut shear = transform(0.0, -12.0, 0.0);
                shear[0][1] = 0.5;
                fa.built.rdram[model..model + 64].copy_from_slice(&mtx_to_bytes(shear));
            }
            _ => unreachable!(),
        }
        let identity = fa.annotations.draws[0].identity;
        let ta = task(&mut r, fa, 1, 1, true);
        let ca = canonical(&mut r);
        let a = source(&mut r, ta);
        let tb = task(&mut r, fb, 2, 1, true);
        let cb = canonical(&mut r);
        let b = source(&mut r, tb);
        assert!(a.ineligibility().is_empty(), "{:?}", a.ineligibility());
        assert!(b.ineligibility().is_empty());
        for t in [0.25, 0.5, 0.75] {
            let image = phase(&mut r, &a, &b, t, restricted(reason, &[identity]));
            assert_eq!(pixel(&image, 64, 20 + (4.0 * t) as u32, 24), [255; 4]);
            assert_eq!(pixel(&image, 64, 44 - (4.0 * t) as u32, 24), [255; 4]);
        }
        assert_eq!(phase(&mut r, &a, &b, 0.0, PhaseResult::Endpoint), ca);
        assert_eq!(phase(&mut r, &a, &b, 1.0, PhaseResult::Endpoint), cb);
    }
}

#[test]
fn restrictions_count_scopes_and_bound_identity_samples_in_submission_order() {
    let mut r = renderer(1);
    let models = [identity(); 7];
    let mut fa = fixture(&models, identity(), projection());
    double_triangle(&mut fa, 0);
    let ids: Vec<_> = fa.annotations.draws.iter().map(|d| d.identity).collect();
    fa.annotations.draws.reverse();
    let ta = task(&mut r, fa, 1, 1, true);
    let a = source(&mut r, ta);
    let mut fb = fixture(&models, transform(0.0, 8.0, 0.0), projection());
    for draw in &mut fb.annotations.draws[..5] {
        draw.identity.generation += 1;
    }
    double_triangle(&mut fb, 5);
    let tb = task(&mut r, fb, 2, 1, true);
    let b = source(&mut r, tb);
    let expected = PhaseResult::Interpolated {
        restrictions: vec![
            DrawRestriction {
                reason: RestrictionReason::NoMatch,
                count: 5,
                identities: ids[..4].to_vec(),
            },
            DrawRestriction {
                reason: RestrictionReason::TopologyChanged,
                count: 1,
                identities: vec![ids[5]],
            },
        ],
    };
    for t in [0.75, 0.25, 0.75] {
        let image = phase(&mut r, &a, &b, t, expected.clone());
        assert_eq!(pixel(&image, 64, 32 + (8.0 * t) as u32, 24), [255; 4]);
    }
}

#[test]
fn discrete_scopes_need_no_correspondence_or_world_camera() {
    let mut r = renderer(1);
    let mut fa = fixture(
        &[identity(), transform(0.0, 14.0, 0.0)],
        identity(),
        projection(),
    );
    for draw in &mut fa.annotations.draws {
        draw.policy = TransformPolicy::Hold;
        draw.matrices.clear();
    }
    let ta = task(&mut r, fa, 1, 1, true);
    let ca = canonical(&mut r);
    let a = source(&mut r, ta);
    let mut fb = fixture(&[transform(0.0, 20.0, 0.0)], identity(), projection());
    fb.annotations.draws[0].policy = TransformPolicy::Hold;
    fb.annotations.draws[0].matrices.clear();
    fb.annotations.draws[0].identity.generation += 1;
    let tb = task(&mut r, fb, 2, 1, true);
    let cb = canonical(&mut r);
    let b = source(&mut r, tb);
    assert_eq!(phase(&mut r, &a, &b, 0.5, INTERPOLATED), ca);
    assert_eq!(phase(&mut r, &a, &b, 1.0, PhaseResult::Endpoint), cb);
}

#[test]
fn distinct_consumed_views_keep_scope_camera_pairing() {
    let mut r = renderer(1);
    let ta = task(
        &mut r,
        fixture(
            &[identity(), transform(0.0, 12.0, 0.0)],
            identity(),
            projection(),
        ),
        1,
        1,
        true,
    );
    let tb = task(
        &mut r,
        fixture(
            &[identity(), transform(0.0, 12.0, 0.0)],
            transform(0.0, 12.0, 0.0),
            projection(),
        ),
        2,
        1,
        true,
    );
    let mut a = ta.retained.as_ref().unwrap().scene.clone();
    let mut b = tb.retained.as_ref().unwrap().scene.clone();
    for (scene, x) in [(&mut a, -4.0), (&mut b, 4.0)] {
        for vertex in &mut scene.annotations.as_mut().unwrap().draws[1].vertices {
            vertex.transform.as_mut().unwrap().view = transform(0.0, x, 0.0);
        }
    }
    let matched = correspondence(&a, &b).expect("resolved views pair within their own scope");
    let phase = phase_scene(&a, &b, &matched, 0.5);
    for (triangle, x) in [(0, 6.0 / 32.0), (1, 12.0 / 32.0)] {
        let vertex = phase.indices[triangle * 3] as usize;
        assert_eq!(phase.mvp_table[phase.mtx_index[vertex] as usize][3][0], x);
    }
}

#[test]
fn billboard_fallback_keeps_a_recipe_under_the_phase_camera_per_scope() {
    for (case, reason) in [
        RestrictionReason::NoMatch,
        RestrictionReason::NoMatch,
        RestrictionReason::PolicyChanged,
        RestrictionReason::DecompositionRejected,
    ]
    .into_iter()
    .enumerate()
    {
        let mut r = renderer(1);
        let recipe = BillboardRecipe {
            anchor: [-12.0, 0.0, 0.0],
            scale: [1.0; 3],
            spin: 0.0,
            basis: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            provenance: BillboardProvenance::Cpu,
        };
        let mut fa = fixture(
            &[transform(0.0, -12.0, 0.0), transform(0.0, 12.0, 0.0)],
            identity(),
            projection(),
        );
        fa.annotations.draws[0].policy = TransformPolicy::BillboardHold(recipe);
        let id = fa.annotations.draws[0].identity;
        let ta = task(&mut r, fa, 1, 1, true);
        let ca = canonical(&mut r);
        let a = source(&mut r, ta);
        let mut fb = fixture(
            &[transform(0.0, -4.0, 0.0), transform(0.0, 4.0, 0.0)],
            transform(0.0, 4.0, 0.0),
            projection(),
        );
        fb.annotations.draws[0].policy = TransformPolicy::BillboardHold(BillboardRecipe {
            anchor: [-4.0, 0.0, 0.0],
            provenance: if case == 3 {
                BillboardProvenance::Segment1
            } else {
                recipe.provenance
            },
            ..recipe
        });
        match case {
            0 => fb.annotations.draws[0].identity.instance += 1,
            1 => fb.annotations.draws[0].identity.generation += 1,
            2 => fb.annotations.draws[0].policy = TransformPolicy::HoldModel,
            3 => (),
            _ => unreachable!(),
        }
        let tb = task(&mut r, fb, 2, 1, true);
        let cb = canonical(&mut r);
        let b = source(&mut r, tb);
        assert!(a.ineligibility().is_empty());
        assert!(b.ineligibility().is_empty());
        for t in [0.25, 0.5, 0.75] {
            let image = phase(&mut r, &a, &b, t, restricted(reason, &[id]));
            assert_eq!(pixel(&image, 64, 20 + (4.0 * t) as u32, 24), [255; 4]);
            assert_eq!(pixel(&image, 64, 44 - (4.0 * t) as u32, 24), [255; 4]);
            assert_ne!(image, ca);
        }
        assert_eq!(phase(&mut r, &a, &b, 0.0, PhaseResult::Endpoint), ca);
        assert_eq!(phase(&mut r, &a, &b, 1.0, PhaseResult::Endpoint), cb);
    }
}

#[test]
fn held_world_generation_changes_do_not_hold_the_pair() {
    for policy in [
        TransformPolicy::StaticWorld,
        TransformPolicy::HoldModel,
        TransformPolicy::ScreenSpace,
    ] {
        let mut r = renderer(1);
        let mut fa = fixture(
            &[identity(), transform(0.0, 12.0, 0.0)],
            identity(),
            projection(),
        );
        fa.annotations.draws[0].policy = policy;
        let ta = task(&mut r, fa, 1, 1, true);
        let a = source(&mut r, ta);
        let mut fb = fixture(
            &[identity(), transform(0.0, 4.0, 0.0)],
            transform(0.0, 4.0, 0.0),
            projection(),
        );
        fb.annotations.draws[0].policy = policy;
        fb.annotations.draws[0].identity.generation += 1;
        let tb = task(&mut r, fb, 2, 1, true);
        let b = source(&mut r, tb);
        let image = phase(&mut r, &a, &b, 0.5, INTERPOLATED);
        let x = if policy == TransformPolicy::ScreenSpace {
            32
        } else {
            34
        };
        assert_eq!(pixel(&image, 64, x, 24), [255; 4]);
        assert_eq!(pixel(&image, 64, 42, 24), [255; 4]);
    }
}

#[test]
fn billboard_recipe_matches_an_executed_scope_without_b_triangles() {
    let mut r = renderer(1);
    let recipe = BillboardRecipe {
        anchor: [0.0; 3],
        scale: [1.0; 3],
        spin: 0.0,
        basis: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
        provenance: BillboardProvenance::Cpu,
    };
    let mut fa = fixture(
        &[identity(), transform(0.0, 12.0, 0.0)],
        identity(),
        projection(),
    );
    fa.annotations.draws[0].policy = TransformPolicy::BillboardHold(recipe);
    let ta = task(&mut r, fa, 1, 1, true);
    let a = source(&mut r, ta);
    let mut fb = fixture(
        &[transform(0.0, 8.0, 0.0), transform(0.0, 12.0, 0.0)],
        transform(0.0, 4.0, 0.0),
        projection(),
    );
    fb.annotations.draws[0].policy = TransformPolicy::BillboardHold(BillboardRecipe {
        anchor: [8.0, 0.0, 0.0],
        ..recipe
    });
    let call = fb.annotations.draws[0].scope.address as usize;
    let leaf = u32::from_be_bytes(fb.built.rdram[call + 4..call + 8].try_into().unwrap()) as usize;
    let (w0, w1) = gsp_enddl();
    fb.built.rdram[leaf + 16..leaf + 20].copy_from_slice(&w0.to_be_bytes());
    fb.built.rdram[leaf + 20..leaf + 24].copy_from_slice(&w1.to_be_bytes());
    let tb = task(&mut r, fb, 2, 1, true);
    let cb = canonical(&mut r);
    let b = source(&mut r, tb);
    assert!(b.ineligibility().is_empty());
    let image = phase(&mut r, &a, &b, 0.5, INTERPOLATED);
    assert_eq!(pixel(&image, 64, 38, 24), [255; 4]);
    assert_eq!(phase(&mut r, &a, &b, 1.0, PhaseResult::Endpoint), cb);
}
