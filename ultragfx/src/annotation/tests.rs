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
        scope: InvocationSelector {
            address,
            occurrence: None,
        },
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
        annotation.scope.occurrence = occurrence;
        interpret(
            &built,
            Some(TaskAnnotations {
                draws: vec![annotation],
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
    let captured = interpret(&built, Some(TaskAnnotations { draws: vec![a] }))
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
    let captured = interpret(&built, Some(TaskAnnotations { draws: vec![a] }))
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
    let captured = interpret(&built, Some(TaskAnnotations { draws: vec![a] }))
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
