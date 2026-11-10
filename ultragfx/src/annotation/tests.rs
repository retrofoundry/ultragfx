use super::*;
use crate::{
    hle::{
        gbi::GbiUcode,
        interp,
        mem::{GbiDataFormat, RdramImage},
    },
    tests::dl_builder::DlBuilder,
};
use n64_gbi::encode::*;

fn binding(operand: u64, role: MatrixRole) -> MatrixBinding {
    MatrixBinding { operand, role }
}
fn scope(address: u64, instance: u64) -> DrawAnnotation {
    DrawAnnotation {
        scope: ScopeSelector::Invocation(InvocationSelector {
            address,
            occurrence: None,
        }),
        identity: DrawIdentity {
            instance,
            ..Default::default()
        },
        policy: TransformPolicy::ScreenSpace,
        ..Default::default()
    }
}
fn interpret(
    built: &crate::tests::dl_builder::Built,
    annotations: Option<TaskAnnotations>,
) -> crate::hle::interp::InterpResult {
    interp::interpret_borrowed(
        &mut RdramImage::new(&built.rdram),
        u64::from(built.entry),
        GbiUcode::F3dex2,
        GbiDataFormat::Fixed,
        Default::default(),
        Default::default(),
        None,
        Default::default(),
        &[],
        true,
        annotations,
    )
}
fn shade() -> (u32, u32) {
    let color = CcPass {
        a: 8,
        b: 8,
        c: 31,
        d: 4,
    };
    let alpha = CcPass {
        a: 7,
        b: 7,
        c: 7,
        d: 4,
    };
    gdp_set_combine_lerp(color, alpha, color, alpha)
}
fn lists() -> (DlBuilder, u32) {
    let mut dl = DlBuilder::new();
    let vertices = dl.bytes(16, &[0; 48]);
    let leaf = dl.list(
        "leaf",
        &[
            shade(),
            gsp_vertex(0, 3, vertices),
            gsp_1triangle(0, 1, 2),
            gsp_enddl(),
        ],
    );
    (dl, leaf)
}
#[test]
fn shared_leaf_has_three_invocation_identities_and_no_canonical_changes() {
    let (mut dl, leaf) = lists();
    let entry = dl.list(
        "main",
        &[
            gsp_displaylist(leaf),
            gsp_displaylist(leaf),
            gsp_displaylist(leaf),
            gsp_enddl(),
        ],
    );
    let built = dl.finish("main");
    let annotations = TaskAnnotations {
        draws: (0..3)
            .map(|i| scope(u64::from(entry) + i * 8, i + 1))
            .collect(),
        ..Default::default()
    };
    let canonical = interpret(&built, None);
    let mut annotated = interpret(&built, Some(annotations));
    let captured = annotated.scene.annotations.take().unwrap();
    assert_eq!(
        captured
            .draws
            .iter()
            .map(|d| d.annotation)
            .collect::<Vec<_>>(),
        [Some(0), Some(1), Some(2)]
    );
    assert!(captured.reasons.is_empty());
    assert_eq!(annotated, canonical);
}
#[test]
fn repeated_selector_requires_an_occurrence() {
    let (mut dl, leaf) = lists();
    let parent = dl.list("parent", &[gsp_displaylist(leaf), gsp_enddl()]);
    dl.list(
        "main",
        &[
            gsp_displaylist(parent),
            gsp_displaylist(parent),
            gsp_enddl(),
        ],
    );
    let built = dl.finish("main");
    let run = |occurrence| {
        let mut annotation = scope(u64::from(parent), 1);
        annotation.scope = ScopeSelector::Invocation(InvocationSelector {
            address: u64::from(parent),
            occurrence,
        });
        interpret(
            &built,
            Some(TaskAnnotations {
                draws: vec![annotation],
                ..Default::default()
            }),
        )
        .scene
        .annotations
        .unwrap()
    };
    assert_eq!(
        run(None).reasons,
        [ReasonCount {
            reason: Ineligibility::UnresolvedScope,
            count: 1
        }]
    );
    let captured = run(Some(1));
    assert!(captured.reasons.is_empty());
    assert_eq!(captured.draws[0].annotation, None);
    assert_eq!(captured.draws[1].annotation, Some(0));
}
#[test]
fn scope_inherits_calls_and_branches_and_nested_scope_overrides_until_return() {
    let (mut dl, leaf) = lists();
    let tail = dl.list("tail", &[gsp_displaylist(leaf), gsp_enddl()]);
    let middle = dl.list("middle", &[gsp_displaylist(leaf), gsp_branchlist(tail)]);
    let parent = dl.list(
        "parent",
        &[gsp_displaylist(middle), gsp_displaylist(leaf), gsp_enddl()],
    );
    let entry = dl.list(
        "main",
        &[gsp_displaylist(parent), gsp_displaylist(leaf), gsp_enddl()],
    );
    let built = dl.finish("main");
    let captured = interpret(
        &built,
        Some(TaskAnnotations {
            draws: vec![scope(u64::from(entry), 1), scope(u64::from(middle), 2)],
            ..Default::default()
        }),
    )
    .scene
    .annotations
    .unwrap();
    assert_eq!(
        captured
            .draws
            .iter()
            .map(|d| d.annotation)
            .collect::<Vec<_>>(),
        [Some(1), Some(0), Some(0), None]
    );
    assert!(captured.reasons.is_empty());
}
#[test]
fn palette_and_repeated_cameras_resolve_actual_per_vertex_reads() {
    let mut dl = DlBuilder::new();
    let mut torso = identity();
    torso[3][0] = 4.0;
    let mut shoulder = identity();
    shoulder[3][1] = 7.0;
    let palette = dl.matrix(torso);
    assert_eq!(dl.matrix(shoulder), palette + 64);
    let v = dl.matrix(identity());
    let p = dl.matrix(identity());
    let vertices = dl.bytes(16, &[0; 48]);
    let leaf = dl.list(
        "leaf",
        &[
            shade(),
            gsp_matrix(0x0d000000, false, true, false),
            gsp_vertex(0, 1, vertices),
            gsp_matrix(0x0d000040, false, true, false),
            gsp_vertex(1, 2, vertices + 16),
            gsp_1triangle(0, 1, 2),
            gsp_enddl(),
        ],
    );
    let camera = dl.list(
        "camera",
        &[
            gsp_segment(13, palette),
            gsp_matrix(p, true, true, false),
            gsp_matrix(v, true, false, false),
            gsp_displaylist(leaf),
            gsp_enddl(),
        ],
    );
    let entry = dl.list(
        "main",
        &[
            gsp_displaylist(camera),
            gsp_displaylist(camera),
            gsp_enddl(),
        ],
    );
    let built = dl.finish("main");
    let mut annotations = TaskAnnotations {
        draws: [0, 8]
            .map(|offset| {
                let mut a = scope(u64::from(entry) + offset, offset);
                a.policy = TransformPolicy::Interpolate;
                a.matrices = vec![
                    binding(u64::from(v), MatrixRole::View),
                    binding(u64::from(p), MatrixRole::Projection),
                ];
                a.palette = Some(MatrixPalette {
                    base: 0x0d000000,
                    parts: vec![17, 10],
                });
                a
            })
            .to_vec(),
        ..Default::default()
    };
    let captured = interpret(&built, Some(annotations.clone()))
        .scene
        .annotations
        .unwrap();
    assert!(captured.reasons.is_empty(), "{:?}", captured.reasons);
    for draw in &captured.draws {
        assert_eq!(
            draw.vertices.each_ref().map(|v| v.transform.unwrap().model),
            [torso, shoulder, shoulder]
        );
        assert_eq!(
            draw.vertices.each_ref().map(|v| v.transform.unwrap().part),
            [Some(17), Some(10), Some(10)]
        );
    }
    annotations.draws[1].matrices[0].operand += 512;
    let captured = interpret(&built, Some(annotations))
        .scene
        .annotations
        .unwrap();
    assert_eq!(
        captured.reasons,
        [ReasonCount {
            reason: Ineligibility::UnresolvedBinding,
            count: 1
        }]
    );
}

