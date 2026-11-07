use super::{inputs::DrawInputs, OutVertex, TexturedPipeline, DEPTH_FORMAT};
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum ReplayLayout {
    StampsDelta,
    Stamps,
    Delta,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct Key {
    layout: ReplayLayout,
    write_depth: bool,
    cull: bool,
    test: bool,
    decal: bool,
    unclipped_depth: bool,
}

pub(super) struct Replay {
    shader: wgpu::ShaderModule,
    pipelines: HashMap<Key, wgpu::RenderPipeline>,
}

impl Replay {
    pub fn new(device: &wgpu::Device) -> Self {
        Self {
            shader: device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("companion-replay"),
                source: wgpu::ShaderSource::Wgsl(
                    format!(
                        "{}\n{}",
                        include_str!("combiner_prelude.wgsl"),
                        include_str!("replay.wgsl")
                    )
                    .into(),
                ),
            }),
            pipelines: Default::default(),
        }
    }

    pub fn pipeline(
        &mut self,
        device: &wgpu::Device,
        textured: &TexturedPipeline,
        draw: &DrawInputs,
        layout: ReplayLayout,
        write_depth: bool,
    ) -> &wgpu::RenderPipeline {
        let (cull, mode, unclipped_depth) = match draw {
            DrawInputs::Tris {
                cull,
                mode,
                unclipped_depth,
                ..
            } => (
                *cull == crate::scene::CullKind::Cull,
                mode,
                *unclipped_depth,
            ),
            DrawInputs::Rectangle { mode, .. } => (false, mode, false),
            _ => unreachable!("only raster operations replay"),
        };
        self.pipeline_for(
            device,
            textured,
            Key {
                layout,
                write_depth,
                cull,
                test: mode.z_test && !mode.read_depth && layout != ReplayLayout::Stamps,
                decal: mode.read_depth,
                unclipped_depth: unclipped_depth
                    && device
                        .features()
                        .contains(wgpu::Features::DEPTH_CLIP_CONTROL),
            },
        )
    }

    pub fn warm(&mut self, device: &wgpu::Device, textured: &TexturedPipeline) {
        // A cold pipeline compile mid-frame can trip the guest's RCP watchdog.
        let unclipped = device
            .features()
            .contains(wgpu::Features::DEPTH_CLIP_CONTROL);
        for layout in [
            ReplayLayout::StampsDelta,
            ReplayLayout::Stamps,
            ReplayLayout::Delta,
        ] {
            for bits in 0..32u8 {
                let flag = |bit: u8| bits & (1u8 << bit) != 0;
                let key = Key {
                    layout,
                    write_depth: flag(0),
                    cull: flag(1),
                    test: flag(2),
                    decal: flag(3),
                    unclipped_depth: flag(4),
                };
                if (key.unclipped_depth && !unclipped)
                    || (key.decal && key.test)
                    || (layout == ReplayLayout::Stamps && (key.write_depth || key.test))
                    || (layout == ReplayLayout::StampsDelta && key.decal && !key.write_depth)
                    || (layout == ReplayLayout::Delta && !key.write_depth)
                {
                    continue;
                }
                self.pipeline_for(device, textured, key);
            }
        }
    }

    #[cfg(test)]
    pub fn pipeline_count(&self) -> usize {
        self.pipelines.len()
    }

    fn pipeline_for(
        &mut self,
        device: &wgpu::Device,
        textured: &TexturedPipeline,
        key: Key,
    ) -> &wgpu::RenderPipeline {
        self.pipelines.entry(key).or_insert_with(|| {
            let mut groups = vec![
                Some(textured.bind_group_layout()),
                Some(textured.uniform_bind_group_layout()),
            ];
            if key.decal {
                groups.push(Some(textured.depth_bind_group_layout()));
            }
            let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("companion-replay"),
                bind_group_layouts: &groups,
                immediate_size: 0,
            });
            let stamps = Some(wgpu::ColorTargetState {
                format: wgpu::TextureFormat::Rg32Uint,
                blend: None,
                write_mask: wgpu::ColorWrites::RED
                    | (if key.write_depth {
                        wgpu::ColorWrites::GREEN
                    } else {
                        wgpu::ColorWrites::empty()
                    }),
            });
            let delta = Some(wgpu::ColorTargetState {
                format: wgpu::TextureFormat::R8Uint,
                blend: None,
                write_mask: if key.write_depth {
                    wgpu::ColorWrites::RED
                } else {
                    wgpu::ColorWrites::empty()
                },
            });
            let targets = match key.layout {
                ReplayLayout::StampsDelta => vec![stamps, delta],
                ReplayLayout::Stamps => vec![stamps],
                ReplayLayout::Delta => vec![delta],
            };
            let entry = match (key.layout, key.decal) {
                (ReplayLayout::StampsDelta, false) => "fs_replay",
                (ReplayLayout::StampsDelta, true) => "fs_replay_decal",
                (ReplayLayout::Stamps, false) => "fs_stamps",
                (ReplayLayout::Stamps, true) => "fs_stamps_decal",
                (ReplayLayout::Delta, false) => "fs_delta",
                (ReplayLayout::Delta, true) => "fs_delta_decal",
            };
            let descriptor = wgpu::RenderPipelineDescriptor {
                label: Some("companion-replay"),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &textured.vertex_shader,
                    entry_point: Some("vs_main"),
                    buffers: &[OutVertex::layout()],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &self.shader,
                    entry_point: Some(entry),
                    targets: &targets,
                    compilation_options: Default::default(),
                }),
                primitive: wgpu::PrimitiveState {
                    cull_mode: key.cull.then_some(wgpu::Face::Back),
                    ..Default::default()
                },
                depth_stencil: (key.layout != ReplayLayout::Stamps).then_some(
                    wgpu::DepthStencilState {
                        format: DEPTH_FORMAT,
                        depth_write_enabled: Some(key.write_depth),
                        depth_compare: Some(if key.test {
                            wgpu::CompareFunction::Less
                        } else {
                            wgpu::CompareFunction::Always
                        }),
                        stencil: Default::default(),
                        bias: Default::default(),
                    },
                ),
                multisample: Default::default(),
                multiview_mask: None,
                cache: None,
            };
            super::depth_clip_pipeline(device, &descriptor, key.unclipped_depth)
        })
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    #[test]
    fn triangle_delta_normalization_literals() {
        let (device, queue, _) = super::super::headless_device();
        let source = format!(
            "{}\n{}",
            include_str!("combiner_prelude.wgsl"),
            r#"
@vertex fn vs_test(@builtin(vertex_index) i:u32) -> @builtin(position) vec4<f32> {
    return vec4<f32>(f32((i<<1u)&2u)*2.0-1.0, 1.0-f32(i&2u)*2.0, 0.0,1.0);
}
@fragment fn fs_test(@builtin(position) p:vec4<f32>) -> @location(0) vec4<f32> {
    let slopes=array<vec2<f32>,10>(vec2<f32>(0,0),vec2<f32>(1,0),vec2<f32>(4,0),vec2<f32>(-4,0),vec2<f32>(16384,0),vec2<f32>(32768,0),vec2<f32>(65536,0),vec2<f32>(-65536,0),vec2<f32>(65540,0),vec2<f32>(-65540,0));
    let delta=triangle_delta(slopes[min(u32(p.x),9u)]);
    return vec4<f32>(f32(delta&255u)/255.0,f32(delta>>8u)/255.0,0.0,1.0);
}
"#
        );
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: None,
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: None,
            layout: None,
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_test"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_test"),
                targets: &[Some(wgpu::TextureFormat::Rgba8Unorm.into())],
                compilation_options: Default::default(),
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        let pixels = crate::tests::common::pixels_from_render(
            &device,
            &queue,
            64,
            1,
            wgpu::TextureFormat::Rgba8Unorm,
            |view| {
                let mut encoder = device.create_command_encoder(&Default::default());
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: None,
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                            store: wgpu::StoreOp::Store,
                        },
                        depth_slice: None,
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
                pass.set_pipeline(&pipeline);
                pass.draw(0..3, 0..1);
                drop(pass);
                queue.submit([encoder.finish()]);
            },
        );
        assert_eq!(
            &pixels.as_chunks::<4>().0[..10],
            &[
                [1u8, 0, 0, 255],
                [3, 0, 0, 255],
                [8, 0, 0, 255],
                [4, 0, 0, 255],
                [0, 128, 0, 255],
                [0, 128, 0, 255],
                [1, 0, 0, 255],
                [1, 0, 0, 255],
                [8, 0, 0, 255],
                [4, 0, 0, 255]
            ]
        );
    }
}
