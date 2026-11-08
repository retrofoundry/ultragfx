pub(crate) mod canonical;
mod schedule;
mod worker;

use crate::interpolation::{PhaseRenderer, SourceFrame};
use crate::render::{Compositor, Scanout};
use crate::scanout::PendingScanout;
use crate::{PresentError, RenderHook, RendererConfig};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use worker::Worker;

#[derive(Default)]
struct Mailbox {
    sources: VecDeque<PendingScanout>,
    queued: bool,
    discontinuity: bool,
}

#[derive(Default)]
struct Schedule {
    latest: Option<PendingScanout>,
    previous: Option<SourceFrame>,
    older: Option<SourceFrame>,
    deadlines: Option<schedule::Deadlines>,
    prepare: std::time::Duration,
    phase: Option<PhaseRenderer>,
    original: Option<(wgpu::PresentMode, u32)>,
}

trait Surface: Send {
    fn configure(&self, device: &wgpu::Device, config: &wgpu::SurfaceConfiguration);
    fn acquire(&self) -> Result<Option<SurfaceFrame>, PresentError>;
}

enum SurfaceFrame {
    Native(wgpu::SurfaceTexture),
    #[cfg(test)]
    Offscreen(wgpu::Texture, Arc<tests::Presented>),
}

impl SurfaceFrame {
    fn texture(&self) -> &wgpu::Texture {
        match self {
            Self::Native(frame) => &frame.texture,
            #[cfg(test)]
            Self::Offscreen(texture, _) => texture,
        }
    }
    fn present(self) {
        match self {
            Self::Native(frame) => frame.present(),
            #[cfg(test)]
            Self::Offscreen(texture, presented) => presented.push(texture),
        }
    }
}

impl Surface for wgpu::Surface<'static> {
    fn configure(&self, device: &wgpu::Device, config: &wgpu::SurfaceConfiguration) {
        self.configure(device, config);
    }
    fn acquire(&self) -> Result<Option<SurfaceFrame>, PresentError> {
        match self.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame)
            | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => {
                Ok(Some(SurfaceFrame::Native(frame)))
            }
            wgpu::CurrentSurfaceTexture::Lost | wgpu::CurrentSurfaceTexture::Outdated => {
                Err(PresentError::SurfaceLost)
            }
            wgpu::CurrentSurfaceTexture::Timeout => Err(PresentError::Timeout),
            wgpu::CurrentSurfaceTexture::Occluded => Ok(None),
            wgpu::CurrentSurfaceTexture::Validation => Err(PresentError::DeviceLost),
        }
    }
}

type SendHook = Box<dyn RenderHook + Send>;

pub(crate) struct Presentation {
    surface: Box<dyn Surface>,
    config: wgpu::SurfaceConfiguration,
    compositor: Compositor,
    hook: Option<SendHook>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    format: wgpu::TextureFormat,
    needs_configure: bool,
    profiling: crate::profiling::Recorder,
    configuration: Arc<Mutex<()>>,
    schedule: Schedule,
}

impl Presentation {
    fn configure(&mut self) {
        let _configuration = self
            .configuration
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        self.surface.configure(&self.device, &self.config);
        self.needs_configure = false;
    }

    fn take_hook(&mut self) -> Option<SendHook> {
        let mut hook = self.hook.take()?;
        hook.deinit();
        Some(hook)
    }

    fn clear_sources(&mut self) {
        self.schedule.latest = None;
        self.schedule.previous = None;
        self.schedule.older = None;
        self.schedule.deadlines = None;
        self.schedule.phase = None;
        if let Some((mode, latency)) = self.schedule.original.take() {
            self.config.present_mode = mode;
            self.config.desired_maximum_frame_latency = latency;
            self.needs_configure = true;
        }
    }

    fn source(&mut self, pending: PendingScanout) {
        let period = pending.policy.period().unwrap();
        if self
            .schedule
            .latest
            .as_ref()
            .is_none_or(|last| last.policy != pending.policy)
        {
            self.schedule.deadlines = Some(schedule::Deadlines::new(Instant::now(), period));
        }
        if self.schedule.original.is_none() {
            self.schedule.original = Some((
                self.config.present_mode,
                self.config.desired_maximum_frame_latency,
            ));
            self.config.present_mode = wgpu::PresentMode::Fifo;
            self.config.desired_maximum_frame_latency = 2;
            self.needs_configure = true;
        }
        let previous = self.schedule.latest.take().and_then(|last| {
            last.source.filter(|previous| {
                pending
                    .source
                    .as_ref()
                    .is_some_and(|next| next.compatible(previous))
            })
        });
        self.schedule.older = self.schedule.previous.take().filter(|older| {
            previous
                .as_ref()
                .is_some_and(|previous| previous.compatible(older))
        });
        self.schedule.previous = previous;
        self.schedule.latest = Some(pending);
    }

