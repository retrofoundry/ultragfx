use super::*;
use crate::tests::common;

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const TARGET: workload::TargetId = workload::TargetId::Guest(0x1000);

fn striped_framebuffer(device: &wgpu::Device, queue: &wgpu::Queue) -> SceneRenderer {
    let mut renderer = SceneRenderer::new(device, FORMAT, 4, 4, false);
    renderer.set_test_scale(2, 256 * 1024 * 1024);
    let fb = renderer.make_fb(
        device,
        ImageLayout {
            width: 4,
            fmt: 0,
            siz: 2,
        },
        (4, 4),
        4,
    );
    for (texture, scale) in [
        (&fb.color, 1),
        (&fb.presentation.as_ref().unwrap().texture, 2),
    ] {
        let bytes: Vec<_> = (0..4 * scale)
            .flat_map(|y| {
                let color = [
                    [255, 0, 0, 255],
                    [0, 255, 0, 255],
                    [0, 0, 255, 255],
                    [255, 255, 255, 255],
                ][y as usize / scale as usize];
                std::iter::repeat_n(color, (4 * scale) as usize).flatten()
            })
            .collect();
        queue.write_texture(
            texture.as_image_copy(),
            &bytes,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4 * 4 * scale),
                rows_per_image: None,
            },
            texture.size(),
        );
    }
    renderer.framebuffers.insert(TARGET, fb);
    renderer
}

#[test]
fn live_scanout_preserves_overheight_rows_and_reduction_clamping() {
    let (device, queue, _) = headless_device();
    let mut renderer = striped_framebuffer(&device, &queue);
    for (reduction, rows, expected_rows) in [
        (
            1,
            0..8,
            vec![
                [255, 0, 0, 255],
                [0, 255, 0, 255],
                [0, 0, 255, 255],
                [255, 255, 255, 255],
                [255, 255, 255, 255],
                [255, 255, 255, 255],
                [255, 255, 255, 255],
                [255, 255, 255, 255],
            ],
        ),
        (
            2,
            0..8,
            vec![
                [255, 0, 0, 255],
                [0, 255, 0, 255],
                [0, 0, 255, 255],
                [255, 255, 255, 255],
            ],
        ),
        (
            1,
            1..4,
            vec![[0, 255, 0, 255], [0, 0, 255, 255], [255, 255, 255, 255]],
        ),
        (
            2,
            1..4,
            vec![[0, 255, 0, 255], [0, 0, 255, 255], [255, 255, 255, 255]],
        ),
    ] {
        renderer.reduction_factor = reduction;
        let actual = common::pixels_from_render(
            &device,
            &queue,
            4,
            expected_rows.len() as u32,
            FORMAT,
            |target| {
                let mut encoder = device.create_command_encoder(&Default::default());
                let image = renderer.resolve_scanout(&device, TARGET, Some(rows), false);
                renderer.compositor().scanout_profiled(
                    &device,
                    &mut encoder,
                    target,
                    &image,
                    crate::PresentationFit::Fill,
                    &Default::default(),
                );
                queue.submit([encoder.finish()]);
            },
        );
        let expected: Vec<u8> = expected_rows
            .into_iter()
            .flat_map(|row| std::iter::repeat_n(row, 4).flatten())
            .collect();
        assert_eq!(actual, expected, "reduction {reduction}");
    }
}

#[test]
fn resolved_scanout_reads_live_pixels_and_native_bypasses_reduction() {
    let (device, queue, _) = headless_device();
    let mut renderer = striped_framebuffer(&device, &queue);
    renderer.reduction_factor = 2;
    let live = renderer.resolve_scanout(&device, TARGET, Some(0..8), false);
    let native = renderer.resolve_scanout(&device, TARGET, Some(0..8), true);
    let texture = &renderer.framebuffers[&TARGET]
        .presentation
        .as_ref()
        .unwrap()
        .texture;
    queue.write_texture(
        texture.as_image_copy(),
        &[0, 255, 255, 255].repeat(64),
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(32),
            rows_per_image: None,
        },
        texture.size(),
    );
    let compositor = renderer.compositor();
    drop(renderer);
    for (image, expected_rows) in [
        (live, vec![[0, 255, 255, 255]; 8]),
        (
            native,
            vec![
                [255, 0, 0, 255],
                [0, 255, 0, 255],
                [0, 0, 255, 255],
                [255, 255, 255, 255],
                [255, 255, 255, 255],
                [255, 255, 255, 255],
                [255, 255, 255, 255],
                [255, 255, 255, 255],
            ],
        ),
    ] {
        let actual = common::pixels_from_render(&device, &queue, 4, 8, FORMAT, |target| {
            let mut encoder = device.create_command_encoder(&Default::default());
            compositor.scanout_profiled(
                &device,
                &mut encoder,
                target,
                &image,
                crate::PresentationFit::Fill,
                &Default::default(),
            );
            queue.submit([encoder.finish()]);
        });
        let expected: Vec<u8> = expected_rows
            .into_iter()
            .flat_map(|row| std::iter::repeat_n(row, 4).flatten())
            .collect();
        assert_eq!(actual, expected);
    }
}

#[test]
fn reduction_does_not_rebuild_the_unused_present_extent() {
    let (device, queue, _) = headless_device();
    let mut renderer = striped_framebuffer(&device, &queue);
    let extent = renderer
        .resolve_scanout(&device, TARGET, None, false)
        .extent;
    renderer.reduction_factor = 2;
    for rows in [1..3, 2..4, 0..3] {
        let scanout = renderer.resolve_scanout(&device, TARGET, Some(rows.clone()), false);
        assert_eq!(scanout.extent, extent);
        assert_eq!(scanout.rows, rows);
    }
    renderer.reduction_factor = 1;
    let scanout = renderer.resolve_scanout(&device, TARGET, Some(0..3), false);
    assert_ne!(scanout.extent, extent);
}