#[test]
fn repeated_camera_reads_and_palette_rebase_preserve_cached_vertex_provenance() {
    use crate::{
        hle::{mem::Rdram, rsp::Rsp},
        scene::Scene,
    };
    let mut dl = DlBuilder::new();
    let mut model_a = identity();
    model_a[3][0] = 5.0;
    let mut model_b = identity();
    model_b[3][1] = 7.0;
    let ma = dl.matrix(model_a);
    let mb = dl.matrix(model_b);
    let mut va = identity();
    va[3][0] = 2.0;
    let mut vb = identity();
    vb[3][0] = -3.0;
    let mut pa = identity();
    pa[0][0] = 2.0;
    let mut pb = identity();
    pb[1][1] = 3.0;
    let view = dl.matrix(va);
    let projection = dl.matrix(pa);
    let vertices = dl.bytes(16, &[0; 48]);
    dl.list("main", &[gsp_enddl()]);
    let built = dl.finish("main");
    let mut rewritten = built.rdram.clone();
    rewritten[view as usize..view as usize + 64].copy_from_slice(&mtx_to_bytes(vb));
    rewritten[projection as usize..projection as usize + 64].copy_from_slice(&mtx_to_bytes(pb));
    let mut rsp = Rsp::default();
    rsp.set_annotations(TaskAnnotations {
        draws: [0x100, 0x108]
            .map(|pc| {
                let mut a = scope(pc, pc);
                a.policy = TransformPolicy::Interpolate;
                a.matrices = vec![
                    binding(0x03000000, MatrixRole::View),
                    binding(0x03000040, MatrixRole::Projection),
                ];
                a.palette = Some(MatrixPalette {
                    base: 0x0d000000,
                    parts: vec![17],
                });
                a
            })
            .to_vec(),
        ..Default::default()
    });
    let mut scene = Scene::default();
    rsp.annotation_call(0x100, 1);
    for (i, (bytes, model)) in [(&built.rdram, ma), (&rewritten, mb)]
        .into_iter()
        .enumerate()
    {
        let mut mem = RdramImage::new(bytes);
        mem.set_segment(3, u64::from(view));
        mem.set_segment(13, u64::from(model));
        rsp.matrix(
            &mem,
            mem.resolve_masked(0x03000040).unwrap(),
            crate::hle::consts::G_MTX_LOAD | crate::hle::consts::G_MTX_PROJECTION,
        )
        .unwrap();
        rsp.matrix(
            &mem,
            mem.resolve_masked(0x03000000).unwrap(),
            crate::hle::consts::G_MTX_PROJECTION,
        )
        .unwrap();
        rsp.matrix(
            &mem,
            mem.resolve_masked(0x0d000000).unwrap(),
            crate::hle::consts::G_MTX_LOAD,
        )
        .unwrap();
        rsp.set_vertex(
            &mem,
            u64::from(vertices),
            3,
            i as u32 * 3,
            &Default::default(),
            &mut scene,
        )
        .unwrap();
    }
    for i in 0..2 {
        if i == 1 {
            rsp.annotation_return(0);
            rsp.annotation_call(0x108, 1);
        }
        rsp.draw_tri(
            i * 3,
            i * 3 + 1,
            i * 3 + 2,
            0,
            0,
            [0; 4],
            Default::default(),
            &mut scene,
            None,
        );
    }
    rsp.finish(&mut scene);
    let records = scene.annotations.unwrap();
    assert!(records.reasons.is_empty(), "{:?}", records.reasons);
    for (draw, (model, view, projection)) in records
        .draws
        .iter()
        .zip([(model_a, va, pa), (model_b, vb, pb)])
    {
        for vertex in &draw.vertices {
            assert_eq!(
                vertex.transform.unwrap(),
                ResolvedTransform {
                    model,
                    view,
                    projection,
                    part: Some(17)
                }
            );
        }
    }
}

