use super::{coverage_texture, Readback};
use crate::render::{
    decal::ReplayLayout,
    framebuffers::{clear_image, DepthImage, ImageLayout},
    inputs::{DrawInputs, DrawMode},
    CombinerUniform, OutVertex, SceneRenderer,
};
use wgpu::util::DeviceExt;

const SIZE: u32 = 16;

struct Case {
    z: [f32; 3],
    w: [f32; 3],
    write_depth: bool,
    mode: DrawMode,
    cull: crate::hle::CullKind,
    reverse: bool,
    unclipped: bool,
    scissor: (u32, u32, u32, u32),
    uniform: CombinerUniform,
}

impl Default for Case {
    fn default() -> Self {
        let mut uniform = CombinerUniform::fill_rect(0xffffffff, 3);
        uniform.fill_siz = 0;
        uniform.is_rect = 0;
        uniform.frame = [17, 12345, SIZE, SIZE];
        uniform.raster = [1, SIZE, 0, 0];
        Self {
            z: [0.5; 3],
            w: [1.0; 3],
            write_depth: true,
            mode: DrawMode {
                z_test: true,
                z_write: true,
                ..Default::default()
            },
            cull: crate::hle::CullKind::None,
            reverse: false,
            unclipped: false,
            scissor: (0, 0, SIZE, SIZE),
            uniform,
        }
    }
}

struct Probe {
    device: wgpu::Device,
    queue: wgpu::Queue,
    renderer: SceneRenderer,
    depth: DepthImage,
    presentation_depth: DepthImage,
    color: wgpu::Texture,
    stamps: wgpu::Texture,
    expected_stamps: Vec<[u32; 2]>,
    slot: u32,
    has_depth: bool,
}

struct Planes {
    stamps: Vec<u8>,
    depth: Vec<u8>,
    delta: Vec<u8>,
}

impl Probe {
    fn new(fallback: bool, has_depth: bool) -> Self {
        let (device, queue) = if fallback {
            crate::render::headless_device_forced_fallback()
        } else {
            let (device, queue, _) = crate::render::headless_device();
            (device, queue)
        };
        let renderer =
            SceneRenderer::new(&device, wgpu::TextureFormat::Rgba8Unorm, SIZE, SIZE, false);
        let layout = ImageLayout {
            width: SIZE,
            fmt: 0,
            siz: 2,
        };
        let depth = DepthImage::new(&device, layout, (SIZE, SIZE));
        let presentation_depth = DepthImage::new(&device, layout, (SIZE, SIZE));
        let color = device.create_texture(&crate::render::framebuffers::color_descriptor(
            "replay-test-color",
            (SIZE, SIZE),
        ));
        let stamps = coverage_texture(&device, SIZE, SIZE);
        let mut encoder = device.create_command_encoder(&Default::default());
        clear_image(&mut encoder, &depth);
        clear_image(&mut encoder, &presentation_depth);
        for texture in [&color, &stamps] {
            let view = texture.create_view(&Default::default());
            crate::render::workload::clear_color(
                &mut encoder,
                &view,
                wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
            );
        }
        queue.submit([encoder.finish()]);
        Self {
            device,
            queue,
            renderer,
            depth,
            presentation_depth,
            color,
            stamps,
            expected_stamps: vec![[0; 2]; (SIZE * SIZE) as usize],
            slot: 0,
            has_depth,
        }
    }

