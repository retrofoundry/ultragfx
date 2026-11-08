use std::collections::VecDeque;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

#[derive(Default)]
pub(crate) struct InFlight(VecDeque<Arc<AtomicBool>>);

impl InFlight {
    pub(crate) fn begin(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) -> Completion {
        while self.0.len() >= 2 {
            if self.0.front().unwrap().load(Ordering::Acquire) {
                self.0.pop_front();
            } else {
                device
                    .poll(wgpu::PollType::Poll)
                    .expect("poll canonical submissions");
                std::thread::yield_now();
            }
        }
        let done = Arc::new(AtomicBool::new(false));
        self.0.push_back(done.clone());
        Completion {
            queue: queue.clone(),
            done,
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
            returned.send(in_flight.0.len()).unwrap();
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
