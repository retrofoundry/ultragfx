use super::*;
use crate::{Hardware, Microcode, NopSink, PresentTarget, Rdram, RdramImage, Renderer};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::ThreadId;
use std::time::Duration;

type Events = Arc<Mutex<Vec<(String, ThreadId)>>>;
fn event(events: &Events, name: &str) {
    events
        .lock()
        .unwrap()
        .push((name.into(), std::thread::current().id()));
}

struct TestSurface {
    device: wgpu::Device,
    config: Mutex<wgpu::SurfaceConfiguration>,
    textures: Arc<Mutex<Vec<wgpu::Texture>>>,
    events: Events,
    faults: Mutex<VecDeque<PresentError>>,
    panic_configure: Arc<AtomicBool>,
    acquire: Option<(mpsc::Sender<()>, Mutex<mpsc::Receiver<()>>)>,
}

impl Surface for TestSurface {
    fn configure(&self, _: &wgpu::Device, config: &wgpu::SurfaceConfiguration) {
        event(&self.events, "configure");
        assert!(
            !self.panic_configure.load(Ordering::SeqCst),
            "configure failed"
        );
        *self.config.lock().unwrap() = config.clone();
    }
    fn acquire(&self) -> Result<Option<SurfaceFrame>, PresentError> {
        event(&self.events, "acquire");
        if let Some((entered, release)) = &self.acquire {
            entered.send(()).unwrap();
            release.lock().unwrap().recv().unwrap();
        }
        if let Some(error) = self.faults.lock().unwrap().pop_front() {
            return Err(error);
        }
        let config = self.config.lock().unwrap();
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("present-test-surface"),
            size: wgpu::Extent3d {
                width: config.width,
                height: config.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: config.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &config.view_formats,
        });
        self.textures.lock().unwrap().push(texture.clone());
        Ok(Some(SurfaceFrame::Offscreen(texture)))
    }
}

fn renderer_config() -> RendererConfig {
    RendererConfig {
        resolution_multiplier: 1,
        sample_count: 1,
        present_mode: wgpu::PresentMode::Fifo,
        format: None,
        clear_policy: crate::ClearPolicy::PerFrame,
        power_preference: wgpu::PowerPreference::LowPower,
    }
}

struct Harness {
    renderer: Renderer,
    textures: Arc<Mutex<Vec<wgpu::Texture>>>,
    events: Events,
    panic_configure: Arc<AtomicBool>,
}

fn harness(
    format: wgpu::TextureFormat,
    faults: Vec<PresentError>,
    acquire: Option<(mpsc::Sender<()>, mpsc::Receiver<()>)>,
) -> Harness {
    let (device, queue, _) = crate::render::headless_device();
    let config = wgpu::SurfaceConfiguration {
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        format,
        width: 64,
        height: 64,
        present_mode: wgpu::PresentMode::Fifo,
        desired_maximum_frame_latency: 2,
        alpha_mode: wgpu::CompositeAlphaMode::Auto,
        view_formats: if format.is_srgb() {
            vec![format.remove_srgb_suffix()]
        } else {
            vec![]
        },
    };
    let mut renderer = Renderer::with_device(
        device.clone(),
        queue.clone(),
        PresentTarget::Headless {
            format: crate::render_format_of(&config),
            width: 64,
            height: 64,
        },
        renderer_config(),
    );
    let textures = Arc::new(Mutex::new(Vec::new()));
    let events = Arc::new(Mutex::new(Vec::new()));
    let panic_configure = Arc::new(AtomicBool::new(false));
    let surface = TestSurface {
        device: device.clone(),
        config: Mutex::new(config.clone()),
        textures: textures.clone(),
        events: events.clone(),
        faults: Mutex::new(faults.into()),
        panic_configure: panic_configure.clone(),
        acquire: acquire.map(|(entered, release)| (entered, Mutex::new(release))),
    };
    renderer.presenter = Some(Presenter::spawn(Presentation {
        surface: Box::new(surface),
        config: config.clone(),
        compositor: renderer.inner.compositor(),
        hook: None,
        device,
        queue,
        format: crate::render_format_of(&config),
        needs_configure: true,
        profiling: Default::default(),
    }));
    Harness {
        renderer,
        textures,
        events,
        panic_configure,
    }
}