#[test]
fn forced_mvp_that_cannot_be_reconstructed_is_an_unresolved_binding() {
    use crate::{hle::rsp::Rsp, scene::Scene};
    let mut dl = DlBuilder::new();
    let va = dl.matrix(identity());
    let pa = dl.matrix(identity());
    let mut forced = identity();
    forced[3][0] = 5.0;
    let fa = dl.matrix(forced);
    let vertices = dl.bytes(16, &[0; 48]);
    dl.list("main", &[gsp_enddl()]);
    let built = dl.finish("main");
    let memory = RdramImage::new(&built.rdram);
    let mut annotation = scope(0x100, 1);
    annotation.policy = TransformPolicy::StaticWorld;
    annotation.matrices = vec![
        binding(u64::from(va), MatrixRole::View),
        binding(u64::from(pa), MatrixRole::Projection),
    ];
    let mut rsp = Rsp::default();
    rsp.set_annotations(TaskAnnotations {
        draws: vec![annotation],
        ..Default::default()
    });
    rsp.matrix(
        &memory,
        u64::from(pa),
        crate::hle::consts::G_MTX_LOAD | crate::hle::consts::G_MTX_PROJECTION,
    )
    .unwrap();
    rsp.matrix(&memory, u64::from(va), crate::hle::consts::G_MTX_PROJECTION)
        .unwrap();
    rsp.force_matrix(&memory, u64::from(fa)).unwrap();
    let mut scene = Scene::default();
    rsp.set_vertex(
        &memory,
        u64::from(vertices),
        3,
        0,
        &Default::default(),
        &mut scene,
    )
    .unwrap();
    rsp.annotation_call(0x100, 1);
    rsp.draw_tri(0, 1, 2, 0, 0, [0; 4], Default::default(), &mut scene, None);
    rsp.finish(&mut scene);
    assert_eq!(
        scene.annotations.unwrap().reasons,
        [ReasonCount {
            reason: Ineligibility::UnresolvedBinding,
            count: 1
        }]
    );
}

