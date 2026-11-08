# Owned-surface interpolation

Native owned surfaces support `RefreshRate::Original` (the default), `Display`,
and `Manual(hz)`. `set_refresh_rate` queues the setting for the next scanout.
`set_display_refresh_rate(Some(millihz))` supplies the monitor's nominal rate;
`None` explicitly means unknown and uses 60 Hz. Manual keeps the requested Hz and
clamps the effective rate to the monitor. Rates must have a finite positive,
representable period. `requested_refresh_rate()` returns the request;
`effective_refresh_rate()` and `display_refresh_rate()` describe the last scanout.
Wasm32 and targets without an owned surface accept Original only.

`interpolation_active()` also describes the last scanout. Hosts can omit task
annotations while it is false. Changing the rate does not enable retention by
itself. Original acknowledges each present. Display and Manual acknowledge
acceptance and present asynchronously; fatal worker errors are latched in
`presenter_error()` and returned by later calls.

The present thread paces before surface acquisition, using the effective rate
and the measured previous present. FIFO is a backstop. It drains a bounded batch
of messages before serving at most one due presentation and keeps the newest
sources. Entering Original discards interpolation work and acknowledges the
boundary. Entering Display or Manual shows plain scanouts until two compatible
retained sources are available. Interior phases are delayed by the larger of the
last two source intervals plus the preparation lead. The preceding pair stays
available until the delayed sample reaches the latest pair. This absorbs source
jitter and preparation time without extrapolating past the newest source. The
phase target is an estimate, not physical scanout or a VRR promise.

`annotation::ANNOTATION_ABI_VERSION` is 2; the host checks it during registration.
`Renderer::submit_task` and the
64-bit native `unsafe submit_task_host` execute the ordinary canonical task and
return an owned `interpolation::TaskHandle`. `TaskSubmission` supplies a unique
host task cookie, source ID, epoch, producer sample, microcode bindings, and an
optional borrowed `TaskAnnotations`. The renderer copies the annotations during
submission. Guest inputs must stay stable until submission returns; retained
scenes and textures then own their data.

`annotations: None` records no interpolation data and retains no task image.
Existing `process_dl` entry points use that path. `Some(empty_annotations)` retains
an endpoint whose geometry is held. Handles belong to the renderer that created
them.

Each `DrawAnnotation::scope` selects the resolved address of a call or branch
command, with an optional zero-based execution occurrence within the task. The
scope inherits through nested lists and branch chains until that call returns.
A nested declaration overrides its parent until its own invocation returns. A
missing selector, multiple matching declarations, or an occurrence-free selector
executed more than once makes the source ineligible. The engine never chooses
one execution of an ambiguous selector. OoT scopes calls emitted into dynamic
lists, each executed once; shared static leaf lists need no annotations.

`DrawIdentity` contains namespace, instance, generation, part, subdraw and pass.
Use a new generation when an instance is reused or its representation changes.
Only this identity matches scopes across sources; addresses and task draw order
are never identities. Triangle order within a matched scope supplies corner
correspondence only for Interpolate scopes. Duplicate identities make a source
ineligible. StaticWorld, HoldModel, ScreenSpace and Hold use A's own geometry,
including despawned scopes and scopes with different triangle counts. B-only
content appears at t=1. Interpolate scopes with a missing identity, changed
triangle count, local vertex positions or palette parts, incompatible policy,
or rejected model decomposition fall back to HoldModel for that entire scope.
Other scopes continue interpolating.

`MatrixBinding { operand, role }` declares View and Projection, or ViewProjection.
Operands may be native, physical or segmented addresses. Each successful canonical
matrix read resolves declarations through the segment table at that read, after
any guest rewrites. The interpreter records the raw read and its role before
MVP deduplication. Projection loads reset the consumed read chain; multiplies
extend it. Each vertex keeps the chain that actually produced it, so repeated
OPA/XLU camera reads and cached vertices bind their own consumed occurrence.
Guests supply no version numbers. Missing roles, duplicate roles or multiple
reads of a role within that accumulation are unresolved bindings. Separate V/P
must reproduce the consumed V·P, and M·(V·P) must reproduce the canonical MVP
with exact floating-point equality. Opaque VP binds as identity V plus VP in P's
place; any change to that VP holds the pair.

Effective Model comes directly from canonical stack operations. Optional
`MatrixPalette { base, parts }` declares consecutive 64-byte slots; `parts.len()`
is the slot count and each entry is its semantic part. Every successful model
read resolves the palette base using that read's segment table and binds its
slot. Each vertex retains its effective M and part, including vertices cached
from earlier scopes. A triangle can join different slots, such as torso 17 and
shoulder 10. A missing slot binding holds the source.