    fn deadline(&self) -> Option<Instant> {
        self.schedule
            .deadlines
            .as_ref()
            .map(|deadlines| deadlines.acquire_at(self.schedule.prepare))
    }

    fn tick(&mut self) {
        if self.needs_configure {
            self.configure();
        }
        let deadlines = self.schedule.deadlines.as_mut().unwrap();
        let skipped = deadlines.skip_expired(Instant::now());
        self.profiling
            .count("presentation.skipped_deadlines", skipped);
        let deadline = deadlines.next();
        let lead = deadline.duration_since(deadlines.acquire_at(self.schedule.prepare));
        if deadlines.acquire_at(self.schedule.prepare) > Instant::now() {
            return;
        }
        let latest = self.schedule.latest.as_ref().unwrap();
        let period = latest.policy.period().unwrap();
        let phase = self
            .schedule
            .previous
            .as_ref()
            .zip(latest.source.as_ref())
            .map(|(a, b)| {
                match schedule::select_phase(
                    self.schedule.older.as_ref().map(SourceFrame::visible_at),
                    a.visible_at(),
                    b.visible_at(),
                    deadline,
                    lead,
                ) {
                    schedule::Phase::Previous(t) => {
                        (self.schedule.older.as_ref().unwrap().clone(), a.clone(), t)
                    }
                    schedule::Phase::Latest(t) => (a.clone(), b.clone(), t),
                }
            });
        let image = latest.image.clone();
        let result = self.present_frame(image, None, |_, _| {}, phase, Some((deadline, period)));
        if self.needs_configure {
            self.configure();
        }
        self.schedule.deadlines.as_mut().unwrap().presented();
        if let Err(error) = result {
            log::warn!("scheduled presentation: {error:?}");
        }
    }

    pub(crate) fn present(
        &mut self,
        source: Option<Scanout>,
        target: Option<wgpu::TextureView>,
        after_hook: impl FnOnce(&mut wgpu::CommandEncoder, &wgpu::TextureView),
    ) -> Result<(), PresentError> {
        self.present_frame(source, target, after_hook, None, None)
            .map(|_| ())
    }

    fn present_frame(
        &mut self,
        mut source: Option<Scanout>,
        target: Option<wgpu::TextureView>,
        after_hook: impl FnOnce(&mut wgpu::CommandEncoder, &wgpu::TextureView),
        phase: Option<(SourceFrame, SourceFrame, f32)>,
        timing: Option<(Instant, std::time::Duration)>,
    ) -> Result<Option<Instant>, PresentError> {
        let _span = self.profiling.span("presentation");
        let frame = if target.is_none() {
            match self.surface.acquire() {
                Ok(Some(frame)) => Some(frame),
                Ok(None) => return Ok(None),
                Err(PresentError::SurfaceLost) => {
                    self.needs_configure = true;
                    return Err(PresentError::SurfaceLost);
                }
                Err(error) => return Err(error),
            }
        } else {
            None
        };
        if timing.is_some_and(|(deadline, period)| Instant::now() >= deadline + period) {
            self.profiling.count("presentation.expired_acquisitions", 1);
            return Ok(None);
        }
        let prepare_start = timing.map(|_| Instant::now());
        if let Some((a, b, t)) = phase {
            let renderer = self.schedule.phase.get_or_insert_with(|| {
                PhaseRenderer::new(
                    self.device.clone(),
                    self.queue.clone(),
                    self.format,
                    b.legacy_extent(),
                    source.as_ref().map_or(1, |source| source.reduction),
                )
            });
            match renderer.scanout_phase(&a, &b, t) {
                Ok(image) => source = Some(image),
                Err(error) => log::warn!("phase render: {error:?}"),
            }
        }
        let view = target.unwrap_or_else(|| {
            frame
                .as_ref()
                .unwrap()
                .texture()
                .create_view(&wgpu::TextureViewDescriptor {
                    format: Some(self.format),
                    ..Default::default()
                })
        });
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("present"),
            });
        if let Some(source) = &source {
            self.compositor.scanout_profiled(
                &self.device,
                &mut encoder,
                &view,
                source,
                &self.profiling,
            );
        }
        crate::hooks::record_and_submit(
            &self.device,
            &self.queue,
            encoder,
            &view,
            self.format,
            self.hook
                .as_mut()
                .map(|hook| &mut **hook as &mut dyn RenderHook),
            |encoder| after_hook(encoder, &view),
            &self.profiling,
        );
        if let Some(lease) = source.and_then(|source| source.lease) {
            self.queue.on_submitted_work_done(move || drop(lease));
        }
        if let Some((deadline, period)) = timing {
            self.schedule.prepare = prepare_start.unwrap().elapsed();
            schedule::wait_until(deadline);
            if Instant::now() >= deadline + period {
                self.profiling.count("presentation.expired_preparations", 1);
                return Ok(None);
            }
        }
        if let Some(frame) = frame {
            frame.present();
            let presented = Instant::now();
            return Ok(Some(presented));
        }
        Ok(None)
    }
}