    fn step(&mut self, mut case: Case) -> Planes {
        self.slot += 1;
        case.uniform.coverage_slot = self.slot;
        case.mode.z_write = case.write_depth;
        let draw = DrawInputs::Tris {
            cull: case.cull,
            unclipped_depth: case.unclipped,
            index_start: 0,
            index_count: 3,
            material_index: 0,
            mode: case.mode,
        };
        let mut vertices: Vec<_> = [(-1.0, -1.0), (3.0, -1.0), (-1.0, 3.0)]
            .into_iter()
            .zip(case.z)
            .zip(case.w)
            .map(|(((x, y), z), w)| OutVertex {
                position: [x * w, y * w, z * w, w],
                color: [1.0; 4],
                uv: [0.0; 2],
                _pad: [0.0; 2],
            })
            .collect();
        if case.reverse {
            vertices.swap(1, 2);
        }
        let vertices = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: None,
                contents: bytemuck::cast_slice(&vertices),
                usage: wgpu::BufferUsages::VERTEX,
            });
        let uniform = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: None,
                contents: bytemuck::bytes_of(&case.uniform),
                usage: wgpu::BufferUsages::UNIFORM,
            });
        let uniforms = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: self.renderer.textured_fb.uniform_bind_group_layout(),
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform.as_entire_binding(),
            }],
        });
        let mut encoder = self.device.create_command_encoder(&Default::default());
        let snapshot = DepthImage::snapshot(&self.device, self.depth.layout, (SIZE, SIZE));
        if case.mode.read_depth {
            self.renderer
                .depth_pipeline
                .copy(&self.device, &mut encoder, &self.depth, &snapshot);
        }
        let depth_bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: self.renderer.textured_fb.depth_bind_group_layout(),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&snapshot.attach),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&snapshot.delta_view),
                },
            ],
        });
        let attached = self.has_depth && (!case.mode.read_depth || case.write_depth);
        let unclipped = usize::from(
            case.unclipped
                && self
                    .device
                    .features()
                    .contains(wgpu::Features::DEPTH_CLIP_CONTROL),
        );
        let real = if case.mode.read_depth {
            self.renderer.textured_fb.select_decal(
                case.cull,
                case.mode.fallback_class,
                case.mode.blend_class,
                case.write_depth,
            )
        } else if let Some(dual) = &self.renderer.textured_fb.dual {
            dual.select(
                case.cull,
                self.has_depth && case.mode.z_test,
                self.has_depth && case.write_depth,
                self.has_depth,
            )
        } else {
            self.renderer.textured_fb.select(
                case.cull,
                self.has_depth && case.mode.z_test,
                self.has_depth && case.write_depth,
                self.has_depth,
                case.mode.fallback_class,
            )
        }[unclipped]
            .clone();
        let layout = if attached {
            ReplayLayout::StampsDelta
        } else {
            ReplayLayout::Stamps
        };
        let replay = self
            .renderer
            .replay_pipeline
            .pipeline(
                &self.device,
                &self.renderer.textured_fb,
                &draw,
                layout,
                case.write_depth,
            )
            .clone();
        let delta = case.write_depth.then(|| {
            self.renderer
                .replay_pipeline
                .pipeline(
                    &self.device,
                    &self.renderer.textured_fb,
                    &draw,
                    ReplayLayout::Delta,
                    true,
                )
                .clone()
        });
        let view = self.stamps.create_view(&Default::default());
        let color = self.color.create_view(&Default::default());
        crate::render::workload::clear_color(
            &mut encoder,
            &color,
            wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
        );
        let mut passes = vec![(&real, vec![&color], attached.then_some(&self.depth.attach))];
        if let Some(delta) = &delta {
            passes.push((
                delta,
                vec![&self.presentation_depth.delta_view],
                self.presentation_depth.delta_depth.as_ref(),
            ));
        }
        passes.push((
            &replay,
            if attached {
                vec![&view, &self.depth.delta_view]
            } else {
                vec![&view]
            },
            attached.then(|| self.depth.delta_depth.as_ref().unwrap()),
        ));
        for (pipeline, colors, depth) in passes {
            let attachments: Vec<_> = colors
                .into_iter()
                .map(|view| {
                    Some(wgpu::RenderPassColorAttachment {
                        view,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Load,
                            store: wgpu::StoreOp::Store,
                        },
                        depth_slice: None,
                    })
                })
                .collect();
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &attachments,
                depth_stencil_attachment: depth.map(|view| {
                    wgpu::RenderPassDepthStencilAttachment {
                        view,
                        depth_ops: Some(wgpu::Operations {
                            load: wgpu::LoadOp::Load,
                            store: wgpu::StoreOp::Store,
                        }),
                        stencil_ops: None,
                    }
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(pipeline);
            pass.set_vertex_buffer(0, vertices.slice(..));
            pass.set_bind_group(0, &self.renderer.fill_bind_group, &[]);
            pass.set_bind_group(1, &uniforms, &[0]);
            if case.mode.read_depth {
                pass.set_bind_group(2, &depth_bg, &[]);
            }
            let (x, y, w, h) = case.scissor;
            pass.set_scissor_rect(x, y, w, h);
            pass.draw(0..3, 0..1);
        }
        let readbacks: Vec<_> = [
            (&self.stamps, 8),
            (&self.color, 4),
            (&self.depth.texture, 4),
            (self.depth.delta_depth.as_ref().unwrap().texture(), 4),
            (
                self.presentation_depth
                    .delta_depth
                    .as_ref()
                    .unwrap()
                    .texture(),
                4,
            ),
            (self.depth.delta_view.texture(), 1),
            (self.presentation_depth.delta_view.texture(), 1),
        ]
        .into_iter()
        .map(|(texture, size)| Readback::new(&self.device, &mut encoder, texture, size))
        .collect();
        self.queue.submit([encoder.finish()]);
        let mut planes = readbacks.into_iter().map(|r| r.read(&self.device));
        let stamps = planes.next().unwrap();
        let color = planes.next().unwrap();
        for (expected, rgba) in self
            .expected_stamps
            .iter_mut()
            .zip(color.as_chunks::<4>().0)
        {
            if rgba[0] != 0 {
                expected[0] = self.slot;
                if self.has_depth && case.write_depth {
                    expected[1] = self.slot;
                }
            }
        }
        assert_eq!(
            slots(&stamps),
            self.expected_stamps,
            "stamps at slot {}",
            self.slot
        );
        let depth = planes.next().unwrap();
        assert_eq!(
            depth,
            planes.next().unwrap(),
            "companion depth at slot {}",
            self.slot
        );
        assert_eq!(
            depth,
            planes.next().unwrap(),
            "presentation companion depth at slot {}",
            self.slot
        );
        let delta = planes.next().unwrap();
        assert_eq!(delta, planes.next().unwrap(), "delta at slot {}", self.slot);
        Planes {
            stamps,
            depth,
            delta,
        }
    }
}

