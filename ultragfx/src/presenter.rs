mod worker;

use crate::render::{Compositor, Scanout};
use crate::{PresentError, RenderHook, RendererConfig};
use worker::Worker;

trait Surface: Send {
    fn configure(&self, device: &wgpu::Device, config: &wgpu::SurfaceConfiguration);
    fn acquire(&self) -> Result<Option<SurfaceFrame>, PresentError>;
}

enum SurfaceFrame {
    Native(wgpu::SurfaceTexture),
    #[cfg(test)]
    Offscreen(wgpu::Texture),
}

impl SurfaceFrame {
    fn texture(&self) -> &wgpu::Texture {
        match self {
            Self::Native(frame) => &frame.texture,
            #[cfg(test)]
            Self::Offscreen(texture) => texture,
        }
    }
    fn present(self) {
        match self {
            Self::Native(frame) => frame.present(),
            #[cfg(test)]
            Self::Offscreen(_) => {}
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
}

impl Presentation {
    fn configure(&mut self) {
        self.surface.configure(&self.device, &self.config);
        self.needs_configure = false;
    }

    fn take_hook(&mut self) -> Option<SendHook> {
        let mut hook = self.hook.take()?;
        hook.deinit();
        Some(hook)
    }

    pub(crate) fn present(
        &mut self,
        source: Option<Scanout>,
        target: Option<wgpu::TextureView>,
        after_hook: impl FnOnce(&mut wgpu::CommandEncoder, &wgpu::TextureView),
    ) -> Result<(), PresentError> {
        let _span = self.profiling.span("presentation");
        let frame = if target.is_none() {
            match self.surface.acquire() {
                Ok(Some(frame)) => Some(frame),
                Ok(None) => return Ok(()),
                Err(PresentError::SurfaceLost) => {
                    self.needs_configure = true;
                    return Err(PresentError::SurfaceLost);
                }
                Err(error) => return Err(error),
            }
        } else {
            None
        };
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
        if let Some(frame) = frame {
            frame.present();
        }
        Ok(())
    }
}

pub(crate) struct Presenter {
    worker: Worker<Option<Presentation>>,
}

impl Presenter {
    pub(crate) fn new(
        surface: wgpu::Surface<'static>,
        config: wgpu::SurfaceConfiguration,
        compositor: Compositor,
        device: wgpu::Device,
        queue: wgpu::Queue,
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
        })
    }

    fn spawn(state: Presentation) -> Self {
        let worker = Worker::spawn(
            move || {
                let mut state = state;
                if state.needs_configure {
                    state.configure();
                }
                Some(state)
            },
            |state| {
                if let Some(state) = state {
                    state.take_hook();
                }
            },
        );
        let presenter = Self { worker };
        let _ = presenter.barrier();
        presenter
    }

    fn request<R: Send + 'static>(
        &self,
        operation: impl FnOnce(&mut Presentation) -> Result<R, PresentError> + Send + 'static,
    ) -> Result<R, PresentError> {
        self.worker.request(move |state| {
            let state = state.as_mut().ok_or(PresentError::PresenterStopped)?;
            let result = operation(state);
            if state.needs_configure {
                state.configure();
            }
            result
        })
    }

    pub(crate) fn present(
        &self,
        source: Option<Scanout>,
        target: Option<wgpu::TextureView>,
    ) -> Result<(), PresentError> {
        self.request(move |state| state.present(source, target, |_, _| {}))
    }

    #[cfg(feature = "debug-ui")]
    pub(crate) fn synchronous<R: Send + 'static>(
        &self,
        operation: impl FnOnce(&mut Presentation) -> R,
    ) -> Result<R, PresentError> {
        let mut state = self
            .worker
            .request(|state| state.take().ok_or(PresentError::PresenterStopped))?;
        let result =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| operation(&mut state)));
        self.worker.request(move |slot| {
            *slot = Some(state);
            let result = match result {
                Ok(result) => result,
                Err(panic) => std::panic::resume_unwind(panic),
            };
            if slot.as_ref().unwrap().needs_configure {
                slot.as_mut().unwrap().configure();
            }
            Ok(result)
        })
    }

    pub(crate) fn resize(&self, width: u32, height: u32) -> Result<(), PresentError> {
        self.request(move |state| {
            state.config.width = width;
            state.config.height = height;
            state.configure();
            Ok(())
        })
    }

    pub(crate) fn reconfigure(
        &self,
        config: &RendererConfig,
        compositor: Compositor,
    ) -> Result<(), PresentError> {
        let (mode, format) = (config.present_mode, config.format);
        self.request(move |state| {
            let config = &mut state.config;
            config.present_mode = mode;
            if let Some(format) = format {
                config.format = format;
                config.view_formats.clear();
            }
            state.format = crate::render_format_of(config);
            state.compositor = compositor;
            state.configure();
            Ok(())
        })
    }

    pub(crate) fn barrier(&self) -> Result<(), PresentError> {
        self.request(|_| Ok(()))
    }

    pub(crate) fn set_hook(&self, hook: SendHook) -> Result<(), PresentError> {
        self.request(move |state| {
            state.take_hook();
            state.hook = Some(hook);
            state
                .hook
                .as_mut()
                .unwrap()
                .init(&state.device, &state.queue, state.format);
            Ok(())
        })
    }

    pub(crate) fn take_hook(&self) -> Result<Option<SendHook>, PresentError> {
        self.request(|state| Ok(state.take_hook()))
    }

    #[cfg(feature = "profiling")]
    pub(crate) fn set_profiling(
        &self,
        recorder: crate::profiling::Recorder,
    ) -> Result<(), PresentError> {
        self.request(move |state| {
            state.profiling = recorder;
            Ok(())
        })
    }

    pub(crate) fn error(&self) -> Option<PresentError> {
        self.worker.error()
    }

    pub(crate) fn shutdown(&mut self) {
        self.worker.shutdown();
    }
}

#[cfg(test)]
mod tests;