enum Request {
    Present(Option<Scanout>, Option<wgpu::TextureView>),
    Sources(Arc<Mutex<Mailbox>>),
    Resize(u32, u32),
    Reconfigure(wgpu::PresentMode, Option<wgpu::TextureFormat>, Compositor),
    Barrier,
    SetHook(SendHook),
    TakeHook,
    #[cfg(feature = "profiling")]
    Profiling(crate::profiling::Recorder),
    #[cfg(any(test, feature = "debug-ui"))]
    Take,
    #[cfg(any(test, feature = "debug-ui"))]
    Restore(Box<Presentation>, Option<Box<dyn std::any::Any + Send>>),
}

enum Reply {
    Done,
    Hook(Option<SendHook>),
    #[cfg(any(test, feature = "debug-ui"))]
    Presentation(Box<Presentation>),
}

fn handle(slot: &mut Option<Box<Presentation>>, request: Request) -> Result<Reply, PresentError> {
    #[cfg(any(test, feature = "debug-ui"))]
    let request = match request {
        Request::Take => {
            return slot
                .take()
                .map(Reply::Presentation)
                .ok_or(PresentError::PresenterStopped)
        }
        Request::Restore(state, panic) => {
            *slot = Some(state);
            if let Some(panic) = panic {
                std::panic::resume_unwind(panic);
            }
            if slot.as_ref().unwrap().needs_configure {
                slot.as_mut().unwrap().configure();
            }
            return Ok(Reply::Done);
        }
        request => request,
    };
    let state = slot.as_mut().ok_or(PresentError::PresenterStopped)?;
    let result = match request {
        Request::Present(source, target) => state
            .present(source, target, |_, _| {})
            .map(|_| Reply::Done),
        Request::Sources(mailbox) => {
            let (sources, discontinuity) = {
                let mut mailbox = mailbox.lock().unwrap();
                mailbox.queued = false;
                let discontinuity = std::mem::take(&mut mailbox.discontinuity);
                (
                    [mailbox.sources.pop_front(), mailbox.sources.pop_front()],
                    discontinuity,
                )
            };
            if discontinuity {
                if let Some(latest) = &mut state.schedule.latest {
                    latest.source = None;
                }
            }
            for source in sources.into_iter().flatten() {
                state.source(source);
            }
            Ok(Reply::Done)
        }
        Request::Resize(width, height) => {
            state.config.width = width;
            state.config.height = height;
            state.configure();
            Ok(Reply::Done)
        }
        Request::Reconfigure(mode, format, compositor) => {
            state.clear_sources();
            let config = &mut state.config;
            config.present_mode = mode;
            if let Some(format) = format {
                config.format = format;
                config.view_formats.clear();
            }
            state.format = crate::render_format_of(config);
            state.compositor = compositor;
            state.configure();
            Ok(Reply::Done)
        }
        Request::Barrier => {
            state.clear_sources();
            Ok(Reply::Done)
        }
        Request::SetHook(hook) => {
            state.take_hook();
            state.hook = Some(hook);
            state
                .hook
                .as_mut()
                .unwrap()
                .init(&state.device, &state.queue, state.format);
            Ok(Reply::Done)
        }
        Request::TakeHook => Ok(Reply::Hook(state.take_hook())),
        #[cfg(feature = "profiling")]
        Request::Profiling(recorder) => {
            state.profiling = recorder;
            Ok(Reply::Done)
        }
        #[cfg(any(test, feature = "debug-ui"))]
        Request::Take | Request::Restore(..) => unreachable!(),
    };
    if state.needs_configure {
        state.configure();
    }
    result
}

pub(crate) struct Presenter {
    worker: Worker<Request, Reply>,
    mailbox: Arc<Mutex<Mailbox>>,
}

