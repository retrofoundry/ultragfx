pub(crate) mod math;

use crate::{
    annotation::{
        BillboardRecipe, CapturedAnnotations, DrawIdentity, TaskAnnotations, TransformPolicy,
    },
    hle::math::Mat4,
    render::{
        isolated::{IsolatedContext, IsolatedRenderer, TargetSnapshot},
        workload::{TargetId, Workload},
    },
    scene::{Scene, SceneOp},
    DlSummary, Microcode,
};
use std::{
    collections::HashMap,
    ops::Range,
    sync::{Arc, Weak},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TaskInfo {
    pub cookie: u64,
    pub source: u64,
    pub epoch: u64,
    pub producer_sample: u64,
}

pub struct TaskSubmission<'a> {
    pub info: TaskInfo,
    /// None opts out of capture; Some(empty) retains an endpoint with held geometry.
    pub annotations: Option<&'a TaskAnnotations>,
    pub microcode_bindings: &'a [(u64, Microcode)],
}

#[derive(Clone)]
pub struct TaskHandle {
    info: TaskInfo,
    pub summary: DlSummary,
    pub(crate) retained: Option<Arc<RetainedTask>>,
}

impl TaskHandle {
    pub fn info(&self) -> TaskInfo {
        self.info
    }
}

pub(crate) struct RetainedTask {
    scene: Scene,
    context: IsolatedContext,
    targets: HashMap<TargetId, Arc<TargetSnapshot>>,
    eligibility: Vec<ReasonCount>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceScanout {
    pub task_cookie: u64,
    pub address: u64,
    pub rows: Option<Range<u32>>,
    pub visible_at: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ineligibility {
    FramebufferTextureLoad,
    FramebufferCopy,
    LiveFramebufferAlias,
    InheritedAttachmentContents,
    InheritedDecalDepth,
    CpuImportDependency,
    TextureReplacement,
    UnsupportedTask,
    Multitask,
    UnresolvedScope,
    UnresolvedBinding,
    UnknownWorldDraw,
    UnsafeBillboard,
    UnsupportedTopology,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReasonCount {
    pub reason: Ineligibility,
    pub count: u64,
}

impl ReasonCount {
    pub(crate) fn add(reasons: &mut Vec<Self>, reason: Ineligibility, count: u64) {
        if let Some(entry) = reasons.iter_mut().find(|entry| entry.reason == reason) {
            entry.count += count;
        } else {
            reasons.push(Self { reason, count });
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SourceCounts {
    pub triangles: u64,
    pub held_triangles: u64,
    pub rectangles: u64,
    pub fills: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceError {
    UnrecordedTask,
    TaskNotFound,
    SourceMismatch,
    TargetNotFound,
}

#[derive(Clone)]
pub struct SourceFrame {
    data: Arc<SourceData>,
}

struct SourceData {
    info: TaskInfo,
    visible_at: u64,
    task: Arc<RetainedTask>,
    target: TargetId,
    rows: Option<Range<u32>>,
    image: Arc<TargetSnapshot>,
    counts: SourceCounts,
    eligibility: Vec<ReasonCount>,
}

impl SourceFrame {
    pub fn info(&self) -> TaskInfo {
        self.data.info
    }
    pub fn visible_at(&self) -> u64 {
        self.data.visible_at
    }
    pub fn counts(&self) -> SourceCounts {
        self.data.counts
    }
    pub fn ineligibility(&self) -> &[ReasonCount] {
        &self.data.eligibility
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RestrictionReason {
    NoMatch,
    TopologyChanged,
    PolicyChanged,
    DecompositionRejected,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DrawRestriction {
    pub reason: RestrictionReason,
    /// Number of affected scopes, regardless of their triangle counts.
    pub count: u64,
    /// First four affected identities, in A's submission order.
    pub identities: Vec<DrawIdentity>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PhaseResult {
    Interpolated { restrictions: Vec<DrawRestriction> },
    Endpoint,
    Held,
}

#[derive(Debug)]
pub enum PhaseError {
    InvalidPhase,
    Render(crate::Diagnostic),
}

pub(crate) struct PhaseState {
    renderer: IsolatedRenderer,
    pair: Option<Pair>,
}

struct Pair {
    a: Weak<SourceData>,
    b: Weak<SourceData>,
    correspondence: Option<Correspondence>,
}

struct Correspondence {
    draws: Vec<PhaseDraw>,
    restrictions: Vec<DrawRestriction>,
}

struct PhaseDraw {
    triangle: usize,
    model: Option<ModelMatch>,
    views: [Mat4; 3],
}

enum ModelMatch {
    Interpolate(usize),
    Billboard(BillboardRecipe, BillboardRecipe),
}

pub(crate) struct Recording<'a> {
    pub(crate) annotations: &'a TaskAnnotations,
    pub(crate) task: Option<Arc<RetainedTask>>,
}

impl RetainedTask {
    pub(crate) fn new(
        scene: Scene,
        context: IsolatedContext,
        targets: HashMap<TargetId, Arc<TargetSnapshot>>,
        cpu_imported: bool,
        replacements: bool,
        summary: DlSummary,
    ) -> Self {
        let mut eligibility = eligibility(&scene, context, cpu_imported, &targets);
        if replacements {
            ReasonCount::add(&mut eligibility, Ineligibility::TextureReplacement, 1);
        }
        if summary.errors != 0
            || summary.warns != 0
            || summary.dropped_runs != 0
            || summary.termination != crate::inspect::WalkTermination::End
        {
            ReasonCount::add(&mut eligibility, Ineligibility::UnsupportedTask, 1);
        }
        Self {
            scene,
            context,
            targets,
            eligibility,
        }
    }
}

fn eligibility(
    scene: &Scene,
    context: IsolatedContext,
    cpu_imported: bool,
    targets: &HashMap<TargetId, Arc<TargetSnapshot>>,
) -> Vec<ReasonCount> {
    use crate::hle::texture_request::TextureSource;
    let mut reasons = scene
        .annotations
        .as_ref()
        .map_or_else(Vec::new, |a| a.reasons.clone());
    if let Some(records) = &scene.annotations {
        for draw in &records.draws {
            if draw.annotation.is_some_and(|i| {
                !matches!(
                    records.sidecar.draws[i].policy,
                    TransformPolicy::Hold | TransformPolicy::ScreenSpace
                )
            }) && scene.indices[draw.indices as usize..draw.indices as usize + 3]
                .iter()
                .any(|&v| scene.modify_flags[v as usize] != 0)
            {
                ReasonCount::add(&mut reasons, Ineligibility::UnsupportedTopology, 1);
            }
        }
    }
    if cpu_imported {
        ReasonCount::add(&mut reasons, Ineligibility::CpuImportDependency, 1);
    }
    if scene
        .materials
        .iter()
        .flat_map(|m| m.texture_sources())
        .any(|s| matches!(s, TextureSource::Framebuffer(_)))
    {
        ReasonCount::add(&mut reasons, Ineligibility::FramebufferTextureLoad, 1);
    }
    let workload = Workload::new(scene);
    let mut known: HashMap<(TargetId, bool), Vec<[i32; 4]>> = HashMap::new();
    let mut layouts = HashMap::new();
    for target in &workload.targets {
        let extent = if target.id == TargetId::Legacy {
            context.legacy_extent
        } else {
            target.logical_extent
        };
        if targets
            .get(&target.id)
            .is_some_and(|image| image.layout.width != extent.0 || image.height > extent.1)
        {
            ReasonCount::add(&mut reasons, Ineligibility::InheritedAttachmentContents, 1);
        }
        let bounds = [0, 0, extent.0 as i32, extent.1 as i32];
        let layout = crate::render::framebuffers::ImageLayout {
            width: extent.0,
            fmt: target.color_image.fmt,
            siz: target.color_image.siz,
        };
        let key = (target.id, target.depth_clear);
        if layouts
            .insert(key, layout)
            .is_some_and(|previous| previous != layout)
        {
            ReasonCount::add(&mut reasons, Ineligibility::InheritedAttachmentContents, 1);
            known.remove(&key);
        }
        for op in &target.operations {
            match &op.draw {
                SceneOp::FramebufferCopy(_) => {
                    ReasonCount::add(&mut reasons, Ineligibility::FramebufferCopy, 1)
                }
                SceneOp::TexRect {
                    fb_source: Some(_), ..
                } => ReasonCount::add(&mut reasons, Ineligibility::LiveFramebufferAlias, 1),
                _ => {}
            }
            if let SceneOp::FillRect { rect, .. } = op.draw {
                let presentation = if context.render_scale > 1 {
                    op.presentation_scissor
                } else {
                    op.scissor
                };
                let area = [
                    rect.ulx.max(op.scissor.ulx).max(presentation.ulx),
                    rect.uly.max(op.scissor.uly).max(presentation.uly),
                    (rect.lrx + 1).min(op.scissor.lrx).min(presentation.lrx),
                    (rect.lry + 1).min(op.scissor.lry).min(presentation.lry),
                ];
                known.entry(key).or_default().push(area);
            } else if !target.depth_clear {
                if !covered(bounds, known.get(&key).map_or(&[], Vec::as_slice)) {
                    ReasonCount::add(&mut reasons, Ineligibility::InheritedAttachmentContents, 1);
                }
                if let Some(depth) = op.depth_image {
                    if layouts
                        .get(&(TargetId::Guest(depth), true))
                        .is_none_or(|layout| layout.width != extent.0)
                        || !covered(
                            bounds,
                            known
                                .get(&(TargetId::Guest(depth), true))
                                .map_or(&[], Vec::as_slice),
                        )
                    {
                        ReasonCount::add(
                            &mut reasons,
                            Ineligibility::InheritedAttachmentContents,
                            1,
                        );
                        let mode = match &op.draw {
                            SceneOp::Tris(run) => Some(run.render_mode_index),
                            SceneOp::TexRect {
                                render_mode_index, ..
                            } => Some(*render_mode_index),
                            _ => None,
                        };
                        if mode.is_some_and(|index| {
                            let mode = &scene.render_modes[index as usize];
                            mode.z_test && mode.z_mode == crate::hle::ZMode::Decal
                        }) {
                            ReasonCount::add(&mut reasons, Ineligibility::InheritedDecalDepth, 1);
                        }
                    }
                }
            }
        }
        if !target.depth_clear && !covered(bounds, known.get(&key).map_or(&[], Vec::as_slice)) {
            ReasonCount::add(&mut reasons, Ineligibility::InheritedAttachmentContents, 1);
        }
    }
    reasons
}

fn source_counts(scene: &Scene) -> SourceCounts {
    let mut counts = SourceCounts {
        triangles: (scene.indices.len() / 3) as u64,
        ..Default::default()
    };
    if let Some(records) = &scene.annotations {
        counts.held_triangles = records
            .draws
            .iter()
            .filter(|d| {
                d.annotation.is_none_or(|i| {
                    !matches!(
                        records.sidecar.draws[i].policy,
                        TransformPolicy::Interpolate | TransformPolicy::StaticWorld
                    )
                })
            })
            .count() as u64;
    }
    for op in scene.framebuffer_pairs.iter().flat_map(|pair| &pair.ops) {
        match op {
            SceneOp::TexRect { .. } => counts.rectangles += 1,
            SceneOp::FillRect { .. } => counts.fills += 1,
            _ => {}
        }
    }
    counts
}

fn covered(bounds: [i32; 4], rectangles: &[[i32; 4]]) -> bool {
    let mut remaining = vec![bounds];
    for &[left, top, right, bottom] in rectangles {
        let mut next = Vec::new();
        for [l, t, r, b] in remaining {
            let (x0, y0, x1, y1) = (l.max(left), t.max(top), r.min(right), b.min(bottom));
            if x0 >= x1 || y0 >= y1 {
                next.push([l, t, r, b]);
                continue;
            }
            for rect in [
                [l, t, r, y0],
                [l, y1, r, b],
                [l, y0, x0, y1],
                [x1, y0, r, y1],
            ] {
                if rect[0] < rect[2] && rect[1] < rect[3] {
                    next.push(rect);
                }
            }
        }
        remaining = next;
    }
    remaining.is_empty()
}

impl crate::Renderer {
    /// Execute one canonical task; annotations are copied before interpretation.
    pub fn submit_task(
        &mut self,
        hw: &impl crate::Hardware,
        entry: u64,
        ucode: Microcode,
        diags: &mut dyn crate::DiagSink,
        submission: TaskSubmission<'_>,
    ) -> TaskHandle {
        self.submit_task_memory(hw.rdram(), entry, ucode, diags, submission)
    }

    /// Execute a native-pointer task with the memory contract of `process_dl_host`.
    ///
    /// # Safety
    /// Every reachable command and input must satisfy [`Self::process_dl_host`].
    #[cfg(all(not(target_arch = "wasm32"), target_pointer_width = "64"))]
    pub unsafe fn submit_task_host(
        &mut self,
        ram: crate::HostRam<'_>,
        entry: u64,
        ucode: Microcode,
        diags: &mut dyn crate::DiagSink,
        submission: TaskSubmission<'_>,
    ) -> TaskHandle {
        let memory = unsafe { crate::hle::host_mem::HostMemory::new(ram) };
        self.submit_task_memory(memory, entry, ucode, diags, submission)
    }

    fn submit_task_memory(
        &mut self,
        memory: impl crate::Rdram,
        entry: u64,
        ucode: Microcode,
        diags: &mut dyn crate::DiagSink,
        submission: TaskSubmission<'_>,
    ) -> TaskHandle {
        let mut recording = submission.annotations.map(|annotations| Recording {
            annotations,
            task: None,
        });
        let summary = self.process_dl_memory_observed(
            memory,
            entry,
            ucode,
            diags,
            None,
            crate::Submission::DiscardOnStop,
            submission.microcode_bindings,
            recording.as_mut(),
        );
        TaskHandle {
            info: submission.info,
            summary,
            retained: recording.and_then(|recording| recording.task),
        }
    }

    /// Associate visibility with an immutable task version. Completion alone creates no source.
    pub fn commit_source(
        &mut self,
        tasks: &[TaskHandle],
        scanout: SourceScanout,
    ) -> Result<SourceFrame, SourceError> {
        let selected = tasks
            .iter()
            .position(|task| task.info.cookie == scanout.task_cookie)
            .ok_or(SourceError::TaskNotFound)?;
        let info = tasks[selected].info;
        let mut reasons = Vec::new();
        let mut counts = SourceCounts::default();
        for task in tasks {
            if task.info.source != info.source || task.info.epoch != info.epoch {
                return Err(SourceError::SourceMismatch);
            }
            let task = task.retained.as_ref().ok_or(SourceError::UnrecordedTask)?;
            let task_counts = source_counts(&task.scene);
            counts.triangles += task_counts.triangles;
            counts.held_triangles += task_counts.held_triangles;
            counts.rectangles += task_counts.rectangles;
            counts.fills += task_counts.fills;
            for &reason in &task.eligibility {
                ReasonCount::add(&mut reasons, reason.reason, reason.count);
            }
        }
        if tasks.len() != 1 {
            ReasonCount::add(&mut reasons, Ineligibility::Multitask, 1);
        }
        let task = tasks[selected].retained.as_ref().unwrap();
        let target = TargetId::Guest(scanout.address);
        let image = task
            .targets
            .get(&target)
            .ok_or(SourceError::TargetNotFound)?
            .clone();
        Ok(SourceFrame {
            data: Arc::new(SourceData {
                info,
                visible_at: scanout.visible_at,
                task: task.clone(),
                target,
                rows: scanout.rows,
                image,
                eligibility: reasons,
                counts,
            }),
        })
    }
}

fn camera(records: &CapturedAnnotations, draws: &[usize]) -> Option<(Mat4, Mat4)> {
    let mut cameras = draws
        .iter()
        .flat_map(|&i| &records.draws[i].vertices)
        .filter_map(|v| v.transform)
        .map(|t| (t.view, t.projection));
    let first = cameras.next()?;
    cameras.all(|value| value == first).then_some(first)
}

fn correspondence(a: &Scene, b: &Scene) -> Option<Correspondence> {
    let (Some(ar), Some(br)) = (&a.annotations, &b.annotations) else {
        return Some(Correspondence {
            draws: Vec::new(),
            restrictions: Vec::new(),
        });
    };
    let mut correspondence = Correspondence {
        draws: Vec::new(),
        restrictions: Vec::new(),
    };
    let common_camera = camera(br, &(0..br.draws.len()).collect::<Vec<_>>());
    let collect = |records: &CapturedAnnotations| {
        let mut groups: HashMap<usize, Vec<usize>> = HashMap::new();
        for (i, draw) in records.draws.iter().enumerate() {
            if let Some(annotation) = draw.annotation {
                groups.entry(annotation).or_default().push(i);
            }
        }
        groups
    };
    let (ag, bg) = (collect(ar), collect(br));
    let identities: HashMap<_, _> = br
        .sidecar
        .draws
        .iter()
        .enumerate()
        .map(|(i, d)| (d.identity, i))
        .collect();
    let mut cameras = Vec::new();
    for (&ai, adraws) in &ag {
        let Some(bdraws) = identities
            .get(&ar.sidecar.draws[ai].identity)
            .and_then(|bi| bg.get(bi))
        else {
            continue;
        };
        let scope_camera = camera(br, bdraws);
        for (i, &ad) in adraws.iter().enumerate() {
            for corner in 0..3 {
                let at = ar.draws[ad].vertices[corner].transform;
                let bt = scope_camera.or_else(|| {
                    (adraws.len() == bdraws.len())
                        .then(|| br.draws[bdraws[i]].vertices[corner].transform)
                        .flatten()
                        .map(|t| (t.view, t.projection))
                });
                if let (Some(at), Some(bt)) = (at, bt) {
                    let pair = ((at.view, at.projection), bt);
                    if !cameras.contains(&pair) {
                        cameras.push(pair);
                    }
                }
            }
        }
    }
    let mut scopes: Vec<_> = ag.into_iter().collect();
    scopes.sort_unstable_by_key(|(_, draws)| draws[0]);
    for (ai, adraws) in scopes {
        let annotation = &ar.sidecar.draws[ai];
        if matches!(
            annotation.policy,
            TransformPolicy::Hold | TransformPolicy::ScreenSpace
        ) {
            continue;
        }
        let matched = identities.get(&annotation.identity).copied();
        let bdraws = matched
            .and_then(|bi| bg.get(&bi))
            .map_or(&[][..], Vec::as_slice);
        let scope_camera = camera(br, bdraws);
        let restriction = if matches!(
            annotation.policy,
            TransformPolicy::Interpolate | TransformPolicy::BillboardHold(_)
        ) {
            match matched {
                None => Some(RestrictionReason::NoMatch),
                Some(bi) => {
                    let bp = br.sidecar.draws[bi].policy;
                    match (annotation.policy, bp) {
                        (TransformPolicy::Interpolate, TransformPolicy::Interpolate) => {
                            if adraws.len() != bdraws.len()
                                || adraws.iter().zip(bdraws).any(|(&ad, &bd)| {
                                    (0..3).any(|corner| {
                                        let at = ar.draws[ad].vertices[corner].transform.unwrap();
                                        let bt = br.draws[bd].vertices[corner].transform.unwrap();
                                        let av = a.indices[ar.draws[ad].indices as usize + corner]
                                            as usize;
                                        let bv = b.indices[br.draws[bd].indices as usize + corner]
                                            as usize;
                                        at.part != bt.part || a.raw_pos[av] != b.raw_pos[bv]
                                    })
                                })
                            {
                                Some(RestrictionReason::TopologyChanged)
                            } else if adraws.iter().zip(bdraws).any(|(&ad, &bd)| {
                                (0..3).any(|corner| {
                                    math::rigid(
                                        ar.draws[ad].vertices[corner].transform.unwrap().model,
                                        br.draws[bd].vertices[corner].transform.unwrap().model,
                                        0.5,
                                    )
                                    .is_none()
                                })
                            }) {
                                Some(RestrictionReason::DecompositionRejected)
                            } else {
                                None
                            }
                        }
                        (TransformPolicy::BillboardHold(a), TransformPolicy::BillboardHold(b)) => {
                            math::billboard(a, b, 0.5)
                                .is_none()
                                .then_some(RestrictionReason::DecompositionRejected)
                        }
                        _ => Some(RestrictionReason::PolicyChanged),
                    }
                }
            }
        } else {
            None
        };
        if let Some(reason) = restriction {
            let restrictions = &mut correspondence.restrictions;
            let index = restrictions
                .iter()
                .position(|r| r.reason == reason)
                .unwrap_or_else(|| {
                    restrictions.push(DrawRestriction {
                        reason,
                        count: 0,
                        identities: Vec::new(),
                    });
                    restrictions.len() - 1
                });
            let entry = &mut restrictions[index];
            entry.count += 1;
            if entry.identities.len() < 4 {
                entry.identities.push(annotation.identity);
            }
        }
        for (i, &ad) in adraws.iter().enumerate() {
            let mut views = [crate::hle::math::identity(); 3];
            for (corner, view) in views.iter_mut().enumerate() {
                let at = ar.draws[ad].vertices[corner].transform.unwrap();
                let target = scope_camera
                    .or_else(|| {
                        (adraws.len() == bdraws.len())
                            .then(|| br.draws[bdraws[i]].vertices[corner].transform)
                            .flatten()
                            .map(|t| (t.view, t.projection))
                    })
                    .or(common_camera)
                    .or_else(|| {
                        let mut candidates = cameras
                            .iter()
                            .filter(|(a, _)| *a == (at.view, at.projection))
                            .map(|(_, b)| *b);
                        let first = candidates.next()?;
                        candidates.all(|c| c == first).then_some(first)
                    })?;
                if at.projection != target.1 {
                    return None;
                }
                *view = target.0;
            }
            let model = if restriction.is_none() {
                match annotation.policy {
                    TransformPolicy::Interpolate => Some(ModelMatch::Interpolate(bdraws[i])),
                    TransformPolicy::BillboardHold(a) => {
                        let TransformPolicy::BillboardHold(b) =
                            br.sidecar.draws[matched.unwrap()].policy
                        else {
                            unreachable!()
                        };
                        Some(ModelMatch::Billboard(a, b))
                    }
                    _ => None,
                }
            } else {
                None
            };
            correspondence.draws.push(PhaseDraw {
                triangle: ad,
                model,
                views,
            });
        }
    }
    Some(correspondence)
}

fn phase_scene(a: &Scene, b: &Scene, correspondence: &Correspondence, t: f32) -> Scene {
    use crate::hle::{math::mul4, rsp::fold_direction};
    let mut scene = a.clone();
    let (Some(ar), Some(br)) = (&a.annotations, &b.annotations) else {
        return scene;
    };
    for draw in &correspondence.draws {
        let ad = &ar.draws[draw.triangle];
        for (corner, a) in ad.vertices.iter().enumerate() {
            let at = a.transform.unwrap();
            let model = match draw.model {
                Some(ModelMatch::Interpolate(bi)) => math::rigid(
                    at.model,
                    br.draws[bi].vertices[corner].transform.unwrap().model,
                    t,
                )
                .unwrap(),
                Some(ModelMatch::Billboard(a, b)) => math::billboard(a, b, t).unwrap(),
                None => at.model,
            };
            let vertex =
                scene.duplicate_vertex(scene.indices[ad.indices as usize + corner] as usize);
            let mvp = mul4(
                model,
                mul4(math::linear(at.view, draw.views[corner], t), at.projection),
            );
            scene.mtx_index[vertex] = scene.mvp_table.len() as u32;
            scene.mvp_table.push(mvp);
            if let Some(raw) = a.lighting.map(|i| &ar.lights[i as usize]) {
                if scene.light_count[vertex] != 0 {
                    scene.light_index[vertex] = scene.lights_table.len() as u32;
                    for &(dir, color) in &raw.lights[..raw.num_dir as usize] {
                        scene.lights_table.push((fold_direction(model, dir), color));
                    }
                    scene.lights_table.push(([0.0; 3], raw.ambient_col));
                }
                if scene.texgen_mode[vertex] != 0 {
                    scene.lookat_index[vertex] = scene.lookat_table.len() as u32;
                    scene.lookat_table.push((
                        fold_direction(model, raw.lookat_axes[0]),
                        fold_direction(model, raw.lookat_axes[1]),
                    ));
                }
            }
            scene.indices[ad.indices as usize + corner] = vertex as u32;
        }
    }
    scene.annotations = None;
    scene
}

impl crate::Renderer {
    /// Render a source pair into a caller-owned target without advancing canonical state.
    pub fn render_phase(
        &mut self,
        a: &SourceFrame,
        b: &SourceFrame,
        t: f32,
        target: &wgpu::TextureView,
    ) -> Result<PhaseResult, PhaseError> {
        if !t.is_finite() || !(0.0..=1.0).contains(&t) {
            return Err(PhaseError::InvalidPhase);
        }
        let source = if t == 1.0 { &b.data } else { &a.data };
        let held = b.data.visible_at <= a.data.visible_at
            || a.data.info.epoch != b.data.info.epoch
            || !a.data.eligibility.is_empty()
            || !b.data.eligibility.is_empty()
            || a.data.image.scale != b.data.image.scale
            || a.data.image.layout != b.data.image.layout
            || a.data.image.height != b.data.image.height
            || a.data.rows != b.data.rows;
        let mut result = if held {
            PhaseResult::Held
        } else {
            PhaseResult::Endpoint
        };
        let rendered;
        let image = if held {
            &*source.image
        } else {
            let task = &source.task;
            let state = self.phases.get_or_insert_with(|| PhaseState {
                renderer: IsolatedRenderer::new(
                    &self.device,
                    self.surface_format,
                    task.context.legacy_extent.0,
                    task.context.legacy_extent.1,
                    self.device
                        .features()
                        .contains(wgpu::Features::DUAL_SOURCE_BLENDING),
                ),
                pair: None,
            });
            let intermediate = t != 0.0 && t != 1.0;
            let scene = if intermediate {
                let atask = &a.data.task;
                let btask = &b.data.task;
                if !state.pair.as_ref().is_some_and(|pair| {
                    pair.a.ptr_eq(&Arc::downgrade(&a.data))
                        && pair.b.ptr_eq(&Arc::downgrade(&b.data))
                }) {
                    state.pair = Some(Pair {
                        a: Arc::downgrade(&a.data),
                        b: Arc::downgrade(&b.data),
                        correspondence: correspondence(&atask.scene, &btask.scene),
                    });
                }
                if let Some(correspondence) = &state.pair.as_ref().unwrap().correspondence {
                    result = PhaseResult::Interpolated {
                        restrictions: correspondence.restrictions.clone(),
                    };
                    phase_scene(&atask.scene, &btask.scene, correspondence, t)
                } else {
                    result = PhaseResult::Held;
                    atask.scene.clone()
                }
            } else {
                task.scene.clone()
            };
            rendered = state
                .renderer
                .render(
                    &self.device,
                    &self.queue,
                    &scene,
                    task.context,
                    source.target,
                )
                .map_err(PhaseError::Render)?;
            &rendered
        };
        let mut encoder = self.device.create_command_encoder(&Default::default());
        self.inner.scanout_snapshot(
            image,
            source.rows.clone(),
            &self.device,
            &mut encoder,
            target,
        );
        self.queue.submit([encoder.finish()]);
        Ok(result)
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests;