Policies are `Interpolate`, `StaticWorld`, `HoldModel`,
`BillboardHold(BillboardRecipe)`, `ScreenSpace` and `Hold`:

- Interpolate blends effective M using translation, scale and shortest-path
  quaternion rotation. For row lengths li and lj, the normalized-row dot tolerance
  is `E/li + E/lj + E²/(li*lj) + 8*f32::EPSILON`, where
  `E = sqrt(3)*2^-16`: truncating each signed 16.16 component introduces less than
  one step of error, so each row's error norm is bounded by E. Expanding the dot
  product of the measured rows around orthogonal true rows gives that bound;
  the final term covers floating-point roundoff. A bound of 1 or greater cannot
  constrain a normalized dot product, so it rejects the transform. Accepted
  rotations are orthonormalized with Gram–Schmidt and a cross product before quaternion
  extraction, rejecting a zero or nonfinite orthogonalization residual.
  Reflections, singular transforms and shear beyond that bound are rejected.
  Endpoints retain the original canonical matrices exactly. Unsupported M reports
  a per-draw DecompositionRejected restriction and keeps A's effective M under the phase camera. World triangles
  with XY/Z screen overrides report source-level UnsupportedTopology;
  ScreenSpace remains discrete.
- StaticWorld requires identity M and applies the phase camera.
- HoldModel keeps A's effective per-vertex M and applies phase V/P.
- BillboardHold interpolates the supplied world anchor, scale, local Z spin
  (radians, shortest angular path), and supplied basis (shortest-path rotation).
  The basis is the row-vector 3×3 of the guest's actual `billboardMtxF`, independent
  of consumed V. Materials and local geometry remain A's; triangle correspondence
  is unnecessary. A missing or generation-changed identity keeps A's recipe
  model under the phase camera with a per-draw NoMatch restriction. An
  incompatible B policy or recipe likewise falls back to HoldModel with a
  per-draw restriction. Other scopes continue interpolating; generation changes
  on StaticWorld, HoldModel, ScreenSpace and Hold need no correspondence.
- ScreenSpace keeps A's triangles. Hold keeps discrete non-world content; Hold
  under a declared gameplay camera makes the source ineligible. World content
  needs HoldModel or BillboardHold.

Billboard recipes have two multiplication orders, in row-vector notation.
`BillboardProvenance::Cpu` reconstructs `Rz * (basis * (S * T))`, then truncates
its entries to N64 signed 16.16, as a CPU-built model upload does.
`BillboardProvenance::Segment1` reconstructs
`fixed(basis) * fixed(Rz * (S * T))`, preserving the separate packed operands
of the asset-level segment-1 multiply. Here S is scale, T is world translation,
and fixed is signed 16.16 truncation. The supplied basis must be a proper
rotation (the quantization-aware orthogonality bound above and unit scale
tolerance 0.001). Each source recipe must reproduce every affected vertex's
consumed effective M with the same exact floating-point equality as MVP reconstruction; otherwise it reports
`UnsafeBillboard`. There is no correction inferred from V. A late camera rewrite
can therefore differ from the supplied basis without losing endpoint parity.
Other construction orders require a source hold.

Call native `Renderer::scanout(origin, visible_at, tasks, selected)` once per VI
swap, even when no task or annotation is available. `ScanoutOrigin::Vi` uses the
ordinary VI source resolver; `Host` uses the native framebuffer address resolver.
The host records `visible_at: std::time::Instant` at swap enqueue and carries its
producer sample separately. `selected` explicitly identifies one of `tasks` from
the same renderer. A no-swap task is left uncommitted.

The resolved scanout image is authoritative. A retained target is reused only if
no later write changed that target. Otherwise the render thread snapshots the
current image and submits the copy before publishing it. Missing, mixed, foreign
or stale task data never suppresses a real scanout; it breaks the interpolation
pair. An unrelated target write does not invalidate a deferred scanout. Snapshot
textures are pooled by extent and format and cannot be reused until both CPU
ownership and the present thread's GPU use have ended. Source-frame construction
is internal; hosts do not call `commit_source` or schedule phases themselves.

Internally, source ineligibility records `ReasonCount` entries, each containing
`reason: Ineligibility` and `count: u64`. Reasons include unresolved scopes,
unresolved or ambiguous bindings, unknown world triangles, unsafe billboard
recipes, unsupported topology, and the framebuffer/task reasons below. A triangle
with no scope that consumed a declared gameplay camera operand is an unknown
world draw. Binding and recipe counts count affected triangles once per reason;
scope counts count unresolved declarations; duplicate identities count each
additional declaration. Framebuffer counts count admission violations or
dependent operations; task-wide flags count once per task. Counts accumulate
across submitted tasks when committing a source. Every reason holds the source.

