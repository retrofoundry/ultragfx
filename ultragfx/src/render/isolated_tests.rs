use super::*;
use crate::scene::{ColorImage, FramebufferPair, Rect, SceneOp, Scissor};
use crate::tests::common;
use crate::ClearPolicy;

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const TARGET: workload::TargetId = workload::TargetId::Guest(0x1000);

fn fill(word: u32) -> crate::Scene {
    crate::Scene {
        framebuffer_pairs: vec![FramebufferPair {
            color_image: ColorImage {
                addr: 0x1000,
                width: 4,
                fmt: 0,
                siz: 2,
            },
            active_scissor: Scissor {
                lrx: 4,
                lry: 3,
                ..Default::default()
            },
            size_extent: (4, 3),
            ops: vec![SceneOp::FillRect {
                rect: Rect {
                    lrx: 3,
                    lry: 2,
                    ..Default::default()
                },
                color_raw: word,
                convert: [0; 6],
                key: Default::default(),
            }],
            ..Default::default()
        }],
        ..Default::default()
    }
}

fn pixels(
    renderer: &SceneRenderer,
    image: &isolated::TargetSnapshot,
    rows: Option<std::ops::Range<u32>>,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    extent: (u32, u32),
) -> Vec<u8> {
    common::pixels_from_render(device, queue, extent.0, extent.1, FORMAT, |target| {
        let mut encoder = device.create_command_encoder(&Default::default());
        renderer.scanout_snapshot(image, rows, device, &mut encoder, target);
        queue.submit([encoder.finish()]);
    })
}

#[test]
fn endpoint_snapshot_keeps_pixels_and_scale_after_address_reuse() {
    let (device, queue, dual) = headless_device();
    let mut renderer = SceneRenderer::new(&device, FORMAT, 4, 3, dual);
    renderer.set_test_scale(3, 256 * 1024 * 1024);
    let mut unrelated = fill(0x07c107c1);
    unrelated.framebuffer_pairs[0].color_image.addr = 0x2000;
    renderer.render_into_store(&device, &queue, &unrelated, ClearPolicy::Persist);
    let scene = fill(0x07c107c1);
    renderer.render_into_store(&device, &queue, &scene, ClearPolicy::Persist);
    let source = &renderer.framebuffers[&TARGET]
        .presentation
        .as_ref()
        .unwrap()
        .texture;
    let bytes: Vec<_> = (0..9)
        .flat_map(|y| {
            (0..12).flat_map(move |_| {
                if y < 6 {
                    [0, 255, 0, 255]
                } else {
                    [0, 0, 255, 255]
                }
            })
        })
        .collect();
    queue.write_texture(
        source.as_image_copy(),
        &bytes,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(48),
            rows_per_image: None,
        },
        source.size(),
    );
    let snapshots = renderer.snapshot_targets(&device, &queue, &scene);
    assert_eq!(snapshots.len(), 1);
    let image = &snapshots[&TARGET];
    assert_eq!((image.layout.width, image.height, image.scale), (4, 3, 3));
    renderer.render_into_store(&device, &queue, &fill(0xf801f801), ClearPolicy::Persist);
    renderer.change_scale(&device, &queue, 1).unwrap();
    for reduction in [1, 3] {
        renderer.reduction_factor = reduction;
        let extent = (12 / reduction, 6 / reduction);
        let actual = pixels(&renderer, image, Some(1..3), &device, &queue, extent);
        let expected: Vec<_> = (0..extent.1)
            .flat_map(|y| {
                (0..extent.0).flat_map(move |_| {
                    if y < extent.1 / 2 {
                        [0, 255, 0, 255]
                    } else {
                        [0, 0, 255, 255]
                    }
                })
            })
            .collect();
        assert_eq!(actual, expected);
    }
}

