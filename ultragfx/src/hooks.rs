//! Render hooks composite consumer UI onto the final N64 frame after VI scanout and before
//! submission. Owned native surfaces require `Send` hooks; caller-owned targets support local hooks.

/// A consumer render hook. `init`/`deinit` are lifecycle callbacks (defaulted no-ops); `draw` runs
/// every present.
pub trait RenderHook {
    /// Once when registered. `format` is the non-sRGB view
    /// format your draw-time pipelines must target.
    fn init(&mut self, _device: &wgpu::Device, _queue: &wgpu::Queue, _format: wgpu::TextureFormat) {
    }

    /// Every present, AFTER VI scan-out has been recorded into `frame.view`, BEFORE `ultragfx`
    /// submits + presents. Record commands into `frame.encoder` targeting `frame.view` with
    /// `LoadOp::Load` (Clear would erase the game). Return any standalone command buffers that
    /// MUST run BEFORE the frame encoder (egui-wgpu `update_buffers` output); else `Vec::new()`.
    fn draw(&mut self, frame: HookFrame<'_>) -> Vec<wgpu::CommandBuffer>;

    /// Once on removal / shutdown, before the device drops.
    fn deinit(&mut self) {}
}

/// The per-present handoff to a `RenderHook`. Passed BY VALUE so the `&mut encoder` borrow expires at
/// `draw`'s return and `ultragfx` regains its encoder to `.finish()`.
pub struct HookFrame<'a> {
    /// egui-wgpu uploads every frame, so we hand the device to `draw` as well, not only `init` — a
    /// deliberate, harmless divergence (owned `&`).
    pub device: &'a wgpu::Device,
    pub queue: &'a wgpu::Queue,
    /// Mid-recording; record into it (LoadOp::Load) but do NOT `finish`/`submit`.
    pub encoder: &'a mut wgpu::CommandEncoder,
    /// The scanned-out target view.
    pub view: &'a wgpu::TextureView,
    /// The renderer's non-sRGB render/view format your draw pipelines must target.
    pub format: wgpu::TextureFormat,
    pub width: u32,
    pub height: u32,
}

/// Wraps a stateless draw closure as a `RenderHook` (no `init`/`deinit`, never emits pre-frame
/// command buffers). Crate-internal — backs `Renderer::set_draw_hook`; not re-exported.
pub(crate) struct FnHook<F>(pub(crate) F);

#[allow(clippy::too_many_arguments)]
pub(crate) fn record_and_submit(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    mut encoder: wgpu::CommandEncoder,
    view: &wgpu::TextureView,
    format: wgpu::TextureFormat,
    hook: Option<&mut dyn RenderHook>,
    after_hook: impl FnOnce(&mut wgpu::CommandEncoder),
    profiling: &crate::profiling::Recorder,
) {
    let extra = hook.map_or_else(Vec::new, |hook| {
        hook.draw(HookFrame {
            device,
            queue,
            encoder: &mut encoder,
            view,
            format,
            width: view.texture().width(),
            height: view.texture().height(),
        })
    });
    after_hook(&mut encoder);
    let command = encoder.finish();
    let _submission = profiling.span("submission");
    queue.submit(extra.into_iter().chain(std::iter::once(command)));
    profiling.count("submissions", 1);
    profiling.count("presentation.submissions", 1);
}

impl<F> RenderHook for FnHook<F>
where
    F: for<'a> FnMut(HookFrame<'a>) + 'static,
{
    fn draw(&mut self, frame: HookFrame<'_>) -> Vec<wgpu::CommandBuffer> {
        (self.0)(frame);
        Vec::new()
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use crate::render::headless_device;

    fn clear(encoder: &mut wgpu::CommandEncoder, view: &wgpu::TextureView, color: wgpu::Color) {
        encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(color),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
    }

    struct OrderedHook;

    impl RenderHook for OrderedHook {
        fn draw(&mut self, frame: HookFrame<'_>) -> Vec<wgpu::CommandBuffer> {
            clear(frame.encoder, frame.view, wgpu::Color::BLUE);
            let mut extra = frame.device.create_command_encoder(&Default::default());
            clear(&mut extra, frame.view, wgpu::Color::GREEN);
            vec![extra.finish()]
        }
    }

    #[test]
    fn frame_submission_keeps_hook_extras_before_draw_and_debugger_after_draw() {
        let (device, queue, _) = headless_device();
        for debugger in [false, true] {
            let actual = crate::tests::common::pixels_from_render(
                &device,
                &queue,
                1,
                1,
                wgpu::TextureFormat::Rgba8Unorm,
                |view| {
                    record_and_submit(
                        &device,
                        &queue,
                        device.create_command_encoder(&Default::default()),
                        view,
                        wgpu::TextureFormat::Rgba8Unorm,
                        Some(&mut OrderedHook),
                        |encoder| {
                            if debugger {
                                clear(encoder, view, wgpu::Color::RED);
                            }
                        },
                        &Default::default(),
                    );
                },
            );
            assert_eq!(
                actual,
                if debugger {
                    [255, 0, 0, 255]
                } else {
                    [0, 0, 255, 255]
                }
            );
        }
    }

    struct NoopHook;
    impl RenderHook for NoopHook {
        fn draw(&mut self, _frame: HookFrame<'_>) -> Vec<wgpu::CommandBuffer> {
            Vec::new()
        }
    }

    #[test]
    fn hook_frame_releases_the_encoder_borrow_at_draw_return() {
        let (device, queue, _dual) = headless_device();
        let tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("hf-view"),
            size: wgpu::Extent3d {
                width: 4,
                height: 4,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder =
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });

        let mut hook = NoopHook;
        let extra = hook.draw(HookFrame {
            device: &device,
            queue: &queue,
            encoder: &mut encoder,
            view: &view,
            format: wgpu::TextureFormat::Rgba8Unorm,
            width: 4,
            height: 4,
        });
        assert!(extra.is_empty());
        // The `&mut encoder` borrow expired at draw's return → we regain the encoder to finish it.
        queue.submit(std::iter::once(encoder.finish()));
    }
}
