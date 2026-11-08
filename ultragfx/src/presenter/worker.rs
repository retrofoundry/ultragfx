use std::sync::{mpsc, Arc, Mutex};
use std::thread::JoinHandle;

use crate::PresentError;

type Job<S> = Box<dyn FnOnce(&mut S) -> bool + Send>;

pub(super) struct Worker<S> {
    sender: Option<mpsc::Sender<Job<S>>>,
    failure: Arc<Mutex<Option<PresentError>>>,
    join: Option<JoinHandle<()>>,
}

fn panic_error(payload: Box<dyn std::any::Any + Send>) -> PresentError {
    let message = payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| {
            payload
                .downcast_ref::<&str>()
                .map(|message| (*message).to_owned())
        })
        .unwrap_or_else(|| "present thread panicked".into());
    PresentError::PresenterFailed(message)
}

impl<S: Send + 'static> Worker<S> {
    pub(super) fn spawn(init: impl FnOnce() -> S + Send + 'static, finish: fn(&mut S)) -> Self {
        let (sender, receiver) = mpsc::channel::<Job<S>>();
        let failure = Arc::new(Mutex::new(None));
        let status = failure.clone();
        let join = std::thread::Builder::new()
            .name("ultragfx-present".into())
            .spawn(move || {
                let initialized = std::panic::catch_unwind(std::panic::AssertUnwindSafe(init));
                let mut state = match initialized {
                    Ok(state) => state,
                    Err(payload) => {
                        *status.lock().unwrap() = Some(panic_error(payload));
                        return;
                    }
                };
                while let Ok(job) = receiver.recv() {
                    if !job(&mut state) {
                        break;
                    }
                }
                if let Err(payload) =
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| finish(&mut state)))
                {
                    *status.lock().unwrap() = Some(panic_error(payload));
                }
            });
        let join = match join {
            Ok(join) => Some(join),
            Err(error) => {
                *failure.lock().unwrap() = Some(PresentError::PresenterFailed(error.to_string()));
                None
            }
        };
        Self {
            sender: Some(sender),
            failure,
            join,
        }
    }

    pub(super) fn request<R: Send + 'static>(
        &self,
        operation: impl FnOnce(&mut S) -> Result<R, PresentError> + Send + 'static,
    ) -> Result<R, PresentError> {
        if let Some(error) = self.error() {
            return Err(error);
        }
        let (reply, receiver) = mpsc::channel();
        let failure = self.failure.clone();
        let job = Box::new(move |state: &mut S| {
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| operation(state))) {
                Ok(result) => {
                    let _ = reply.send(result);
                    true
                }
                Err(payload) => {
                    let error = panic_error(payload);
                    *failure.lock().unwrap() = Some(error.clone());
                    let _ = reply.send(Err(error));
                    false
                }
            }
        });
        self.sender
            .as_ref()
            .ok_or(PresentError::PresenterStopped)?
            .send(job)
            .map_err(|_| self.error().unwrap_or(PresentError::PresenterStopped))?;
        receiver
            .recv()
            .unwrap_or_else(|_| Err(self.error().unwrap_or(PresentError::PresenterStopped)))
    }
}

impl<S> Worker<S> {
    pub(super) fn error(&self) -> Option<PresentError> {
        self.failure.lock().unwrap().clone()
    }

    pub(super) fn shutdown(&mut self) {
        self.sender.take();
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

impl<S> Drop for Worker<S> {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    #[test]
    fn requests_wait_for_completion_and_preserve_order() {
        let worker = Arc::new(Worker::spawn(Vec::<u32>::new, |_| {}));
        let (entered, acquired) = mpsc::channel();
        let (release, wait) = mpsc::channel();
        let progress = Arc::new(AtomicUsize::new(0));
        let caller = {
            let worker = worker.clone();
            let progress = progress.clone();
            std::thread::spawn(move || {
                worker
                    .request(move |frames| {
                        entered.send(()).unwrap();
                        wait.recv().unwrap();
                        frames.push(1);
                        Ok(())
                    })
                    .unwrap();
                progress.fetch_add(1, Ordering::SeqCst);
                worker
                    .request(|frames| {
                        frames.push(2);
                        Ok(())
                    })
                    .unwrap();
            })
        };
        acquired.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(progress.load(Ordering::SeqCst), 0);
        release.send(()).unwrap();
        caller.join().unwrap();
        assert_eq!(worker.request(|frames| Ok(frames.clone())).unwrap(), [1, 2]);
    }

    #[test]
    fn panic_is_latched_and_finish_runs_on_the_worker() {
        let (finished, done) = mpsc::channel();
        let worker = Worker::spawn(
            move || finished,
            |finished| {
                finished.send(std::thread::current().id()).unwrap();
            },
        );
        let thread = worker.request(|_| Ok(std::thread::current().id())).unwrap();
        assert_ne!(thread, std::thread::current().id());
        assert!(matches!(
            worker.request::<()>(|_| panic!("draw failed")),
            Err(PresentError::PresenterFailed(_))
        ));
        for _ in 0..10 {
            assert!(matches!(
                worker.request(|_| Ok(())),
                Err(PresentError::PresenterFailed(_))
            ));
        }
        assert_eq!(done.recv_timeout(Duration::from_secs(5)).unwrap(), thread);
    }
}