#[test]
fn isolated_render_reconstructs_endpoints_without_inherited_pixels() {
    let (device, queue, dual) = headless_device();
    let mut canonical = SceneRenderer::new(&device, FORMAT, 4, 3, dual);
    let mut isolated = isolated::IsolatedRenderer::new(&device, FORMAT, 4, 3, dual);
    for scale in [1, 3] {
        canonical.change_scale(&device, &queue, scale).unwrap();
        canonical.frame_serial = 13;
        canonical.dither_seed = 731;
        let scene = fill(0x07c107c1);
        canonical.render_into_store(&device, &queue, &scene, ClearPolicy::Persist);
        let descriptors = canonical.target_descriptors.clone();
        let context = canonical.isolated_context();
        let endpoint = canonical.snapshot_targets(&device, &queue, &scene);
        let expected = pixels(
            &canonical,
            &endpoint[&TARGET],
            None,
            &device,
            &queue,
            (4 * scale, 3 * scale),
        );
        let first = isolated
            .render(&device, &queue, &scene, context, TARGET)
            .unwrap();
        assert_eq!(
            pixels(
                &canonical,
                &first,
                None,
                &device,
                &queue,
                (4 * scale, 3 * scale)
            ),
            expected
        );
        let mut partial = fill(0xf801f801);
        let SceneOp::FillRect { rect, .. } = &mut partial.framebuffer_pairs[0].ops[0] else {
            unreachable!()
        };
        rect.lrx = 0;
        let middle = isolated
            .render(&device, &queue, &partial, context, TARGET)
            .unwrap();
        let middle = pixels(
            &canonical,
            &middle,
            None,
            &device,
            &queue,
            (4 * scale, 3 * scale),
        );
        assert_eq!(&middle[..4], &[255, 0, 0, 255]);
        assert_ne!(
            &middle[(3 * scale * 4) as usize..(3 * scale * 4 + 4) as usize],
            &[0, 255, 0, 255]
        );
        let repeated = isolated
            .render(&device, &queue, &scene, context, TARGET)
            .unwrap();
        assert_eq!(
            pixels(
                &canonical,
                &repeated,
                None,
                &device,
                &queue,
                (4 * scale, 3 * scale)
            ),
            expected
        );
        assert_eq!(canonical.target_descriptors, descriptors);
        assert_eq!(canonical.frame_serial, 13);
        assert_eq!(canonical.dither_seed, 731);
        assert_eq!(
            pixels(
                &canonical,
                &endpoint[&TARGET],
                None,
                &device,
                &queue,
                (4 * scale, 3 * scale)
            ),
            expected
        );
    }
}

#[test]
fn isolated_render_latches_alpha_dither_context() {
    use n64_gbi::encode::*;

    let pass = |zero, output| CcPass {
        a: zero,
        b: zero,
        c: zero,
        d: output,
    };
    let mut commands = vec![
        gdp_set_color_image(0, 2, 4, 0x1000),
        gdp_set_scissor(0, 0, 0, 16, 12),
        gdp_set_cycle_type(0),
        gdp_set_render_mode(3, 0),
        gdp_set_combine_lerp(
            pass(ZERO_C, 3),
            pass(ZERO_A, 3),
            pass(ZERO_C, 3),
            pass(ZERO_A, 3),
        ),
        gdp_set_prim_color(0, 0, 0xff000080),
    ];
    commands.extend(gsp_texture_rectangle(
        0, 0, 16, 12, 0, 0, 0, 1024, 1024, false,
    ));
    commands.push(gsp_enddl());
    let bytes: Vec<_> = commands
        .into_iter()
        .flat_map(|(a, b)| [a.to_be_bytes(), b.to_be_bytes()].concat())
        .collect();
    let interpreted = crate::hle::interpret_rdram(&bytes, 0);
    assert!(interpreted.diags.is_empty(), "{:?}", interpreted.diags);
    let mut scene = interpreted.scene;
    scene.framebuffer_pairs[0]
        .ops
        .insert(0, fill(0x07c107c1).framebuffer_pairs[0].ops.remove(0));
    let (device, queue, dual) = headless_device();
    let mut canonical = SceneRenderer::new(&device, FORMAT, 4, 3, dual);
    canonical.frame_serial = 13;
    canonical.dither_seed = 731;
    canonical.render_into_store(&device, &queue, &scene, ClearPolicy::Persist);
    let expected = canonical.snapshot_targets(&device, &queue, &scene);
    let expected = pixels(
        &canonical,
        &expected[&TARGET],
        None,
        &device,
        &queue,
        (4, 3),
    );
    let mut isolated = isolated::IsolatedRenderer::new(&device, FORMAT, 4, 3, dual);
    let actual = isolated
        .render(
            &device,
            &queue,
            &scene,
            canonical.isolated_context(),
            TARGET,
        )
        .unwrap();
    assert_eq!(
        pixels(&canonical, &actual, None, &device, &queue, (4, 3)),
        expected
    );
    assert!(expected.as_chunks::<4>().0.contains(&[255, 0, 0, 128]));
    assert!(expected.as_chunks::<4>().0.contains(&[0, 255, 0, 255]));
}

