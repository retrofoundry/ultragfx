use std::{collections::HashMap, ops::Range, sync::Arc};

use super::{
    framebuffers,
    workload::{TargetId, Workload},
    ImageLayout, SceneRenderer,
};

#[derive(Debug)]
pub(crate) struct TargetSnapshot {
    pub(super) texture: wgpu::Texture,
    pub(crate) layout: ImageLayout,
    pub(crate) height: u32,
    pub(crate) scale: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct IsolatedContext {
    pub(crate) frame_serial: u64,
    pub(crate) dither_seed: u32,
    pub(crate) render_scale: u32,
    pub(crate) legacy_extent: (u32, u32),
}

pub(crate) struct IsolatedRenderer {
    pub(super) renderer: SceneRenderer,
}

impl IsolatedRenderer {
    pub(crate) fn new(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        width: u32,
        height: u32,
        dual_source: bool,
    ) -> Self {
        Self {
            renderer: SceneRenderer::new(device, format, width, height, dual_source),
        }
    }

    pub(crate) fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        scene: &crate::Scene,
        context: IsolatedContext,
        target: TargetId,
    ) -> Result<TargetSnapshot, crate::Diagnostic> {
        let renderer = &mut self.renderer;
        if renderer.render_scale != context.render_scale {
            renderer.reset();
        }
        renderer.begin_frame();
        renderer.target_descriptors = Default::default();
        renderer.diagnostics.clear();
        renderer.frame_serial = context.frame_serial;
        renderer.dither_seed = context.dither_seed;
        renderer.render_scale = context.render_scale;
        renderer.resize(device, context.legacy_extent.0, context.legacy_extent.1);
        if let Some(mut inputs) = super::inputs::RenderInputs::with_targets(
            scene,
            context.legacy_extent,
            [context.frame_serial as u32, context.dither_seed],
            &renderer.target_descriptors,
        ) {
            renderer.framebuffers.retain(|id, fb| {
                inputs
                    .targets
                    .iter()
                    .find(|target| target.id == *id && !target.depth_clear)
                    .is_some_and(|target| {
                        fb.layout
                            == ImageLayout {
                                width: target.extents.native.0,
                                fmt: target.color_image.fmt,
                                siz: target.color_image.siz,
                            }
                            && fb.height == target.extents.native.1
                    })
            });
            renderer.depthbuffers.retain(|id, depth| {
                inputs
                    .targets
                    .iter()
                    .find(|target| target.depth_id() == Some(*id))
                    .is_some_and(|target| {
                        depth.layout.width == target.extents.native.0
                            && (depth.texture.width(), depth.texture.height())
                                == target.extents.native
                    })
            });
            renderer.preflight_inputs(
                &mut inputs,
                &device.limits(),
                false,
                crate::ClearPolicy::PerFrame,
            )?;
            renderer.render_prepared(device, queue, &inputs, crate::ClearPolicy::PerFrame);
        }
        if let Some(diagnostic) = renderer.diagnostics.first() {
            return Err(*diagnostic);
        }
        let fb = renderer
            .framebuffers
            .get(&target)
            .expect("an eligible source's selected target is rendered by its retained scene");
        Ok(TargetSnapshot {
            texture: fb
                .presentation
                .as_ref()
                .map_or(&fb.color, |p| &p.texture)
                .clone(),
            layout: fb.layout,
            height: fb.height,
            scale: context.render_scale,
        })
    }
}

impl SceneRenderer {
    pub(crate) fn has_replacement_provider(&self) -> bool {
        self.texture_resources.replacements.active.is_some()
    }

    pub(crate) fn isolated_context(&self) -> IsolatedContext {
        IsolatedContext {
            frame_serial: self.frame_serial,
            dither_seed: self.dither_seed,
            render_scale: self.render_scale,
            legacy_extent: (self.fb_w, self.fb_h),
        }
    }

    pub(crate) fn snapshot_targets(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        scene: &crate::Scene,
    ) -> HashMap<TargetId, Arc<TargetSnapshot>> {
        let targets: std::collections::HashSet<_> = Workload::new(scene)
            .targets
            .into_iter()
            .filter(|target| !target.depth_clear)
            .map(|target| target.id)
            .collect();
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("endpoint-snapshots"),
        });
        let snapshots = self
            .framebuffers
            .iter()
            .filter(|(id, _)| targets.contains(id))
            .map(|(&id, fb)| {
                let source = fb.presentation.as_ref().map_or(&fb.color, |p| &p.texture);
                let texture = device.create_texture(&framebuffers::color_descriptor(
                    "endpoint-snapshot",
                    (source.width(), source.height()),
                ));
                encoder.copy_texture_to_texture(
                    source.as_image_copy(),
                    texture.as_image_copy(),
                    source.size(),
                );
                (
                    id,
                    Arc::new(TargetSnapshot {
                        texture,
                        layout: fb.layout,
                        height: fb.height,
                        scale: self.render_scale,
                    }),
                )
            })
            .collect();
        queue.submit([encoder.finish()]);
        snapshots
    }

    pub(crate) fn scanout_snapshot(
        &self,
        image: &TargetSnapshot,
        rows: Option<Range<u32>>,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
    ) {
        self.scanout_texture(
            device,
            encoder,
            target,
            (&image.texture, image.height, image.scale, rows),
        );
    }
}
