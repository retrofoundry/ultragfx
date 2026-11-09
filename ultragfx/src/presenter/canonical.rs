use std::collections::VecDeque;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

#[derive(Default)]
pub(crate) struct InFlight {
    pending: VecDeque<Arc<AtomicBool>>,
}

impl InFlight {
    pub(crate) fn begin(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) -> Completion {
        self.wait_for_slot(|| {
            let _ = device.poll(wgpu::PollType::Poll);
        });
        let done = Arc::new(AtomicBool::new(false));
        self.pending.push_back(done.clone());
        Completion {
            queue: queue.clone(),
            done,
        }
    }

    fn wait_for_slot(&mut self, mut poll: impl FnMut()) {
        while self.pending.len() >= 2 {
            if self.pending.front().unwrap().load(Ordering::Acquire) {
                self.pending.pop_front();
            } else {
                poll();
                if !self.pending.front().unwrap().load(Ordering::Acquire) {
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
            }
        }
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
    fn incomplete_submissions_back_off_between_nonblocking_polls() {
        let first = Arc::new(AtomicBool::new(false));
        let mut in_flight = InFlight {
            pending: VecDeque::from([first.clone(), Arc::new(AtomicBool::new(false))]),
        };
        let start = std::time::Instant::now();
        let mut polls = 0;
        in_flight.wait_for_slot(|| {
            polls += 1;
            if polls == 4 {
                first.store(true, Ordering::Release);
            }
        });
        assert!(start.elapsed() >= Duration::from_millis(3));
        assert_eq!(in_flight.pending.len(), 1);
    }

    #[test]
    fn a_third_submission_waits_until_one_of_the_first_two_completes() {
        let (device, queue, _) = crate::render::headless_device();
        let mut in_flight = InFlight::default();
        let first = in_flight.begin(&device, &queue);
        let second = in_flight.begin(&device, &queue);
        let (entered, starting) = mpsc::channel();
        let (returned, finished) = mpsc::channel();
        let caller = std::thread::spawn(move || {
            entered.send(()).unwrap();
            let third = in_flight.begin(&device, &queue);
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
