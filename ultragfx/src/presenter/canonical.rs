use std::collections::VecDeque;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

#[derive(Default)]
pub(crate) struct InFlight {
    pending: VecDeque<Arc<AtomicBool>>,
    failure: Option<crate::PresentError>,
}

impl InFlight {
    pub(crate) fn begin(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> Result<Completion, crate::PresentError> {
        self.wait_for_slot(|| device.poll(wgpu::PollType::Poll))?;
        let done = Arc::new(AtomicBool::new(false));
        self.pending.push_back(done.clone());
        Ok(Completion {
            queue: queue.clone(),
            done,
        })
    }

    pub(crate) fn error(&self) -> Option<crate::PresentError> {
        self.failure.clone()
    }

    fn wait_for_slot(
        &mut self,
        mut poll: impl FnMut() -> Result<wgpu::PollStatus, wgpu::PollError>,
    ) -> Result<(), crate::PresentError> {
        if let Some(error) = self.error() {
            return Err(error);
        }
        while self.pending.len() >= 2 {
            if self.pending.front().unwrap().load(Ordering::Acquire) {
                self.pending.pop_front();
            } else {
                if poll().is_err() {
                    self.failure = Some(crate::PresentError::DeviceLost);
                    return Err(crate::PresentError::DeviceLost);
                }
                if !self.pending.front().unwrap().load(Ordering::Acquire) {
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
            }
        }
        Ok(())
    }
}

pub(crate) struct Completion {
    queue: wgpu::Queue,
    done: Arc<AtomicBool>,
}

impl Drop for Completion {
    fn drop(&mut self) {
        let done = self.done.clone();
        self.queue
            .on_submitted_work_done(move || done.store(true, Ordering::Release));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn poll_failure_is_returned_and_remembered_without_panicking() {
        let mut in_flight = InFlight {
            pending: (0..2).map(|_| Arc::new(AtomicBool::new(false))).collect(),
            failure: None,
        };
        assert_eq!(
            in_flight.wait_for_slot(|| Err(wgpu::PollError::Timeout)),
            Err(crate::PresentError::DeviceLost)
        );
        assert_eq!(in_flight.error(), Some(crate::PresentError::DeviceLost));
        assert_eq!(
            in_flight.wait_for_slot(|| panic!("polled after failure")),
            Err(crate::PresentError::DeviceLost)
        );
        assert_eq!(in_flight.pending.len(), 2);
    }

    #[test]
    fn incomplete_submissions_back_off_between_nonblocking_polls() {
        let first = Arc::new(AtomicBool::new(false));
        let mut in_flight = InFlight {
            pending: VecDeque::from([first.clone(), Arc::new(AtomicBool::new(false))]),
            failure: None,
        };
        let start = std::time::Instant::now();
        let mut polls = 0;
        in_flight
            .wait_for_slot(|| {
                polls += 1;
                if polls == 4 {
                    first.store(true, Ordering::Release);
                }
                Ok(wgpu::PollStatus::Poll)
            })
            .unwrap();
        assert!(start.elapsed() >= Duration::from_millis(3));
        assert_eq!(in_flight.pending.len(), 1);
    }

    #[test]
    fn canonical_poll_failure_rejects_tasks_and_scanouts() {
        let mut h = super::super::tests::harness(wgpu::TextureFormat::Rgba8Unorm, vec![], None);
        h.renderer
            .set_refresh_rate(crate::RefreshRate::Display)
            .unwrap();
        h.renderer
            .scanout(
                crate::ScanoutOrigin::Host(None),
                std::time::Instant::now(),
                &[],
                None,
            )
            .unwrap();
        h.renderer.in_flight.pending = (0..2).map(|_| Arc::new(AtomicBool::new(false))).collect();
        assert_eq!(
            h.renderer
                .in_flight
                .wait_for_slot(|| Err(wgpu::PollError::Timeout)),
            Err(crate::PresentError::DeviceLost)
        );
        assert_eq!(
            h.renderer.presenter_error(),
            Some(crate::PresentError::DeviceLost)
        );
        let summary = h.renderer.process_dl_memory_observed(
            crate::RdramImage::new(&[]),
            0,
            crate::Microcode::F3dex2,
            &mut crate::NopSink,
            None,
            crate::Submission::DiscardOnStop,
            &[],
            None,
        );
        assert_eq!(summary.errors, 1);
        assert_eq!(
            summary.termination,
            crate::inspect::WalkTermination::Rejected
        );
        assert!(h.renderer.frame_scenes.is_empty());
        assert_eq!(
            h.renderer.scanout(
                crate::ScanoutOrigin::Host(None),
                std::time::Instant::now(),
                &[],
                None
            ),
            Err(crate::PresentError::DeviceLost)
        );
    }

    #[test]
    fn a_third_submission_waits_until_one_of_the_first_two_completes() {
        let (device, queue, _) = crate::render::headless_device();
        let mut in_flight = InFlight::default();
        let first = in_flight.begin(&device, &queue).unwrap();
        let second = in_flight.begin(&device, &queue).unwrap();
        let (entered, starting) = mpsc::channel();
        let (returned, finished) = mpsc::channel();
        let caller = std::thread::spawn(move || {
            entered.send(()).unwrap();
            let third = in_flight.begin(&device, &queue).unwrap();
            returned.send(in_flight.pending.len()).unwrap();
            drop(third);
        });
        starting.recv().unwrap();
        assert_eq!(
            finished.recv_timeout(Duration::from_millis(30)),
            Err(mpsc::RecvTimeoutError::Timeout)
        );
        drop(first);
        assert_eq!(finished.recv_timeout(Duration::from_secs(3)).unwrap(), 2);
        drop(second);
        caller.join().unwrap();
    }
}