struct Image(Vec<u8>, Option<crate::ViRegisters>);
impl Hardware for Image {
    fn rdram(&self) -> impl Rdram + '_ {
        RdramImage::new(&self.0)
    }
    fn vi(&self) -> Option<crate::ViRegisters> {
        self.1
    }
}

fn pixels(renderer: &Renderer, texture: &wgpu::Texture) -> Vec<u8> {
    let width = texture.width();
    let height = texture.height();
    let stride = (width * 4).div_ceil(256) * 256;
    let buffer = renderer.device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: u64::from(stride * height),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = renderer.device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(stride),
                rows_per_image: Some(height),
            },
        },
        texture.size(),
    );
    renderer.queue.submit([encoder.finish()]);
    let (tx, rx) = mpsc::channel();
    buffer
        .slice(..)
        .map_async(wgpu::MapMode::Read, move |r| tx.send(r).unwrap());
    renderer
        .device
        .poll(wgpu::PollType::wait_indefinitely())
        .unwrap();
    rx.recv().unwrap().unwrap();
    let bytes = buffer.slice(..).get_mapped_range();
    bytes
        .chunks(stride as usize)
        .flat_map(|row| row[..width as usize * 4].iter().copied())
        .collect()
}

#[test]
fn original_matches_synchronous_bytes_count_order_resize_and_srgb_view() {
    for format in [
        wgpu::TextureFormat::Rgba8Unorm,
        wgpu::TextureFormat::Rgba8UnormSrgb,
    ] {
        let mut h = harness(format, vec![], None);
        let (data, entry) = crate::tests::fixtures::fixture("flat-color--vertex-colors");
        let hw = Image(data.to_vec(), None);
        for (index, (scale, reduction, size)) in
            [(1, 1, 64), (2, 1, 32), (4, 2, 64)].into_iter().enumerate()
        {
            h.renderer
                .set_scale_policy(crate::ScalePolicy::Fixed(scale))
                .unwrap();
            h.renderer.begin_frame();
            h.renderer
                .process_dl(&hw, entry, Microcode::F3dex2, &mut NopSink);
            h.renderer.set_reduction_factor(reduction).unwrap();
            h.renderer.resize(size, size);
            h.renderer.present(&hw).unwrap();
            let texture = h.textures.lock().unwrap()[index].clone();
            let expected = h.renderer.device.create_texture(&wgpu::TextureDescriptor {
                label: None,
                size: texture.size(),
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: format.remove_srgb_suffix(),
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            let presenter = h.renderer.presenter.take();
            h.renderer
                .present_to(&hw, &expected.create_view(&Default::default()));
            h.renderer.presenter = presenter;
            let expected = pixels(&h.renderer, &expected);
            assert!(expected
                .as_chunks::<4>()
                .0
                .iter()
                .any(|pixel| { pixel[..3].iter().any(|channel| (32..224).contains(channel)) }));
            assert_eq!(
                pixels(&h.renderer, &texture),
                expected,
                "scale={scale} reduction={reduction}"
            );
        }
        assert_eq!(h.textures.lock().unwrap().len(), 3);
        let events = h.events.lock().unwrap();
        assert!(events
            .iter()
            .all(|(_, id)| *id != std::thread::current().id()));
    }
}

struct Hook {
    events: Events,
    name: &'static str,
    panic: Option<&'static str>,
}
impl RenderHook for Hook {
    fn init(&mut self, _: &wgpu::Device, _: &wgpu::Queue, _: wgpu::TextureFormat) {
        event(&self.events, &format!("{} init", self.name));
        assert_ne!(self.panic, Some("init"), "init failed");
    }
    fn draw(&mut self, frame: crate::HookFrame<'_>) -> Vec<wgpu::CommandBuffer> {
        event(&self.events, &format!("{} draw", self.name));
        assert_ne!(self.panic, Some("draw"), "draw failed");
        frame
            .encoder
            .begin_render_pass(&wgpu::RenderPassDescriptor {
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: frame.view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLUE),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
        let mut extra = frame.device.create_command_encoder(&Default::default());
        extra.begin_render_pass(&wgpu::RenderPassDescriptor {
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: frame.view,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::GREEN),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        vec![extra.finish()]
    }
    fn deinit(&mut self) {
        event(&self.events, &format!("{} deinit", self.name));
        assert_ne!(self.panic, Some("deinit"), "deinit failed");
    }
}

#[test]
fn hooks_are_ordered_acknowledged_and_run_for_frames_without_images() {
    let mut h = harness(wgpu::TextureFormat::Rgba8Unorm, vec![], None);
    for name in ["a", "b"] {
        h.renderer
            .set_send_render_hook(Box::new(Hook {
                events: h.events.clone(),
                name,
                panic: None,
            }))
            .unwrap();
        h.renderer.present_host(None).unwrap();
        let texture = h.textures.lock().unwrap().last().unwrap().clone();
        assert!(pixels(&h.renderer, &texture)
            .as_chunks::<4>()
            .0
            .iter()
            .all(|pixel| *pixel == [0, 0, 255, 255]));
        h.renderer
            .present_host_to(None, &texture.create_view(&Default::default()));
    }
    assert!(h.renderer.take_send_render_hook().unwrap().is_some());
    assert!(h.renderer.take_send_render_hook().unwrap().is_none());
    h.renderer.shutdown();
    let events = h.events.lock().unwrap();
    let hooks: Vec<_> = events
        .iter()
        .filter(|(name, _)| name.starts_with("a ") || name.starts_with("b "))
        .collect();
    assert_eq!(
        hooks
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>(),
        ["a init", "a draw", "a draw", "a deinit", "b init", "b draw", "b draw", "b deinit"]
    );
    assert!(hooks
        .iter()
        .all(|(_, thread)| *thread == hooks[0].1 && *thread != std::thread::current().id()));
}

#[test]
fn surface_errors_are_returned_for_the_same_call_and_lost_reconfigures() {
    let mut h = harness(
        wgpu::TextureFormat::Rgba8Unorm,
        vec![
            PresentError::Timeout,
            PresentError::SurfaceLost,
            PresentError::DeviceLost,
        ],
        None,
    );
    for expected in [
        PresentError::Timeout,
        PresentError::SurfaceLost,
        PresentError::DeviceLost,
    ] {
        assert_eq!(h.renderer.present_last(), Err(expected));
    }
    h.renderer.present_last().unwrap();
    assert_eq!(
        h.events
            .lock()
            .unwrap()
            .iter()
            .filter(|(event, _)| event == "configure")
            .count(),
        2
    );
    assert_eq!(h.textures.lock().unwrap().len(), 1);
}

#[test]
fn hook_panics_latch_failures_and_deinitialize_once() {
    for stage in ["init", "draw", "deinit"] {
        let mut h = harness(wgpu::TextureFormat::Rgba8Unorm, vec![], None);
        let installed = h.renderer.set_send_render_hook(Box::new(Hook {
            events: h.events.clone(),
            name: "panic",
            panic: Some(stage),
        }));
        let result = match stage {
            "init" => installed,
            "draw" => h.renderer.present_last(),
            _ => h.renderer.take_send_render_hook().map(|_| ()),
        };
        assert!(matches!(result, Err(PresentError::PresenterFailed(_))));
        h.renderer.resize(20, 20);
        h.renderer.reset();
        h.renderer.reconfigure(renderer_config());
        assert_eq!(h.renderer.presenter_error(), result.err());
        assert!(h.renderer.present_last().is_err());
        h.renderer.shutdown();
        assert_eq!(
            h.events
                .lock()
                .unwrap()
                .iter()
                .filter(|(event, _)| event == "panic deinit")
                .count(),
            1
        );
    }
}

#[test]
fn present_waits_for_surface_acquisition_before_returning() {
    let (entered, acquired) = mpsc::channel();
    let (release, wait) = mpsc::channel();
    let (returned, result) = mpsc::channel();
    let caller = std::thread::spawn(move || {
        let mut h = harness(
            wgpu::TextureFormat::Rgba8Unorm,
            vec![],
            Some((entered, wait)),
        );
        returned.send(h.renderer.present_last()).unwrap();
        assert_eq!(h.textures.lock().unwrap().len(), 1);
    });
    acquired.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(result.try_recv(), Err(mpsc::TryRecvError::Empty));
    release.send(()).unwrap();
    assert_eq!(result.recv_timeout(Duration::from_secs(5)).unwrap(), Ok(()));
    caller.join().unwrap();
}

#[test]
fn failed_reconfigure_finishes_canonical_reset() {
    let mut h = harness(wgpu::TextureFormat::Rgba8Unorm, vec![], None);
    let (data, entry) = crate::tests::fixtures::fixture("flat-color--vertex-colors");
    let hw = Image(data.to_vec(), None);
    h.renderer
        .process_dl(&hw, entry, Microcode::F3dex2, &mut NopSink);
    assert!(h.renderer.last_scanout_addr.is_some());
    assert!(!h.renderer.frame_scenes.is_empty());
    h.panic_configure.store(true, Ordering::SeqCst);
    let config = RendererConfig {
        format: Some(wgpu::TextureFormat::Bgra8Unorm),
        resolution_multiplier: 2,
        ..renderer_config()
    };
    h.renderer.reconfigure(config);
    let error = PresentError::PresenterFailed("configure failed".into());
    assert_eq!(h.renderer.presenter_error(), Some(error.clone()));
    assert!(h.renderer.last_scanout_addr.is_none());
    assert!(h.renderer.frame_scenes.is_empty());
    assert_eq!(h.renderer.surface_format, wgpu::TextureFormat::Bgra8Unorm);
    assert_eq!(h.renderer.config, config);
    assert_eq!(h.renderer.scale_policy, crate::ScalePolicy::Fixed(2));
    assert_eq!(h.renderer.present_last(), Err(error.clone()));
    h.renderer.begin_frame();
    h.renderer
        .process_dl(&hw, entry, Microcode::F3dex2, &mut NopSink);
    assert!(!h.renderer.frame_scenes.is_empty());
    assert_eq!(h.renderer.present(&hw), Err(error));
}

#[test]
fn reset_clears_canonical_state_after_presenter_failure() {
    let mut h = harness(wgpu::TextureFormat::Rgba8Unorm, vec![], None);
    let (data, entry) = crate::tests::fixtures::fixture("flat-color--vertex-colors");
    let hw = Image(data.to_vec(), None);
    h.renderer.begin_frame();
    h.renderer
        .process_dl(&hw, entry, Microcode::F3dex2, &mut NopSink);
    assert_ne!(h.renderer.rdp, Default::default());
    assert!(!h.renderer.frame_scenes.is_empty());
    h.renderer.guest_writes.push(crate::GuestWrite {
        address: 0,
        bytes: vec![1],
    });
    h.renderer.color_image_epoch = 42;
    h.renderer.inner.dither_seed = 123;
    h.panic_configure.store(true, Ordering::SeqCst);
    h.renderer.resize(32, 32);
    let error = h.renderer.presenter_error().unwrap();
    h.renderer.reset();
    assert_eq!(h.renderer.rdp, Default::default());
    assert!(h.renderer.take_guest_writes().is_empty());
    assert_eq!(h.renderer.color_image_epoch, 0);
    assert_eq!(h.renderer.inner.frame_serial, 0);
    assert_eq!(h.renderer.inner.dither_seed, 0);
    assert_eq!(h.renderer.inner.scanout_images().count(), 0);
    assert!(h.renderer.frame_scenes.is_empty());
    assert!(h.renderer.last_scanout_addr.is_none());
    assert_eq!(h.renderer.present_last(), Err(error));
    h.renderer.begin_frame();
    h.renderer
        .process_dl(&hw, entry, Microcode::F3dex2, &mut NopSink);
    assert!(!h.renderer.frame_scenes.is_empty());
}

#[test]
fn reconfigure_updates_canonical_state_after_presenter_failure() {
    let mut h = harness(wgpu::TextureFormat::Rgba8Unorm, vec![], None);
    let (data, entry) = crate::tests::fixtures::fixture("flat-color--vertex-colors");
    let hw = Image(data.to_vec(), None);
    h.renderer
        .process_dl(&hw, entry, Microcode::F3dex2, &mut NopSink);
    h.panic_configure.store(true, Ordering::SeqCst);
    h.renderer.resize(32, 32);
    let error = h.renderer.presenter_error().unwrap();
    let config = RendererConfig {
        format: Some(wgpu::TextureFormat::Bgra8Unorm),
        resolution_multiplier: 2,
        ..renderer_config()
    };
    h.renderer.reconfigure(config);
    assert_eq!(h.renderer.config, config);
    assert_eq!(h.renderer.surface_format, wgpu::TextureFormat::Bgra8Unorm);
    assert_eq!(h.renderer.effective_resolution_multiplier(), 2);
    assert_eq!(
        h.renderer.requested_scale_policy(),
        crate::ScalePolicy::Fixed(2)
    );
    assert!(h.renderer.last_scanout_addr.is_none());
    assert!(h.renderer.frame_scenes.is_empty());
    h.renderer.begin_frame();
    h.renderer
        .process_dl(&hw, entry, Microcode::F3dex2, &mut NopSink);
    assert!(!h.renderer.frame_scenes.is_empty());
    assert_eq!(h.renderer.present_last(), Err(error));
}

#[test]
fn configure_panic_allows_rendering_and_returns_error_before_resolving_scanout() {
    let mut h = harness(wgpu::TextureFormat::Rgba8Unorm, vec![], None);
    h.panic_configure.store(true, Ordering::SeqCst);
    h.renderer.resize(32, 32);
    let error = PresentError::PresenterFailed("configure failed".into());
    assert_eq!(h.renderer.presenter_error(), Some(error.clone()));
    h.renderer.begin_frame();
    let (data, entry) = crate::tests::fixtures::fixture("flat-color--vertex-colors");
    let hw = Image(data.to_vec(), None);
    h.renderer
        .process_dl(&hw, entry, Microcode::F3dex2, &mut NopSink);
    assert!(!h.renderer.frame_scenes.is_empty());
    assert_eq!(h.renderer.present(&hw), Err(error.clone()));
    assert_eq!(
        h.renderer.threaded_present(
            Some(crate::ScanoutSource {
                target: crate::TargetId::Guest(0xdead),
                rows: None,
            }),
            None,
            false,
        ),
        Err(error),
    );
}

#[test]
fn caller_owned_targets_still_scan_out_after_presenter_failure() {
    for failure_in_hook in [false, true] {
        let mut h = harness(wgpu::TextureFormat::Rgba8Unorm, vec![], None);
        let (data, entry) = crate::tests::fixtures::fixture("flat-color--vertex-colors");
        let hw = Image(data.to_vec(), None);
        h.renderer
            .process_dl(&hw, entry, Microcode::F3dex2, &mut NopSink);
        h.renderer.present_last().unwrap();
        let source = h.textures.lock().unwrap()[0].clone();
        let expected = pixels(&h.renderer, &source);
        assert!(expected
            .as_chunks::<4>()
            .0
            .iter()
            .any(|pixel| pixel[0] != pixel[1]));
        if failure_in_hook {
            h.renderer
                .set_send_render_hook(Box::new(Hook {
                    events: h.events.clone(),
                    name: "caller panic",
                    panic: Some("draw"),
                }))
                .unwrap();
        } else {
            h.panic_configure.store(true, Ordering::SeqCst);
            h.renderer.resize(32, 32);
            assert!(h.renderer.presenter_error().is_some());
        }
        for method in 0..4 {
            let target = h.renderer.device.create_texture(&wgpu::TextureDescriptor {
                label: None,
                size: source.size(),
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: source.format(),
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            let view = target.create_view(&Default::default());
            match method {
                0 => h.renderer.present_to(&hw, &view),
                1 => h.renderer.present_last_to(&view),
                2 => h.renderer.present_host_to(None, &view),
                _ => h.renderer.present_native_to(&hw, &view),
            }
            assert_eq!(pixels(&h.renderer, &target), expected);
        }
    }
}

#[test]
fn startup_preserves_a_host_configured_surface() {
    let mut h = harness(wgpu::TextureFormat::Rgba8Unorm, vec![], None);
    let state = h
        .renderer
        .presenter
        .as_ref()
        .unwrap()
        .worker
        .request(|slot| Ok(slot.take().unwrap()))
        .unwrap();
    h.events.lock().unwrap().clear();
    h.renderer.presenter = Some(Presenter::spawn(state));
    assert!(h.events.lock().unwrap().is_empty());
    h.renderer.present_last().unwrap();
    assert_eq!(h.events.lock().unwrap()[0].0, "acquire");
    h.renderer.resize(32, 32);
    assert_eq!(h.events.lock().unwrap()[1].0, "configure");
}

#[test]
fn vi_origin_rows_overheight_and_fallback_preserve_every_scanout() {
    use n64_gbi::encode::*;
    let mut h = harness(wgpu::TextureFormat::Rgba8Unorm, vec![], None);
    for (base, color) in [(0x1000, 0xf801_f801), (0x8000, 0x07c1_07c1)] {
        let commands = [
            gdp_set_color_image(0, 2, 64, base),
            gdp_set_scissor(0, 0, 0, 256, 256),
            gdp_set_cycle_type(3),
            gdp_set_fill_color(color),
            gdp_fill_rectangle(0, 0, 252, 124),
            gdp_set_fill_color(0x003f_003f),
            gdp_fill_rectangle(0, 128, 252, 252),
            gsp_enddl(),
        ];
        let hw = Image(
            commands
                .into_iter()
                .flat_map(|(a, b)| [a.to_be_bytes(), b.to_be_bytes()].concat())
                .collect(),
            None,
        );
        h.renderer
            .process_dl(&hw, 0, Microcode::F3dex2, &mut NopSink);
    }
    for (origin, height) in [
        (0x1000, 64),
        (0x1000 + 128 * 16, 32),
        (0x1000, 128),
        (0x8000, 64),
        (0xf000, 64),
    ] {
        let vi = crate::ViRegisters {
            origin,
            status: 2,
            width: 64,
            v_start: height * 2,
            y_scale: 1024,
            ..Default::default()
        };
        let hw = Image(vec![], Some(vi));
        h.renderer.present(&hw).unwrap();
        let texture = h.textures.lock().unwrap().last().unwrap().clone();
        let actual = pixels(&h.renderer, &texture);
        let presenter = h.renderer.presenter.take();
        h.renderer
            .present_to(&hw, &texture.create_view(&Default::default()));
        h.renderer.presenter = presenter;
        assert_eq!(
            actual,
            pixels(&h.renderer, &texture),
            "origin {origin:#x}, height {height}"
        );
    }
    for address in [Some(0x1000), Some(0x8000), Some(0xf000), None] {
        h.renderer.present_host(address).unwrap();
        let texture = h.textures.lock().unwrap().last().unwrap().clone();
        let actual = pixels(&h.renderer, &texture);
        let presenter = h.renderer.presenter.take();
        h.renderer
            .present_host_to(address, &texture.create_view(&Default::default()));
        h.renderer.presenter = presenter;
        assert_eq!(actual, pixels(&h.renderer, &texture));
    }
    assert_eq!(h.textures.lock().unwrap().len(), 9);
    h.renderer.reset();
    assert!(h.renderer.last_scanout_addr.is_none());
    h.renderer.reconfigure(renderer_config());
    assert!(h.renderer.frame_scenes.is_empty());
    h.renderer.present_last().unwrap();
    assert_eq!(h.textures.lock().unwrap().len(), 10);
}

#[test]
fn non_send_hook_is_rejected_before_init_on_threaded_targets() {
    let mut h = harness(wgpu::TextureFormat::Rgba8Unorm, vec![], None);
    let count = std::rc::Rc::new(std::cell::Cell::new(0));
    struct LocalHook(std::rc::Rc<std::cell::Cell<u32>>);
    impl RenderHook for LocalHook {
        fn init(&mut self, _: &wgpu::Device, _: &wgpu::Queue, _: wgpu::TextureFormat) {
            self.0.set(self.0.get() + 1);
        }
        fn draw(&mut self, _: crate::HookFrame<'_>) -> Vec<wgpu::CommandBuffer> {
            vec![]
        }
    }
    assert_eq!(
        h.renderer
            .try_set_render_hook(Box::new(LocalHook(count.clone()))),
        Err(PresentError::HookRequiresSend)
    );
    assert_eq!(count.get(), 0);
    for draw_only in [false, true] {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            if draw_only {
                let count = count.clone();
                h.renderer
                    .set_draw_hook(move |_| count.set(count.get() + 1));
            } else {
                h.renderer
                    .set_render_hook(Box::new(LocalHook(count.clone())));
            }
        }));
        assert_eq!(result.is_err(), cfg!(debug_assertions));
        assert_eq!(count.get(), 0);
    }
}

#[cfg(feature = "debug-ui")]
#[test]
fn debugger_hook_panic_latches_failure_and_deinitializes_on_worker() {
    let mut h = harness(wgpu::TextureFormat::Rgba8Unorm, vec![], None);
    h.renderer
        .set_send_render_hook(Box::new(Hook {
            events: h.events.clone(),
            name: "debug panic",
            panic: Some("draw"),
        }))
        .unwrap();
    h.renderer.set_debugger_enabled(true);
    let error = h.renderer.present_last().unwrap_err();
    assert!(
        matches!(&error, PresentError::PresenterFailed(message) if message.contains("draw failed"))
    );
    assert_eq!(h.renderer.presenter_error(), Some(error.clone()));
    assert_eq!(h.renderer.present_last(), Err(error.clone()));
    h.renderer.set_debugger_enabled(false);
    assert_eq!(h.renderer.present_last(), Err(error));
    h.renderer.shutdown();
    let events = h.events.lock().unwrap();
    let hooks: Vec<_> = events
        .iter()
        .filter(|(name, _)| name.starts_with("debug panic"))
        .collect();
    assert_eq!(
        hooks
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>(),
        ["debug panic init", "debug panic draw", "debug panic deinit"]
    );
    assert_eq!(hooks[1].1, std::thread::current().id());
    assert_eq!(hooks[0].1, hooks[2].1);
    assert_ne!(hooks[0].1, std::thread::current().id());
}

#[cfg(feature = "debug-ui")]
#[test]
fn debugger_presents_synchronously_with_hook_lifecycle_on_the_worker() {
    let mut h = harness(wgpu::TextureFormat::Rgba8Unorm, vec![], None);
    h.renderer
        .set_send_render_hook(Box::new(Hook {
            events: h.events.clone(),
            name: "debug",
            panic: None,
        }))
        .unwrap();
    h.renderer.set_debugger_enabled(true);
    h.renderer.present_last().unwrap();
    h.renderer.set_debugger_enabled(false);
    h.renderer.present_last().unwrap();
    h.renderer.shutdown();
    let events = h.events.lock().unwrap();
    let draws: Vec<_> = events
        .iter()
        .filter(|(name, _)| name == "debug draw")
        .collect();
    assert_eq!(draws[0].1, std::thread::current().id());
    assert_ne!(draws[1].1, std::thread::current().id());
    assert!(events
        .iter()
        .filter(|(name, _)| name.ends_with("init"))
        .all(|(_, id)| *id != std::thread::current().id()));
}

#[cfg(feature = "capture")]
#[test]
fn presentation_invalidates_canonical_capture_identity() {
    let mut h = harness(wgpu::TextureFormat::Rgba8Unorm, vec![], None);
    let old = h.renderer.capture_generation.clone();
    h.renderer.present_last().unwrap();
    assert!(!std::rc::Rc::ptr_eq(&old, &h.renderer.capture_generation));
}

#[cfg(feature = "profiling")]
#[test]
fn presenter_counters_are_separate_from_canonical_work() {
    let mut h = harness(wgpu::TextureFormat::Rgba8Unorm, vec![], None);
    let canonical = crate::profiling::Recorder::new(crate::profiling::Mode::Counters);
    let presentation = crate::profiling::Recorder::new(crate::profiling::Mode::Counters);
    h.renderer.set_profiling(canonical.clone());
    h.renderer
        .set_present_profiling(presentation.clone())
        .unwrap();
    h.renderer.present_last().unwrap();
    assert_eq!(canonical.snapshot().counters.get("submissions"), None);
    assert_eq!(
        presentation
            .snapshot()
            .counters
            .get("presentation.submissions"),
        Some(&1)
    );
}

#[test]
fn format_reconfigure_is_acknowledged_before_the_next_present() {
    let mut h = harness(wgpu::TextureFormat::Rgba8Unorm, vec![], None);
    h.renderer
        .set_send_render_hook(Box::new(Hook {
            events: h.events.clone(),
            name: "format",
            panic: None,
        }))
        .unwrap();
    let mut config = renderer_config();
    config.format = Some(wgpu::TextureFormat::Bgra8Unorm);
    config.present_mode = wgpu::PresentMode::Immediate;
    h.renderer.reconfigure(config);
    h.renderer.present_last().unwrap();
    let texture = h.textures.lock().unwrap().last().unwrap().clone();
    assert_eq!(texture.format(), wgpu::TextureFormat::Bgra8Unorm);
    assert!(pixels(&h.renderer, &texture)
        .as_chunks::<4>()
        .0
        .iter()
        .all(|pixel| *pixel == [255, 0, 0, 255]));
}