impl Presenter {
    pub(crate) fn new(
        surface: wgpu::Surface<'static>,
        config: wgpu::SurfaceConfiguration,
        compositor: Compositor,
        device: wgpu::Device,
        queue: wgpu::Queue,
        configuration: Arc<Mutex<()>>,
    ) -> Self {
        let format = crate::render_format_of(&config);
        Self::spawn(Presentation {
            surface: Box::new(surface),
            config,
            compositor,
            hook: None,
            device,
            queue,
            format,
            needs_configure: false,
            profiling: Default::default(),
            configuration,
            schedule: Default::default(),
        })
    }

    fn spawn(state: Presentation) -> Self {
        let worker = Worker::spawn_scheduled(
            move || {
                let mut state = state;
                if state.needs_configure {
                    state.configure();
                }
                Some(Box::new(state))
            },
            |state| {
                if let Some(state) = state {
                    state.take_hook();
                }
            },
            |state| state.as_deref().and_then(Presentation::deadline),
            |state| {
                if let Some(state) = state {
                    state.tick();
                }
            },
            handle,
        );
        let presenter = Self {
            worker,
            mailbox: Arc::new(Mutex::new(Mailbox {
                sources: VecDeque::with_capacity(2),
                queued: false,
                discontinuity: false,
            })),
        };
        let _ = presenter.barrier();
        presenter
    }

    pub(crate) fn scanout(&self, pending: PendingScanout) -> Result<(), PresentError> {
        let mut mailbox = self.mailbox.lock().unwrap();
        if mailbox.sources.len() == 2 {
            mailbox.sources.pop_front();
            mailbox.discontinuity = true;
        }
        mailbox.sources.push_back(pending);
        if !mailbox.queued {
            mailbox.queued = true;
            drop(mailbox);
            self.worker.post(Request::Sources(self.mailbox.clone()))?;
        } else if let Some(error) = self.error() {
            return Err(error);
        }
        Ok(())
    }

    pub(crate) fn present(
        &self,
        source: Option<Scanout>,
        target: Option<wgpu::TextureView>,
    ) -> Result<(), PresentError> {
        self.worker
            .request(Request::Present(source, target))
            .map(|_| ())
    }

    #[cfg(any(test, feature = "debug-ui"))]
    pub(crate) fn synchronous<R>(
        &self,
        operation: impl FnOnce(&mut Presentation) -> R,
    ) -> Result<R, PresentError> {
        let Reply::Presentation(mut state) = self.worker.request(Request::Take)? else {
            unreachable!()
        };
        let (result, panic) = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            operation(&mut state)
        })) {
            Ok(result) => (Some(result), None),
            Err(panic) => (None, Some(panic)),
        };
        self.worker.request(Request::Restore(state, panic))?;
        Ok(result.unwrap())
    }

    pub(crate) fn resize(&self, width: u32, height: u32) -> Result<(), PresentError> {
        self.worker
            .request(Request::Resize(width, height))
            .map(|_| ())
    }

    pub(crate) fn reconfigure(
        &self,
        config: &RendererConfig,
        compositor: Compositor,
    ) -> Result<(), PresentError> {
        self.mailbox.lock().unwrap().sources.clear();
        self.worker
            .request(Request::Reconfigure(
                config.present_mode,
                config.format,
                compositor,
            ))
            .map(|_| ())
    }

    pub(crate) fn barrier(&self) -> Result<(), PresentError> {
        self.mailbox.lock().unwrap().sources.clear();
        self.worker.request(Request::Barrier).map(|_| ())
    }

    pub(crate) fn set_hook(&self, hook: SendHook) -> Result<(), PresentError> {
        self.worker.request(Request::SetHook(hook)).map(|_| ())
    }

    pub(crate) fn take_hook(&self) -> Result<Option<SendHook>, PresentError> {
        let Reply::Hook(hook) = self.worker.request(Request::TakeHook)? else {
            unreachable!()
        };
        Ok(hook)
    }

    #[cfg(feature = "profiling")]
    pub(crate) fn set_profiling(
        &self,
        recorder: crate::profiling::Recorder,
    ) -> Result<(), PresentError> {
        self.worker
            .request(Request::Profiling(recorder))
            .map(|_| ())
    }

    pub(crate) fn error(&self) -> Option<PresentError> {
        self.worker.error()
    }

    pub(crate) fn shutdown(&mut self) {
        self.mailbox.lock().unwrap().sources.clear();
        self.worker.shutdown();
    }
}

#[cfg(test)]
mod tests;
