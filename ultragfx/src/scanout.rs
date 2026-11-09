use crate::{
    interpolation::{SourceFrame, SourceScanout, TaskHandle},
    render::Scanout,
    PresentError, Renderer,
};
use std::time::Instant;

#[derive(Clone, Copy, Debug)]
pub enum ScanoutOrigin {
    Vi(Option<crate::ViRegisters>),
    Host(Option<u64>),
}

pub(crate) struct PendingScanout {
    pub(crate) image: Option<Scanout>,
    pub(crate) source: Option<SourceFrame>,
    pub(crate) policy: crate::refresh::RefreshPolicy,
    pub(crate) fit: crate::PresentationFit,
}

impl Renderer {
    /// Publish one native VI swap. Original acknowledges presentation; other modes acknowledge
    /// acceptance and report fatal worker failures through `presenter_error` and later calls.
    /// `selected` must belong to `tasks` and this renderer. Ineligible tasks never suppress the image.
    pub fn scanout(
        &mut self,
        origin: ScanoutOrigin,
        visible_at: Instant,
        tasks: &[TaskHandle],
        selected: Option<&TaskHandle>,
    ) -> Result<(), PresentError> {
        let source = match origin {
            ScanoutOrigin::Vi(vi) => self.scanout_source(vi),
            ScanoutOrigin::Host(address) => self.host_scanout_source(address),
        };
        self.mark_mutation();
        self.scanout_resolved(source, visible_at, tasks, selected)
    }

    pub(crate) fn prepare_scanout(
        &mut self,
        resolved: Option<crate::ScanoutSource>,
        visible_at: Instant,
        tasks: &[TaskHandle],
        selected: Option<&TaskHandle>,
    ) -> PendingScanout {
        let retained = resolved.as_ref().and_then(|resolved| {
            let crate::render::workload::TargetId::Guest(address) = resolved.target else {
                return None;
            };
            let selected =
                selected.filter(|selected| tasks.iter().any(|task| task.same_task(selected)))?;
            if tasks
                .iter()
                .filter(|task| task.info().cookie == selected.info().cookie)
                .count()
                != 1
            {
                return None;
            }
            self.commit_source(
                tasks,
                SourceScanout {
                    task_cookie: selected.info().cookie,
                    address,
                    rows: resolved.rows.clone(),
                    visible_at,
                },
            )
            .ok()
            .filter(|source| self.source_is_current(source))
        });
        let mut image = resolved.map(|resolved| {
            self.inner
                .resolve_scanout(&self.device, resolved.target, resolved.rows, false)
        });
        if let Some(image) = &mut image {
            if let Some(source) = &retained {
                self.inner.compositor().replace_texture(
                    &self.device,
                    image,
                    source.image().texture.clone(),
                );
            } else {
                self.inner
                    .snapshot_scanout(&self.device, &self.queue, image);
            }
        }
        PendingScanout {
            image,
            source: retained.filter(|source| source.ineligibility().is_empty()),
            policy: self.refresh_latched,
            fit: self.presentation_fit,
        }
    }

    pub(crate) fn scanout_resolved(
        &mut self,
        source: Option<crate::ScanoutSource>,
        visible_at: Instant,
        tasks: &[TaskHandle],
        selected: Option<&TaskHandle>,
    ) -> Result<(), PresentError> {
        let presenter = self
            .presenter
            .as_ref()
            .ok_or(PresentError::SurfaceRequired)?;
        let policy = self.refresh_requested;
        #[cfg(feature = "debug-ui")]
        let policy = if self.debugger.enabled {
            crate::refresh::RefreshPolicy {
                requested: crate::RefreshRate::Original,
                ..policy
            }
        } else {
            policy
        };
        let was_active = self.interpolation_active();
        self.refresh_latched = policy;
        if !self.interpolation_active() {
            if was_active {
                presenter.barrier()?;
            }
            return self.threaded_present(source, None, false);
        }
        let pending = {
            let _completion = self.in_flight.begin(&self.device, &self.queue);
            let configuration = self.configuration.clone();
            let _configuration = configuration
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            self.prepare_scanout(source, visible_at, tasks, selected)
        };
        self.presenter.as_ref().unwrap().scanout(pending)
    }
}