fn slots(bytes: &[u8]) -> Vec<[u32; 2]> {
    bytes
        .as_chunks::<8>()
        .0
        .iter()
        .map(|p| {
            [
                u32::from_ne_bytes(p[..4].try_into().unwrap()),
                u32::from_ne_bytes(p[4..].try_into().unwrap()),
            ]
        })
        .collect()
}

#[test]
fn replay_preserves_depth_when_depth_writes_are_disabled() {
    for fallback in [false, true] {
        let mut probe = Probe::new(fallback, true);
        for (write_depth, expected) in [(false, [1, 0]), (true, [2, 2]), (false, [3, 2])] {
            let planes = probe.step(Case {
                z: [0.8 - probe.slot as f32 * 0.2; 3],
                write_depth,
                mode: DrawMode::default(),
                ..Default::default()
            });
            assert!(slots(&planes.stamps).iter().all(|&p| p == expected));
        }
    }
}

#[test]
fn replay_matches_color_at_coplanar_and_adjacent_depths() {
    for fallback in [false, true] {
        for unclipped in [false, true] {
            let mut probe = Probe::new(fallback, true);
            let z = 0.5f32;
            let cases = [
                z,
                z,
                z.next_up(),
                z.next_down(),
                z,
                z.next_down().next_down(),
            ];
            let expected = [1, 1, 1, 4, 4, 6];
            for (z, expected) in cases.into_iter().zip(expected) {
                let mut case = Case {
                    z: [z; 3],
                    unclipped,
                    ..Default::default()
                };
                case.uniform.inv_tex_size[3] = f32::from(unclipped);
                let planes = probe.step(case);
                assert!(slots(&planes.stamps).iter().all(|&p| p == [expected; 2]));
            }
        }
    }
}