#[test]
fn steady_phases_reuse_color_depth_and_presentation_attachments() {
    let (device, queue, dual) = headless_device();
    let mut isolated = isolated::IsolatedRenderer::new(&device, FORMAT, 4, 3, dual);
    isolated.renderer.profiling = crate::profiling::Recorder::new(crate::profiling::Mode::Counters);
    let mut scene = fill(0x07c107c1);
    scene.framebuffer_pairs[0].depth_image = Some(0x2000);
    let mut depth = scene.framebuffer_pairs[0].clone();
    depth.color_image.addr = 0x2000;
    depth.is_depth_clear = true;
    depth.size_extent.1 = 4;
    depth.active_scissor.lry = 4;
    let SceneOp::FillRect { rect, .. } = &mut depth.ops[0] else {
        unreachable!()
    };
    rect.lry = 3;
    scene.framebuffer_pairs.insert(0, depth);
    for scale in [1, 2] {
        let context = isolated::IsolatedContext {
            frame_serial: 3,
            dither_seed: 7,
            render_scale: scale,
            legacy_extent: (4, 3),
        };
        let first = isolated
            .render(&device, &queue, &scene, context, TARGET)
            .unwrap();
        let color = isolated.renderer.framebuffers[&TARGET].color.clone();
        let depth = isolated.renderer.depthbuffers[&workload::TargetId::Guest(0x2000)].clone();
        assert_eq!(first.height, 3);
        assert_eq!(depth.texture.height(), 4);
        isolated.renderer.profiling.drain();
        for _ in 0..3 {
            let image = isolated
                .render(&device, &queue, &scene, context, TARGET)
                .unwrap();
            assert_eq!(image.texture, first.texture);
            assert_eq!(isolated.renderer.framebuffers[&TARGET].color, color);
            let current = &isolated.renderer.depthbuffers[&workload::TargetId::Guest(0x2000)];
            assert_eq!(current.texture, depth.texture);
            assert_eq!(current.delta_view, depth.delta_view);
            assert_eq!(current.delta_depth, depth.delta_depth);
            assert_eq!(
                current.presentation.as_ref().map(|p| &p.texture),
                depth.presentation.as_ref().map(|p| &p.texture)
            );
        }
        let counters = isolated.renderer.profiling.drain().counters;
        assert!(
            !counters
                .iter()
                .any(|(name, &count)| name.starts_with("texture.")
                    && name.ends_with(".creations")
                    && count != 0),
            "{counters:?}"
        );
    }
}

#[test]
fn scratch_attachments_follow_layout_extent_and_target_changes() {
    let (device, queue, dual) = headless_device();
    let mut isolated = isolated::IsolatedRenderer::new(&device, FORMAT, 4, 3, dual);
    let context = isolated::IsolatedContext {
        frame_serial: 3,
        dither_seed: 7,
        render_scale: 1,
        legacy_extent: (4, 3),
    };
    for (width, height, siz, address) in [
        (4u32, 3, 2, 0x1000),
        (8, 3, 2, 0x1000),
        (8, 2, 2, 0x1000),
        (8, 2, 3, 0x1000),
        (4, 3, 2, 0x3000),
    ] {
        let mut scene = fill(if siz == 2 { 0xf801f801 } else { 0xff0000ff });
        let pair = &mut scene.framebuffer_pairs[0];
        pair.color_image.width = width as u16;
        pair.color_image.siz = siz;
        pair.color_image.addr = address;
        pair.depth_image = Some(0x2000);
        pair.size_extent = (width, height);
        pair.active_scissor.lrx = width as i32;
        pair.active_scissor.lry = height as i32;
        let SceneOp::FillRect { rect, .. } = &mut pair.ops[0] else {
            unreachable!()
        };
        rect.lrx = width as i32 - 1;
        rect.lry = height as i32 - 1;
        let mut depth = pair.clone();
        depth.color_image.addr = 0x2000;
        depth.color_image.siz = 2;
        depth.is_depth_clear = true;
        scene.framebuffer_pairs.insert(0, depth);
        let target = workload::TargetId::Guest(address);
        let image = isolated
            .render(&device, &queue, &scene, context, target)
            .unwrap();
        assert_eq!((image.layout.width, image.height), (width, height));
        assert_eq!(
            pixels(
                &isolated.renderer,
                &image,
                None,
                &device,
                &queue,
                (width, height)
            ),
            [255, 0, 0, 255].repeat((width * height) as usize)
        );
        assert_eq!(isolated.renderer.framebuffers.len(), 1);
        let current_depth = &isolated.renderer.depthbuffers[&workload::TargetId::Guest(0x2000)];
        assert_eq!(
            (
                current_depth.texture.width(),
                current_depth.texture.height()
            ),
            (width, height)
        );
    }
}

