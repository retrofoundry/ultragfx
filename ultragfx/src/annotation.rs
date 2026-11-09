use std::collections::HashMap;

use crate::{
    hle::{
        math::{identity, mul4, Mat4},
        mem::Rdram,
    },
    interpolation::{Ineligibility, ReasonCount},
};

mod composed;

use composed::ComposedView;

pub const ANNOTATION_ABI_VERSION: u32 = 3;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct DrawIdentity {
    pub namespace: u64,
    pub instance: u64,
    pub generation: u64,
    pub part: u64,
    pub subdraw: u32,
    pub pass: u32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InvocationSelector {
    /// Resolved address of the call or branch command, not its target.
    pub address: u64,
    pub occurrence: Option<u32>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CommandSpan {
    pub start: u64,
    pub end: u64,
    pub occurrence: Option<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScopeSelector {
    /// The resolved address of a call or branch command.
    Invocation(InvocationSelector),
    /// Commands with resolved addresses in a half-open range within one list.
    Commands(CommandSpan),
}

impl Default for ScopeSelector {
    fn default() -> Self {
        Self::Invocation(InvocationSelector::default())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MatrixRole {
    View,
    Projection,
    /// An opaque VP must stay unchanged across the pair.
    ViewProjection,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MatrixBinding {
    /// A native, physical or segmented operand resolved at each matrix read.
    pub operand: u64,
    pub role: MatrixRole,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MatrixPalette {
    pub base: u64,
    /// Consecutive 64-byte slots mapped to semantic parts; length is the slot count.
    pub parts: Vec<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BillboardProvenance {
    Cpu,
    Segment1,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BillboardRecipe {
    pub anchor: [f32; 3],
    pub scale: [f32; 3],
    /// Local Z rotation, in radians.
    pub spin: f32,
    /// Row-vector 3x3 basis from the guest's billboardMtxF.
    pub basis: [[f32; 3]; 3],
    pub provenance: BillboardProvenance,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum TransformPolicy {
    Interpolate,
    StaticWorld,
    HoldModel,
    BillboardHold(BillboardRecipe),
    ScreenSpace,
    #[default]
    Hold,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct DrawAnnotation {
    pub scope: ScopeSelector,
    /// Index into `TaskAnnotations::composed_views`.
    pub composed_view: Option<u32>,
    pub identity: DrawIdentity,
    pub policy: TransformPolicy,
    pub matrices: Vec<MatrixBinding>,
    pub palette: Option<MatrixPalette>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct TaskAnnotations {
    pub draws: Vec<DrawAnnotation>,
    /// CPU-composed views as 16 big-endian-order words in the fixed-point G_MTX layout.
    pub composed_views: Vec<[u32; 16]>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct RawLighting {
    pub lights: [([f32; 3], [f32; 3]); 8],
    pub ambient_col: [f32; 3],
    pub num_dir: u32,
    pub lookat_axes: [[f32; 3]; 2],
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ResolvedTransform {
    pub model: Mat4,
    pub view: Mat4,
    pub projection: Mat4,
    pub part: Option<u64>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CapturedVertexUse {
    pub transform: Option<ResolvedTransform>,
    pub lighting: Option<u32>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CapturedDraw {
    pub indices: u32,
    pub annotation: Option<usize>,
    pub vertices: [CapturedVertexUse; 3],
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CapturedAnnotations {
    pub sidecar: TaskAnnotations,
    pub lights: Vec<RawLighting>,
    pub draws: Vec<CapturedDraw>,
    pub reasons: Vec<ReasonCount>,
}

struct Scope {
    depth: usize,
    annotation: usize,
    commands: bool,
}

pub(crate) struct AnnotationCapture {
    records: CapturedAnnotations,
    occurrences: HashMap<u64, u32>,
    command_spans: Vec<(usize, CommandSpan)>,
    span_entries: Vec<u32>,
    list_spans: Vec<Vec<(usize, u32)>>,
    composed_views: Vec<Option<ComposedView>>,
    matches: Vec<u32>,
    collisions: Vec<bool>,
    scopes: Vec<Scope>,
    projection_reads: Vec<MatrixRead>,
    model_parts: [Vec<(usize, u64)>; crate::hle::rsp::RSP_MATRIX_STACK_SIZE],
    transforms: Vec<LoadedTransform>,
    vertices: Vec<LoadedVertex>,
}

#[derive(Clone, Debug, PartialEq)]
struct LoadedTransform {
    model: Mat4,
    viewproj: Mat4,
    mvp: Mat4,
    model_parts: Vec<(usize, u64)>,
    projection_reads: Vec<MatrixRead>,
}

#[derive(Clone, Debug, PartialEq)]
struct MatrixRead {
    matrix: Mat4,
    roles: Vec<(usize, MatrixRole)>,
}

#[derive(Clone, Copy)]
struct LoadedVertex {
    transform: u32,
    lighting: Option<u32>,
}

impl AnnotationCapture {
    pub(crate) fn new(sidecar: TaskAnnotations) -> Self {
        let count = sidecar.draws.len();
        let command_spans = sidecar
            .draws
            .iter()
            .enumerate()
            .filter_map(|(i, a)| match a.scope {
                ScopeSelector::Commands(span) => Some((i, span)),
                ScopeSelector::Invocation(_) => None,
            })
            .collect();
        let composed_views = sidecar
            .composed_views
            .iter()
            .map(ComposedView::new)
            .collect();
        Self {
            records: CapturedAnnotations {
                sidecar,
                lights: Vec::new(),
                draws: Vec::new(),
                reasons: Vec::new(),
            },
            occurrences: HashMap::new(),
            command_spans,
            span_entries: vec![0; count],
            list_spans: Vec::new(),
            composed_views,
            matches: vec![0; count],
            collisions: vec![false; count],
            scopes: Vec::new(),
            projection_reads: Vec::new(),
            model_parts: std::array::from_fn(|_| Vec::new()),
            transforms: Vec::new(),
            vertices: Vec::new(),
        }
    }

    pub(crate) fn enter(&mut self, pc: u64, depth: usize) {
        let occurrence = self.occurrences.entry(pc).or_default();
        let matching: Vec<_> = self
            .records
            .sidecar
            .draws
            .iter()
            .enumerate()
            .filter(|(_, a)| {
                matches!(a.scope, ScopeSelector::Invocation(selector)
                    if selector.address == pc && selector.occurrence.is_none_or(|n| n == *occurrence))
            })
            .map(|(i, _)| i)
            .collect();
        *occurrence += 1;
        for &i in &matching {
            self.matches[i] += 1;
            self.collisions[i] |= matching.len() > 1;
        }
        let Some(&annotation) = matching.first() else {
            return;
        };
        self.scopes.push(Scope {
            depth,
            annotation,
            commands: false,
        });
    }

    pub(crate) fn command(&mut self, pc: u64, depth: usize) {
        if self.command_spans.is_empty() {
            return;
        }
        self.list_spans.resize_with(depth + 1, Vec::new);
        let previous = &self.list_spans[depth];
        let mut inside = Vec::new();
        let mut matching = Vec::new();
        for &(i, span) in &self.command_spans {
            if !(span.start..span.end).contains(&pc) {
                continue;
            }
            let prior_entry = previous.iter().find(|&&(index, _)| index == i);
            let occurrence = if let Some(&(_, occurrence)) = prior_entry {
                occurrence
            } else {
                let occurrence = self.span_entries[i];
                self.span_entries[i] += 1;
                occurrence
            };
            inside.push((i, occurrence));
            if span.occurrence.is_none_or(|n| n == occurrence) {
                matching.push(i);
                if prior_entry.is_none() {
                    self.matches[i] += 1;
                    self.scopes.push(Scope {
                        depth,
                        annotation: i,
                        commands: true,
                    });
                }
            }
        }
        self.scopes
            .retain(|s| !s.commands || s.depth != depth || matching.contains(&s.annotation));
        for &annotation in &matching {
            self.collisions[annotation] |= matching.len() > 1;
        }
        self.list_spans[depth] = inside;
    }

    pub(crate) fn leave(&mut self, depth: usize) {
        self.scopes.retain(|scope| scope.depth <= depth);
        self.list_spans.truncate(depth + 1);
    }

    pub(crate) fn matrix(
        &mut self,
        memory: &impl Rdram,
        address: u64,
        matrix: Mat4,
        projection: bool,
        load: bool,
        model_top: usize,
    ) {
        if projection {
            if load {
                self.projection_reads.clear();
            }
            let roles = self
                .records
                .sidecar
                .draws
                .iter()
                .enumerate()
                .flat_map(|(i, a)| {
                    a.matrices.iter().filter_map(move |b| {
                        (memory.resolve_masked(b.operand).ok() == Some(address))
                            .then_some((i, b.role))
                    })
                })
                .collect();
            self.projection_reads.push(MatrixRead { matrix, roles });
        } else {
            self.model_parts[model_top] = self
                .records
                .sidecar
                .draws
                .iter()
                .enumerate()
                .filter_map(|(i, a)| {
                    let palette = a.palette.as_ref()?;
                    let base = memory.resolve_masked(palette.base).ok()?;
                    let offset = address.checked_sub(base)?;
                    if !offset.is_multiple_of(64) {
                        return None;
                    }
                    palette
                        .parts
                        .get((offset / 64) as usize)
                        .map(|&part| (i, part))
                })
                .collect();
        }
    }

    pub(crate) fn load_vertices(
        &mut self,
        model: Mat4,
        viewproj: Mat4,
        mvp: Mat4,
        model_top: usize,
        count: usize,
        lighting: Option<RawLighting>,
    ) {
        let state = LoadedTransform {
            model,
            viewproj,
            mvp,
            model_parts: self.model_parts[model_top].clone(),
            projection_reads: self.projection_reads.clone(),
        };
        let transform = if self.transforms.last() == Some(&state) {
            self.transforms.len() - 1
        } else {
            self.transforms.push(state);
            self.transforms.len() - 1
        } as u32;
        let lighting = lighting.map(|state| {
            self.records
                .lights
                .iter()
                .position(|previous| *previous == state)
                .unwrap_or_else(|| {
                    self.records.lights.push(state);
                    self.records.lights.len() - 1
                }) as u32
        });
        self.vertices.extend(std::iter::repeat_n(
            LoadedVertex {
                transform,
                lighting,
            },
            count,
        ));
    }

    pub(crate) fn clone_vertex(&mut self, vertex: usize) {
        self.vertices.push(self.vertices[vertex]);
    }

    pub(crate) fn draw(&mut self, indices: u32, vertices: [u32; 3]) {
        let scope = self.scopes.last();
        let annotation = scope.map(|s| s.annotation);
        let mut reasons = Vec::new();
        let captured = vertices.map(|vertex| {
            let loaded = self.vertices[vertex as usize];
            let transform = scope.and_then(|scope| match self.resolve(scope, &loaded) {
                Ok(transform) => transform,
                Err(error) => {
                    if !reasons.contains(&error) {
                        reasons.push(error);
                    }
                    None
                }
            });
            CapturedVertexUse {
                transform,
                lighting: loaded.lighting,
            }
        });
        for reason in reasons {
            ReasonCount::add(&mut self.records.reasons, reason, 1);
        }
        let world = vertices.iter().any(|&v| {
            self.transforms[self.vertices[v as usize].transform as usize]
                .projection_reads
                .iter()
                .any(|read| !read.roles.is_empty())
        });
        if world {
            let reason = match annotation.map(|i| self.records.sidecar.draws[i].policy) {
                None => Some(Ineligibility::UnknownWorldDraw),
                Some(TransformPolicy::Hold) => Some(Ineligibility::UnsupportedTopology),
                _ => None,
            };
            if let Some(reason) = reason {
                ReasonCount::add(&mut self.records.reasons, reason, 1);
            }
        }
        self.records.draws.push(CapturedDraw {
            indices,
            annotation,
            vertices: captured,
        });
    }

    fn camera(
        &self,
        annotation: usize,
        state: &LoadedTransform,
        composed: Option<&ComposedView>,
    ) -> Option<(Mat4, Mat4)> {
        let declaration = &self.records.sidecar.draws[annotation];
        let mut view = if let Some(composed) = composed {
            if !matches!(
                declaration.matrices.as_slice(),
                [MatrixBinding {
                    role: MatrixRole::Projection,
                    ..
                }]
            ) {
                return None;
            }
            Some(composed.matrix)
        } else {
            None
        };
        let mut projection = None;
        for read in &state.projection_reads {
            for &(_, role) in read.roles.iter().filter(|(i, _)| *i == annotation) {
                let target = match role {
                    MatrixRole::View => &mut view,
                    MatrixRole::Projection => &mut projection,
                    MatrixRole::ViewProjection => {
                        if view.replace(identity()).is_some()
                            || projection.replace(read.matrix).is_some()
                        {
                            return None;
                        }
                        continue;
                    }
                };
                if target.replace(read.matrix).is_some() {
                    return None;
                }
            }
        }
        let (view, projection) = (view?, projection?);
        let expected = if composed.is_some() {
            projection
        } else {
            mul4(view, projection)
        };
        (expected == state.viewproj).then_some((view, projection))
    }

    fn resolve(
        &self,
        scope: &Scope,
        vertex: &LoadedVertex,
    ) -> Result<Option<ResolvedTransform>, Ineligibility> {
        let annotation = &self.records.sidecar.draws[scope.annotation];
        if matches!(
            annotation.policy,
            TransformPolicy::Hold | TransformPolicy::ScreenSpace
        ) {
            return Ok(None);
        }
        let composed_view = annotation
            .composed_view
            .map(|index| {
                self.composed_views
                    .get(index as usize)
                    .and_then(Option::as_ref)
                    .ok_or(Ineligibility::UnresolvedBinding)
            })
            .transpose()?;
        let state = &self.transforms[vertex.transform as usize];
        let (view, projection) = self
            .camera(scope.annotation, state, composed_view)
            .ok_or(Ineligibility::UnresolvedBinding)?;
        let effective = match composed_view {
            Some(view) => view
                .model(state.model)
                .ok_or(Ineligibility::UnresolvedBinding)?,
            None => state.model,
        };
        let model = if annotation.policy == TransformPolicy::StaticWorld {
            identity()
        } else {
            effective
        };
        if model != effective || mul4(state.model, state.viewproj) != state.mvp {
            return Err(Ineligibility::UnresolvedBinding);
        }
        if let TransformPolicy::BillboardHold(recipe) = annotation.policy {
            if crate::interpolation::math::billboard_model(recipe) != Some(model) {
                return Err(Ineligibility::UnsafeBillboard);
            }
        }
        let part = if annotation.palette.is_some() {
            Some(
                state
                    .model_parts
                    .iter()
                    .find(|(i, _)| *i == scope.annotation)
                    .ok_or(Ineligibility::UnresolvedBinding)?
                    .1,
            )
        } else {
            None
        };
        Ok(Some(ResolvedTransform {
            model,
            view,
            projection,
            part,
        }))
    }

    pub(crate) fn finish(mut self) -> CapturedAnnotations {
        for (matches, collision) in self.matches.into_iter().zip(self.collisions) {
            if matches != 1 || collision {
                ReasonCount::add(&mut self.records.reasons, Ineligibility::UnresolvedScope, 1);
            }
        }
        let mut identities = HashMap::new();
        for declaration in &self.records.sidecar.draws {
            if identities.insert(declaration.identity, ()).is_some() {
                ReasonCount::add(
                    &mut self.records.reasons,
                    Ineligibility::UnsupportedTopology,
                    1,
                );
            }
        }
        self.records
    }
}

#[cfg(test)]
mod tests;