#[test]
fn replay_matches_color_acceptance_on_isolated_operations() {
    for fallback in [false, true] {
        let mut probe = Probe::new(fallback, true);
        for index in 0..24 {
            let mut case = Case {
                mode: DrawMode::default(),
                ..Default::default()
            };
            match index {
                0 => case.scissor = (3, 4, 7, 5),
                1 | 2 => {
                    case.cull = crate::hle::CullKind::Cull;
                    case.reverse = index == 2;
                }
                3..=5 => {
                    case.mode.z_test = true;
                    case.z = [[0.4; 3], [0.6; 3], [0.4; 3]][index - 3];
                }
                6 => {
                    case.write_depth = false;
                    case.mode.z_test = true;
                    case.z = [0.3; 3];
                }
                7..=10 => {
                    case.uniform.alpha_flags = 1;
                    case.uniform.alpha_threshold = 0.5;
                    case.uniform.prim[3] = [0.0, 0.5, 0.49, 1.0][index - 7];
                }
                11 => {
                    case.uniform.alpha_flags = 3;
                    case.uniform.prim[3] = 0.5;
                }
                12..=14 => {
                    case.uniform.alpha_flags = 4;
                    case.uniform.prim[3] = [0.124, 0.125, 0.126][index - 12];
                }
                15 => {
                    case.uniform.primitive_depth = [1, 0x2345, 32, 0];
                }
                16 => {
                    case.z = [0.4, 0.45, 0.5];
                }
                17..=19 => {
                    case.mode.read_depth = true;
                    case.write_depth = false;
                    case.z = [[0.45; 3], [0.99; 3], [0.0; 3]][index - 17];
                }
                20 | 21 => {
                    case.mode.read_depth = true;
                    case.z = [0.45; 3];
                }
                22 => {
                    case.write_depth = false;
                    case.z = [0.1; 3];
                }
                23 => {
                    case.mode.z_test = true;
                    case.z = [0.2; 3];
                }
                _ => unreachable!(),
            }
            let planes = probe.step(case);
            let accepted = slots(&planes.stamps)
                .iter()
                .filter(|p| p[0] == probe.slot)
                .count();
            match index {
                0 => assert_eq!(accepted, 35),
                4 | 5 | 7 | 9 | 12 => assert_eq!(accepted, 0, "case {index}"),
                11 => assert!(accepted > 0 && accepted < (SIZE * SIZE) as usize),
                3 | 6 | 8 | 10 | 13..=16 | 22 | 23 => {
                    assert_eq!(accepted, (SIZE * SIZE) as usize, "case {index}")
                }
                _ => {}
            }
        }
        let mut probe = Probe::new(fallback, false);
        let planes = probe.step(Case {
            write_depth: false,
            ..Default::default()
        });
        assert!(slots(&planes.stamps).iter().all(|&p| p == [1, 0]));
        assert!(planes.delta.iter().all(|&d| d == 0));
        assert!(planes
            .depth
            .as_chunks::<4>()
            .0
            .iter()
            .all(|&z| f32::from_ne_bytes(z) == 1.0));
    }
}

#[test]
fn replay_matches_color_at_perspective_depth_ties() {
    for fallback in [false, true] {
        for unclipped in [false, true] {
            let mut probe = Probe::new(fallback, true);
            let z = f32::from_bits(0x3f6a6a7e);
            for z in [
                z,
                z,
                z.next_up(),
                z.next_down(),
                z,
                z.next_down().next_down(),
            ] {
                let mut case = Case {
                    z: [z; 3],
                    w: [1.234567, 12.34567, 0.01234567],
                    unclipped,
                    ..Default::default()
                };
                case.uniform.inv_tex_size[3] = f32::from(unclipped);
                let planes = probe.step(case);
                assert!(slots(&planes.stamps).iter().any(|p| p[0] != 0));
            }
        }
    }
}