Source counts track submitted triangles, held triangles
(all policies except Interpolate and StaticWorld, including unscoped triangles),
texture rectangles and fills, whether the source is eligible or held. Rectangles
and fills stay in A's retained content. Counts describe submitted primitives,
including clipped or transparent geometry, rather than visible pixels.

Framebuffer admission rejects texture loads, copies, live aliases, inherited
colour/depth contents, CPU imports and multitask sources. It requires explicit
fills covering each attachment before use, with matching layout and depth/colour
role. Ordinary blending and depth reads after those fills are supported. Active
texture replacement providers and canonical tasks with diagnostics, dropped draws
or incomplete termination also hold. Multitask sources retain the selected
endpoint rather than replaying task history.

The present thread creates its independent `PhaseRenderer` lazily on the shared
device and queue. The canonical `Renderer` remains `!Send + !Sync`; native source
frames can cross threads. Phase state uses the current presentation format and
reduction factor. Its finite `0..=1` phase selects retained content, with `t == 1`
switching to B's discrete scene. Matching uses draw identity. M follows the scope's policy;
V blends entrywise. World camera validation is independent of draw matching:
matching world scopes pair their consumed cameras, including distinct camera
reads within a scope. A uniform B camera applies regardless of triangle count.
A scope missing from B can use the common world camera or an unambiguous camera
pair established by surviving scopes with the same consumed A V/P. Both sources
must provide resolved world V/P when A has world geometry. A camera that cannot
be paired unambiguously, or a changed P, holds the interval. Light and look-at
directions are folded again with M, including held models. Other vertex, material and
texture content remains A's until B.

Owned-surface endpoints reuse their immutable images. An ineligible scanout or
epoch break presents the newly scanned image directly and breaks the pair.
Changes in scale, dimensions or crop, and non-increasing visibility timestamps
also break it.
`PhaseResult::Interpolated { restrictions }` reports per-draw fallback, grouped by
`RestrictionReason`: NoMatch, TopologyChanged, PolicyChanged or
DecompositionRejected. Each `DrawRestriction` contains the affected scope count
and the first four `DrawIdentity` values in A's submission order. One scope counts
once even if it contains many triangles or palette parts; its first failed check
selects the reason. Successful phases with no fallback have an empty list.
Endpoint and Held results have no restrictions. Source ineligibility still holds
the whole interval. Scratch render failures are returned as `PhaseError`; phase
validation rejects NaN and out-of-range values.

Phase rendering uses a private `SceneRenderer`, reusable colour/depth attachments,
the retained Scene/Workload/RenderInputs pipeline, and the source's saved dither
context. Attachments are cleared per phase and reused while their layout, size
and scale match; phases blit directly from scratch without snapshot copies.
It runs at 1x as well as higher presentation scales. Scanout uses the
saved source scale, native crop and phase reduction factor through the normal
box-reduction/blit path. Final composition invokes render hooks on every
presented frame, including phases and plain snapshots. Hook command buffers run
before the final encoder. Phases do not advance canonical state, ownership,
write-back, frame serials or dither seeds.

Each phase renderer owns its scratch targets, pipeline variants, texture residency,
scanout resources and correspondence for one pair with weak source references.
Device and queue handles are cloned; source scenes, texture bytes and frozen
endpoint textures are immutable and reference-counted. There are no shared mutable
caches, profiling counters, serials or dither seeds. Queue submissions can interleave:
phases read only source resources and write only their own scratch and caller targets.
Canonical snapshots are submitted before their handles are returned.

Source handles own their selected task and endpoint; dropping them on either
thread releases that history, even while the phase renderer survives. Scratch
attachments are limited to the current retained scene's targets, and texture
residency uses its own bounded cache. Reset and reconfiguration discard scheduled
sources and phase state. The presenter retains up to three sources and a two-source
mailbox. Coalescing an overflow breaks continuity with the discarded history.
Hosts control the lifetime of any task handles they keep.

Surface configuration excludes all canonical submissions. This lock is not held
across acquisition, phase rendering or joining the present thread. Hosts using
cloned device/queue handles must stop external submissions during configuration
and surface recovery. Before external work on an owned surface, select Original
and acknowledge a scanout so asynchronous recovery has stopped; hook submissions
already run on the present thread.