pub(in crate::render) fn buffer_bytes(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    source: &wgpu::Buffer,
) -> Vec<u8> {
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: source.size(),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.copy_buffer_to_buffer(source, 0, &readback, 0, source.size());
    queue.submit([encoder.finish()]);
    let (tx, rx) = std::sync::mpsc::channel();
    readback
        .slice(..)
        .map_async(wgpu::MapMode::Read, move |result| {
            tx.send(result).unwrap();
        });
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    rx.recv().unwrap().unwrap();
    readback.slice(..).get_mapped_range().to_vec()
}

pub(in crate::render) fn texture_bytes(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
) -> Vec<u8> {
    let (declaration, value) = match texture.format() {
        wgpu::TextureFormat::Depth32Float => {
            ("texture_depth_2d", "bitcast<u32>(textureLoad(image,c,0))")
        }
        wgpu::TextureFormat::R8Uint => ("texture_2d<u32>", "textureLoad(image,c,0).r"),
        wgpu::TextureFormat::Rgba8Unorm => ("texture_2d<f32>", ""),
        other => panic!("unexpected plane {other:?}"),
    };
    let output = if value.is_empty() {
        "return textureLoad(image,c,0);".to_owned()
    } else {
        format!("let w={value}; return vec4<f32>(f32(w&255u),f32((w>>8u)&255u),f32((w>>16u)&255u),f32(w>>24u))/255.0;")
    };
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: None,
        source: wgpu::ShaderSource::Wgsl(
            format!(
                "@group(0) @binding(0) var image: {declaration};
@vertex fn vs(@builtin(vertex_index) i:u32) -> @builtin(position) vec4<f32> {{
    return vec4<f32>(f32((i<<1u)&2u)*2.0-1.0,1.0-f32(i&2u)*2.0,0.0,1.0);
}}
@fragment fn fs(@builtin(position) p:vec4<f32>) -> @location(0) vec4<f32> {{
    let c=vec2<i32>(p.xy); {output}
}}"
            )
            .into(),
        ),
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: None,
        layout: None,
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs"),
            buffers: &[],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs"),
            targets: &[Some(FORMAT.into())],
            compilation_options: Default::default(),
        }),
        primitive: Default::default(),
        depth_stencil: None,
        multisample: Default::default(),
        multiview_mask: None,
        cache: None,
    });
    let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &pipeline.get_bind_group_layout(0),
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: wgpu::BindingResource::TextureView(&texture.create_view(&Default::default())),
        }],
    });
    common::pixels_from_render(
        device,
        queue,
        texture.width(),
        texture.height(),
        FORMAT,
        |view| {
            let mut encoder = device.create_command_encoder(&Default::default());
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &group, &[]);
            pass.draw(0..3, 0..1);
            drop(pass);
            queue.submit([encoder.finish()]);
        },
    )
}

impl SceneRenderer {
    pub(crate) fn test_guest_state(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> impl PartialEq + std::fmt::Debug {
        let colors: std::collections::HashMap<_, _> = self
            .framebuffers
            .iter()
            .map(|(&id, fb)| {
                (
                    id,
                    (
                        fb.layout,
                        fb.height,
                        texture_bytes(device, queue, &fb.color),
                        fb.presentation
                            .as_ref()
                            .map(|p| texture_bytes(device, queue, &p.texture)),
                    ),
                )
            })
            .collect();
        let depths: std::collections::HashMap<_, _> = self
            .depthbuffers
            .iter()
            .map(|(&id, depth)| {
                (
                    id,
                    std::iter::once(depth)
                        .chain(depth.presentation.as_deref())
                        .map(|image| image.test_planes(device, queue))
                        .collect::<Vec<_>>(),
                )
            })
            .collect();
        (
            (
                colors,
                depths,
                self.target_descriptors.clone(),
                self.first_touch.clone(),
                self.depth_first_touch.clone(),
            ),
            (
                self.color_versions
                    .iter()
                    .map(|(&writer, texture)| (writer, texture_bytes(device, queue, texture)))
                    .collect::<std::collections::HashMap<_, _>>(),
                self.presentation_versions
                    .iter()
                    .map(|(&writer, texture)| (writer, texture_bytes(device, queue, texture)))
                    .collect::<std::collections::HashMap<_, _>>(),
                self.isolated_context(),
            ),
            self.coherence.as_ref().map(|c| c.test_state(device, queue)),
            self.framebuffer_copies.test_state(device, queue),
        )
    }
}

impl DepthImage {
    pub(crate) fn test_planes(&self, device: &wgpu::Device, queue: &wgpu::Queue) -> [Vec<u8>; 3] {
        [
            &self.texture,
            self.delta_view.texture(),
            self.delta_depth.as_ref().unwrap().texture(),
        ]
        .map(|texture| texture_bytes(device, queue, texture))
    }
}