#[test]
fn duplicate_camera_roles_are_ambiguous_and_branch_scopes_work() {
    let (mut dl, leaf) = lists();
    let v = dl.matrix(identity());
    let p = dl.matrix(identity());
    let camera = dl.list(
        "camera",
        &[
            gsp_matrix(p, true, true, false),
            gsp_matrix(v, true, false, false),
            gsp_branchlist(leaf),
        ],
    );
    let entry = dl.list("main", &[gsp_branchlist(camera)]);
    let built = dl.finish("main");
    let mut a = scope(u64::from(entry), 1);
    a.policy = TransformPolicy::StaticWorld;
    a.matrices = vec![
        binding(u64::from(v), MatrixRole::View),
        binding(u64::from(v), MatrixRole::View),
        binding(u64::from(p), MatrixRole::Projection),
    ];
    let captured = interpret(
        &built,
        Some(TaskAnnotations {
            draws: vec![a],
            ..Default::default()
        }),
    )
    .scene
    .annotations
    .unwrap();
    assert_eq!(captured.draws[0].annotation, Some(0));
    assert_eq!(
        captured.reasons,
        [ReasonCount {
            reason: Ineligibility::UnresolvedBinding,
            count: 1
        }]
    );
}

#[test]
fn opaque_view_projection_binds_the_actual_read() {
    let (mut dl, leaf) = lists();
    let mut vp = identity();
    vp[3][0] = 3.0;
    let pa = dl.matrix(vp);
    let camera = dl.list(
        "camera",
        &[gsp_matrix(pa, true, true, false), gsp_branchlist(leaf)],
    );
    let entry = dl.list("main", &[gsp_branchlist(camera)]);
    let built = dl.finish("main");
    let mut a = scope(u64::from(entry), 1);
    a.policy = TransformPolicy::StaticWorld;
    a.matrices = vec![binding(u64::from(pa), MatrixRole::ViewProjection)];
    let captured = interpret(
        &built,
        Some(TaskAnnotations {
            draws: vec![a],
            ..Default::default()
        }),
    )
    .scene
    .annotations
    .unwrap();
    assert!(captured.reasons.is_empty(), "{:?}", captured.reasons);
    assert_eq!(
        captured.draws[0].vertices[0].transform.unwrap().projection,
        vp
    );
}

