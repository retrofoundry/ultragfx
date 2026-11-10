use std::ops::Range;
use std::sync::{Arc, OnceLock};

use super::{buffer_init, downsample, lod_level_entries, sampling_entry};

#[derive(Clone)]
pub(crate) struct Compositor {
    present: wgpu::RenderPipeline,
    present_extent_layout: wgpu::BindGroupLayout,
    bind_group_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    dummy_view: wgpu::TextureView,
    image_sampling: wgpu::Buffer,
    downsample: Arc<OnceLock<downsample::BoxFilter>>,
}

#[derive(Clone)]
pub(crate) struct Scanout {
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) lease: Option<Arc<wgpu::Texture>>,
    pub(super) texture: wgpu::Texture,
    pub(super) height: u32,
    pub(super) scale: u32,
    pub(super) rows: Range<u32>,
    pub(super) bindings: wgpu::BindGroup,
    pub(super) extent: wgpu::BindGroup,
    pub(crate) reduction: u32,
}

impl Compositor {
    pub(super) fn new(
        present: wgpu::RenderPipeline,
        present_extent_layout: wgpu::BindGroupLayout,
        bind_group_layout: wgpu::BindGroupLayout,
        sampler: wgpu::Sampler,
        dummy_view: wgpu::TextureView,
        image_sampling: wgpu::Buffer,
    ) -> Self {
        Self {
            present,
            present_extent_layout,
            bind_group_layout,
            sampler,
            dummy_view,
            image_sampling,
            downsample: Default::default(),
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn replace_texture(
        &self,
        device: &wgpu::Device,
        image: &mut Scanout,
        texture: Arc<wgpu::Texture>,
    ) {
        image.texture = (*texture).clone();
        image.bindings =
            self.make_present_bindings(device, &texture.create_view(&Default::default()));
        image.lease = Some(texture);
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn snapshot(
        &self,
        device: &wgpu::Device,
        image: &super::isolated::TargetSnapshot,
        rows: Option<Range<u32>>,
        reduction: u32,
    ) -> Scanout {
        let rows = rows.unwrap_or(0..image.height);
        Scanout {
            texture: (*image.texture).clone(),
            lease: Some(image.texture.clone()),
            height: image.height,
            scale: image.scale,
            extent: self.make_present_extent(
                device,
                image.texture.width(),
                rows.start * image.scale..rows.end * image.scale,
                image.texture.height(),
                &Default::default(),
            ),
            bindings: self
                .make_present_bindings(device, &image.texture.create_view(&Default::default())),
            rows,
            reduction,
        }
    }

    pub(crate) fn scanout_profiled(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        image: &Scanout,
        fit: crate::PresentationFit,
        profiling: &crate::profiling::Recorder,
    ) {
        if image.reduction > 1 {
            self.scanout_texture(
                device,
                encoder,
                target,
                fit,
                profiling,
                (
                    &image.texture,
                    image.height,
                    image.scale,
                    Some(image.rows.clone()),
                    image.reduction,
                ),
            );
        } else {
            self.blit_to(encoder, target, &image.bindings, &image.extent, fit);
        }
    }

    fn blit_to(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        dst_view: &wgpu::TextureView,
        src_bg: &wgpu::BindGroup,
        extent_bg: &wgpu::BindGroup,
        fit: crate::PresentationFit,
    ) {
        let viewport = fit.viewport(dst_view.texture().width(), dst_view.texture().height());
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("present-pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: dst_view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: if viewport.is_some() {
                        wgpu::LoadOp::Clear(wgpu::Color::BLACK)
                    } else {
                        wgpu::LoadOp::Load
                    },
                    store: wgpu::StoreOp::Store,
                },
                depth_slice: None,
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&self.present);
        if let Some([x, y, width, height]) = viewport {
            pass.set_viewport(x as f32, y as f32, width as f32, height as f32, 0.0, 1.0);
        }
        pass.set_bind_group(0, src_bg, &[]);
        pass.set_bind_group(1, extent_bg, &[]);
        pass.draw(0..3, 0..1);
    }

    pub(super) fn make_present_extent(
        &self,
        device: &wgpu::Device,
        width: u32,
        rows: std::ops::Range<u32>,
        attachment_height: u32,
        profiling: &crate::profiling::Recorder,
    ) -> wgpu::BindGroup {
        let buffer = buffer_init(
            device,
            profiling,
            "presentation",
            &wgpu::util::BufferInitDescriptor {
                label: Some("present-extent"),
                contents: bytemuck::cast_slice(&[
                    (rows.end - rows.start) as f32 / attachment_height as f32,
                    rows.start as f32 / attachment_height as f32,
                    (width as f32 - 0.5) / width as f32,
                    (rows.end as f32 - 0.5) / attachment_height as f32,
                ]),
                usage: wgpu::BufferUsages::UNIFORM,
            },
        );
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("present-extent"),
            layout: &self.present_extent_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: buffer.as_entire_binding(),
            }],
        })
    }

    pub(super) fn make_present_bindings(
        &self,
        device: &wgpu::Device,
        sampled: &wgpu::TextureView,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("fb-store-present-bg"),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(sampled),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler), // Clamp/Linear
                },
                // TEXEL1 slot: the present pipeline never samples it (present.wgsl uses only
                // bindings 0/1) — bind the shared dummy to satisfy the shared group(0) layout.
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&self.dummy_view),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                // DETAIL slot (4/5): the present pipeline never samples it; shared dummy.
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::TextureView(&self.dummy_view),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ]
            .into_iter()
            .chain(lod_level_entries(&self.dummy_view))
            .chain([sampling_entry(&self.image_sampling)])
            .collect::<Vec<_>>(),
        })
    }

    pub(super) fn scanout_texture(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        fit: crate::PresentationFit,
        profiling: &crate::profiling::Recorder,
        (texture, height, scale, rows, reduction_factor): (
            &wgpu::Texture,
            u32,
            u32,
            Option<std::ops::Range<u32>>,
            u32,
        ),
    ) {
        let rows = rows.unwrap_or(0..height);
        let rows = rows.start * scale..rows.end.min(height) * scale;
        let reduced = (reduction_factor > 1).then(|| {
            self.downsample
                .get_or_init(|| downsample::BoxFilter::new(device))
                .reduce(device, encoder, texture, rows.clone(), reduction_factor)
        });
        let source = reduced.as_ref().unwrap_or(texture);
        let rows = if reduced.is_some() {
            0..source.height()
        } else {
            rows
        };
        let bindings = self.make_present_bindings(device, &source.create_view(&Default::default()));
        let extent =
            self.make_present_extent(device, source.width(), rows, source.height(), profiling);
        self.blit_to(encoder, target, &bindings, &extent, fit);
    }
}