#[test]
fn asset_segment_one_billboard_recipe_matches_effective_rsp_multiply() {
    let (mut dl, leaf) = lists();
    let root = [
        [0.0, 3.0, 0.0, 0.0],
        [-2.0, 0.0, 0.0, 0.0],
        [0.0, 0.0, 4.0, 0.0],
        [12.0, 4.0, 0.0, 1.0],
    ];
    let basis = [[0.0, 0.0, -1.0], [0.0, 1.0, 0.0], [1.0, 0.0, 0.0]];
    let ma = dl.matrix(root);
    let ba = dl.matrix([
        [0.0, 0.0, -1.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [1.0, 0.0, 0.0, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ]);
    let va = dl.matrix(identity());
    let pa = dl.matrix(identity());
    let asset = dl.list(
        "asset",
        &[
            gsp_segment(1, ba),
            gsp_matrix(ma, false, true, false),
            gsp_matrix(0x01000000, false, false, true),
            gsp_displaylist(leaf),
            gsp_popmatrix(1),
            gsp_enddl(),
        ],
    );
    let entry = dl.list(
        "main",
        &[
            gsp_matrix(pa, true, true, false),
            gsp_matrix(va, true, false, false),
            gsp_displaylist(asset),
            gsp_enddl(),
        ],
    );
    let built = dl.finish("main");
    let mut a = scope(u64::from(entry) + 16, 1);
    a.policy = TransformPolicy::BillboardHold(BillboardRecipe {
        anchor: [12.0, 4.0, 0.0],
        scale: [2.0, 3.0, 4.0],
        spin: std::f32::consts::FRAC_PI_2,
        basis,
        provenance: BillboardProvenance::Segment1,
    });
    a.matrices = vec![
        binding(u64::from(va), MatrixRole::View),
        binding(u64::from(pa), MatrixRole::Projection),
    ];
    let captured = interpret(
        &built,
        Some(TaskAnnotations {
            draws: vec![a],
            ..Default::default()
        }),
    )
    .scene
    .annotations
    .unwrap();
    assert!(captured.reasons.is_empty(), "{:?}", captured.reasons);
    assert_eq!(
        captured.draws[0].vertices[0].transform.unwrap().model,
        [
            [0.0, 0.0, -4.0, 0.0],
            [-2.0, 0.0, 0.0, 0.0],
            [0.0, 3.0, 0.0, 0.0],
            [12.0, 4.0, 0.0, 1.0]
        ]
    );
}

fn span(start: u64, end: u64, occurrence: Option<u32>, instance: u64) -> DrawAnnotation {
    DrawAnnotation {
        scope: ScopeSelector::Commands(CommandSpan {
            start,
            end,
            occurrence,
        }),
        ..scope(0, instance)
    }
}

#[test]
fn command_span_enters_and_exits_around_one_primitive() {
    let mut dl = DlBuilder::new();
    let vertices = dl.bytes(16, &[0; 48]);
    let entry = u64::from(dl.list(
        "main",
        &[
            shade(),
            gsp_vertex(0, 3, vertices),
            gsp_1triangle(0, 1, 2),
            gsp_2triangles(0, 1, 2, 0, 2, 1),
            gsp_1triangle(0, 1, 2),
            gsp_enddl(),
        ],
    ));
    let built = dl.finish("main");
    let canonical = interpret(&built, None);
    let mut annotated = interpret(
        &built,
        Some(TaskAnnotations {
            draws: vec![span(entry + 24, entry + 32, None, 1)],
            ..Default::default()
        }),
    );
    let records = annotated.scene.annotations.take().unwrap();
    assert!(records.reasons.is_empty(), "{:?}", records.reasons);
    assert_eq!(
        records
            .draws
            .iter()
            .map(|d| d.annotation)
            .collect::<Vec<_>>(),
        [None, Some(0), Some(0), None]
    );
    assert_eq!(annotated, canonical);
}

#[test]
fn command_span_occurrences_count_entries_in_each_list_execution() {
    let (mut dl, leaf) = lists();
    dl.list(
        "main",
        &[gsp_displaylist(leaf), gsp_displaylist(leaf), gsp_enddl()],
    );
    let built = dl.finish("main");
    let run = |occurrences: &[Option<u32>]| {
        interpret(
            &built,
            Some(TaskAnnotations {
                draws: occurrences
                    .iter()
                    .enumerate()
                    .map(|(i, &occurrence)| {
                        span(
                            u64::from(leaf) + 8,
                            u64::from(leaf) + 24,
                            occurrence,
                            i as u64,
                        )
                    })
                    .collect(),
                ..Default::default()
            }),
        )
        .scene
        .annotations
        .unwrap()
    };
    let records = run(&[Some(0), Some(1)]);
    assert!(records.reasons.is_empty(), "{:?}", records.reasons);
    assert_eq!(
        records
            .draws
            .iter()
            .map(|d| d.annotation)
            .collect::<Vec<_>>(),
        [Some(0), Some(1)]
    );
    for occurrence in [None, Some(2)] {
        assert_eq!(
            run(&[occurrence]).reasons,
            [ReasonCount {
                reason: Ineligibility::UnresolvedScope,
                count: 1,
            }]
        );
    }
}

#[test]
fn overlapping_duplicate_and_unreached_command_spans_are_unresolved() {
    let (dl, leaf) = lists();
    let built = dl.finish("leaf");
    for second in [u64::from(leaf) + 8, u64::from(leaf) + 16] {
        let records = interpret(
            &built,
            Some(TaskAnnotations {
                draws: vec![
                    span(u64::from(leaf) + 8, u64::from(leaf) + 24, None, 1),
                    span(second, u64::from(leaf) + 24, None, 2),
                ],
                ..Default::default()
            }),
        )
        .scene
        .annotations
        .unwrap();
        assert_eq!(
            records.reasons,
            [ReasonCount {
                reason: Ineligibility::UnresolvedScope,
                count: 2
            }]
        );
    }
    let records = interpret(
        &built,
        Some(TaskAnnotations {
            draws: vec![span(0, 8, None, 1)],
            ..Default::default()
        }),
    )
    .scene
    .annotations
    .unwrap();
    assert_eq!(
        records.reasons,
        [ReasonCount {
            reason: Ineligibility::UnresolvedScope,
            count: 1
        }]
    );
}

#[test]
fn spans_inherit_calls_and_nest_with_invocation_scopes() {
    let (mut dl, leaf) = lists();
    let middle = dl.list(
        "middle",
        &[
            gsp_displaylist(leaf),
            gsp_displaylist(leaf),
            gsp_displaylist(leaf),
            gsp_enddl(),
        ],
    );
    let entry = u64::from(dl.list(
        "main",
        &[
            gsp_displaylist(middle),
            gsp_displaylist(leaf),
            gsp_displaylist(leaf),
            gsp_enddl(),
        ],
    ));
    let built = dl.finish("main");
    let records = interpret(
        &built,
        Some(TaskAnnotations {
            draws: vec![
                span(entry, entry + 16, None, 1),
                scope(u64::from(middle) + 8, 2),
                span(u64::from(leaf) + 16, u64::from(leaf) + 24, Some(2), 3),
            ],
            ..Default::default()
        }),
    )
    .scene
    .annotations
    .unwrap();
    assert!(records.reasons.is_empty(), "{:?}", records.reasons);
    assert_eq!(
        records
            .draws
            .iter()
            .map(|d| d.annotation)
            .collect::<Vec<_>>(),
        [Some(0), Some(1), Some(2), Some(0), None]
    );
}

#[test]
fn a_span_inside_an_invocation_restores_the_invocation_on_exit() {
    let mut dl = DlBuilder::new();
    let vertices = dl.bytes(16, &[0; 48]);
    let leaf = u64::from(dl.list(
        "leaf",
        &[
            shade(),
            gsp_vertex(0, 3, vertices),
            gsp_1triangle(0, 1, 2),
            gsp_1triangle(0, 1, 2),
            gsp_1triangle(0, 1, 2),
            gsp_enddl(),
        ],
    ));
    let entry = dl.list("main", &[gsp_displaylist(leaf as u32), gsp_enddl()]);
    let built = dl.finish("main");
    let records = interpret(
        &built,
        Some(TaskAnnotations {
            draws: vec![
                scope(u64::from(entry), 1),
                span(leaf + 24, leaf + 32, None, 2),
            ],
            ..Default::default()
        }),
    )
    .scene
    .annotations
    .unwrap();
    assert!(records.reasons.is_empty(), "{:?}", records.reasons);
    assert_eq!(
        records
            .draws
            .iter()
            .map(|d| d.annotation)
            .collect::<Vec<_>>(),
        [Some(0), Some(1), Some(0)]
    );
}

fn composed_fixture(
    view: Mat4,
    modelview: Mat4,
) -> (crate::tests::dl_builder::Built, TaskAnnotations) {
    let mut dl = DlBuilder::new();
    let mut projection = identity();
    projection[0][0] = 0.75;
    projection[2][3] = -1.0;
    let pa = dl.matrix(projection);
    let ma = dl.matrix(modelview);
    let vertices = dl.bytes(16, &[0; 48]);
    let entry = u64::from(dl.list(
        "main",
        &[
            shade(),
            gsp_matrix(pa, true, true, false),
            gsp_matrix(ma, false, true, false),
            gsp_vertex(0, 3, vertices),
            gsp_1triangle(0, 1, 2),
            gsp_enddl(),
        ],
    ));
    let mut annotation = span(entry + 32, entry + 40, None, 1);
    annotation.policy = TransformPolicy::HoldModel;
    annotation.composed_view = Some(0);
    annotation.matrices = vec![binding(u64::from(pa), MatrixRole::Projection)];
    (
        dl.finish("main"),
        TaskAnnotations {
            draws: vec![annotation],
            composed_views: vec![fixed_words(view)],
        },
    )
}

fn composed_view() -> Mat4 {
    [
        [0.8, 0.6, 0.0, 0.0],
        [-0.6, 0.8, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
        [123.125, -41.75, 7.0, 1.0],
    ]
}

#[test]
fn composed_camera_only_model_is_exact_identity_including_static_world() {
    let view = composed_view();
    let (built, mut annotations) = composed_fixture(view, view);
    for policy in [
        TransformPolicy::HoldModel,
        TransformPolicy::StaticWorld,
        TransformPolicy::Interpolate,
    ] {
        annotations.draws[0].policy = policy;
        let records = interpret(&built, Some(annotations.clone()))
            .scene
            .annotations
            .unwrap();
        assert!(records.reasons.is_empty(), "{:?}", records.reasons);
        let decoded = ComposedView::new(&fixed_words(view)).unwrap().matrix;
        for vertex in records.draws[0].vertices.iter() {
            assert_eq!(vertex.transform.unwrap().model, identity());
            assert_eq!(vertex.transform.unwrap().view, decoded);
        }
    }
}

#[test]
fn composed_model_recovers_a_fixed_point_modelview_and_checks_static_world() {
    let view = composed_view();
    let model = [
        [1.5, 0.0, 0.0, 0.0],
        [0.0, 0.75, 0.0, 0.0],
        [0.0, 0.0, 2.0, 0.0],
        [3.25, -4.5, 1.0, 1.0],
    ];
    let (built, mut annotations) = composed_fixture(view, mul4(model, view));
    let mut result = interpret(&built, Some(annotations.clone()));
    let records = result.scene.annotations.take().unwrap();
    assert!(records.reasons.is_empty(), "{:?}", records.reasons);
    for vertex in &records.draws[0].vertices {
        let resolved = vertex.transform.unwrap();
        for (recovered, expected) in resolved.model.iter().flatten().zip(model.iter().flatten()) {
            assert!((recovered - expected).abs() < 0.0001, "{resolved:?}");
        }
        assert_ne!(
            mul4(resolved.model, mul4(resolved.view, resolved.projection)),
            result.scene.mvp_table[0]
        );
    }
    annotations.draws[0].policy = TransformPolicy::StaticWorld;
    assert_eq!(
        interpret(&built, Some(annotations))
            .scene
            .annotations
            .unwrap()
            .reasons,
        [ReasonCount {
            reason: Ineligibility::UnresolvedBinding,
            count: 1
        }]
    );
}

#[test]
fn composed_view_rejects_bad_indices_singular_views_and_forbidden_bindings() {
    let (built, annotations) = composed_fixture(composed_view(), composed_view());
    for case in 0..6 {
        let mut annotations = annotations.clone();
        match case {
            0 => annotations.draws[0].composed_view = Some(1),
            1 => annotations.composed_views[0] = [0; 16],
            2 => annotations.draws[0].matrices.clear(),
            3 => annotations.draws[0].matrices[0].role = MatrixRole::ViewProjection,
            4 => annotations.draws[0]
                .matrices
                .push(binding(0xdead, MatrixRole::View)),
            5 => annotations.draws[0].matrices[0].operand = 0xdead,
            _ => unreachable!(),
        }
        assert_eq!(
            interpret(&built, Some(annotations))
                .scene
                .annotations
                .unwrap()
                .reasons,
            [ReasonCount {
                reason: Ineligibility::UnresolvedBinding,
                count: 1
            }],
            "case {case}"
        );
    }
}

#[test]
fn an_unscoped_triangle_using_a_composed_scopes_projection_still_holds() {
    let (built, mut annotations) = composed_fixture(composed_view(), composed_view());
    let ScopeSelector::Commands(span) = &mut annotations.draws[0].scope else {
        unreachable!()
    };
    span.start -= 8;
    span.end -= 8;
    let records = interpret(&built, Some(annotations))
        .scene
        .annotations
        .unwrap();
    assert_eq!(
        records.reasons,
        [ReasonCount {
            reason: Ineligibility::UnknownWorldDraw,
            count: 1
        }]
    );
}

#[test]
fn composed_view_rejects_an_uncertifiable_nearly_singular_view() {
    let q = 1.0 / 65536.0;
    let view = [
        [q, 2.0, 0.0, 0.0],
        [0.0, q, 2.0, 0.0],
        [0.0, 0.0, q, 2.0],
        [0.0, 0.0, 0.0, q],
    ];
    let (built, annotations) = composed_fixture(view, identity());
    assert!(ComposedView::new(&annotations.composed_views[0]).is_none());
    assert_eq!(
        interpret(&built, Some(annotations))
            .scene
            .annotations
            .unwrap()
            .reasons,
        [ReasonCount {
            reason: Ineligibility::UnresolvedBinding,
            count: 1
        }]
    );
}

#[test]
fn composed_recovery_budgets_f32_rounding_at_large_world_translations() {
    let q = 1.0 / 65536.0;
    let view = [
        [27824.0 * q, 6264.0 * q, -16112.0 * q, 0.0],
        [0.0, -30520.0 * q, -11848.0 * q, 0.0],
        [-17296.0 * q, 10072.0 * q, -25928.0 * q, 0.0],
        [12160.5, -909.75, 751.75, 1.0],
    ];
    let mut model = identity();
    model[3] = [-19369.027, -994.2944, 14780.223, 1.0];
    let (built, annotations) = composed_fixture(view, mul4(model, view));
    let records = interpret(&built, Some(annotations))
        .scene
        .annotations
        .unwrap();
    assert!(records.reasons.is_empty(), "{:?}", records.reasons);
    for vertex in &records.draws[0].vertices {
        let recovered = vertex.transform.unwrap().model;
        for (recovered, expected) in recovered.iter().flatten().zip(model.iter().flatten()) {
            assert!((recovered - expected).abs() < 0.002);
        }
    }
}

#[test]
fn a_nested_entry_does_not_change_the_callers_span_occurrence() {
    let (mut dl, leaf) = lists();
    let entry = u64::from(dl.list(
        "main",
        &[gsp_displaylist(leaf), gsp_1triangle(0, 1, 2), gsp_enddl()],
    ));
    let built = dl.finish("main");
    for occurrence in [0, 1] {
        let records = interpret(
            &built,
            Some(TaskAnnotations {
                draws: vec![span(u64::from(leaf), entry + 24, Some(occurrence), 1)],
                ..Default::default()
            }),
        )
        .scene
        .annotations
        .unwrap();
        assert!(records.reasons.is_empty(), "{:?}", records.reasons);
        assert_eq!(
            records
                .draws
                .iter()
                .map(|d| d.annotation)
                .collect::<Vec<_>>(),
            [Some(0), (occurrence == 0).then_some(0)]
        );
    }
}
