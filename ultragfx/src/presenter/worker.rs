use std::sync::{mpsc, Arc, Condvar, Mutex};
use std::thread::JoinHandle;

use crate::PresentError;

enum Message<Q> {
    Request(Q),
    Post(Q),
}

struct Reply<R> {
    result: Option<Result<R, PresentError>>,
    failure: Option<PresentError>,
    stopped: bool,
}

pub(super) struct Worker<Q, R> {
    sender: Option<mpsc::SyncSender<Message<Q>>>,
    reply: Arc<(Mutex<Reply<R>>, Condvar)>,
    requester: Mutex<()>,
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

impl<Q: Send + 'static, R: Send + 'static> Worker<Q, R> {
    pub(super) fn spawn_scheduled<S: Send + 'static>(
        init: impl FnOnce() -> S + Send + 'static,
        finish: fn(&mut S),
        deadline: fn(&S) -> Option<std::time::Instant>,
        tick: fn(&mut S),
        handle: fn(&mut S, Q) -> Result<R, PresentError>,
    ) -> Self {
        let (sender, receiver) = mpsc::sync_channel::<Message<Q>>(128);
        let reply = Arc::new((
            Mutex::new(Reply {
                result: None,
                failure: None,
                stopped: false,
            }),
            Condvar::new(),
        ));
        let status = reply.clone();
        let join = std::thread::Builder::new()
            .name("ultragfx-present".into())
            .spawn(move || {
                let initialized = std::panic::catch_unwind(std::panic::AssertUnwindSafe(init));
                let mut state = match initialized {
                    Ok(state) => state,
                    Err(payload) => {
                        let mut reply = status.0.lock().unwrap();
                        reply.failure = Some(panic_error(payload));
                        reply.stopped = true;
                        status.1.notify_all();
                        return;
                    }
                };
                let dispatch = |state: &mut S, message| match message {
                    Message::Request(request) => {
                        let result = handle(state, request);
                        status.0.lock().unwrap().result = Some(result);
                        status.1.notify_one();
                        Ok(())
                    }
                    Message::Post(request) => handle(state, request).map(|_| ()),
                };
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    loop {
                        let received = if let Some(at) = deadline(&state) {
                            let remaining = at.saturating_duration_since(std::time::Instant::now());
                            if remaining > super::schedule::SPIN_WINDOW {
                                receiver.recv_timeout(remaining / 2)
                            } else {
                                match receiver.try_recv() {
                                    Ok(request) => Ok(request),
                                    Err(mpsc::TryRecvError::Disconnected) => {
                                        Err(mpsc::RecvTimeoutError::Disconnected)
                                    }
                                    Err(mpsc::TryRecvError::Empty) => {
                                        super::schedule::wait_until(at);
                                        Err(mpsc::RecvTimeoutError::Timeout)
                                    }
                                }
                            }
                        } else {
                            receiver
                                .recv()
                                .map_err(|_| mpsc::RecvTimeoutError::Disconnected)
                        };
                        match received {
                            Ok(request) => dispatch(&mut state, request)?,
                            Err(mpsc::RecvTimeoutError::Disconnected) => break,
                            Err(mpsc::RecvTimeoutError::Timeout) => {}
                        }
                        for _ in 0..15 {
                            match receiver.try_recv() {
                                Ok(request) => dispatch(&mut state, request)?,
                                Err(mpsc::TryRecvError::Disconnected) => return Ok(()),
                                Err(mpsc::TryRecvError::Empty) => break,
                            }
                        }
                        if deadline(&state).is_some_and(|at| at <= std::time::Instant::now()) {
                            tick(&mut state);
                        }
                    }
                    Ok(())
                }));
                {
                    let mut reply = status.0.lock().unwrap();
                    reply.failure = match result {
                        Ok(Ok(())) => None,
                        Ok(Err(error)) => Some(error),
                        Err(payload) => Some(panic_error(payload)),
                    };
                    reply.stopped = true;
                    status.1.notify_all();
                }
                drop(receiver);
                if let Err(payload) =
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| finish(&mut state)))
                {
                    let mut reply = status.0.lock().unwrap();
                    if reply.failure.is_none() {
                        reply.failure = Some(panic_error(payload));
                    }
                }
            });
        let join = match join {
            Ok(join) => Some(join),
            Err(error) => {
                let mut status = reply.0.lock().unwrap();
                status.failure = Some(PresentError::PresenterFailed(error.to_string()));
                status.stopped = true;
                None
            }
        };
        Self {
            sender: Some(sender),
            reply,
            requester: Mutex::new(()),
            join,
        }
    }

    pub(super) fn post(&self, request: Q) -> Result<(), PresentError> {
        self.send(Message::Post(request))
    }

    fn send(&self, message: Message<Q>) -> Result<(), PresentError> {
        {
            let reply = self.reply.0.lock().unwrap();
            if let Some(error) = &reply.failure {
                return Err(error.clone());
            }
            if reply.stopped {
                return Err(PresentError::PresenterStopped);
            }
        }
        self.sender
            .as_ref()
            .ok_or(PresentError::PresenterStopped)?
            .send(message)
            .map_err(|_| self.error().unwrap_or(PresentError::PresenterStopped))
    }

    pub(super) fn request(&self, request: Q) -> Result<R, PresentError> {
        let _requester = self.requester.lock().unwrap();
        self.send(Message::Request(request))?;
        let mut reply = self.reply.0.lock().unwrap();
        loop {
            if let Some(result) = reply.result.take() {
                return result;
            }
            if reply.stopped {
                return Err(reply
                    .failure
                    .clone()
                    .unwrap_or(PresentError::PresenterStopped));
            }
            reply = self.reply.1.wait(reply).unwrap();
        }
    }
}

impl<Q, R> Worker<Q, R> {
    pub(super) fn error(&self) -> Option<PresentError> {
        self.reply.0.lock().unwrap().failure.clone()
    }

    pub(super) fn shutdown(&mut self) {
        self.sender.take();
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

impl<Q, R> Drop for Worker<Q, R> {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

    enum Request {
        Add(u32),
        Read,
        Thread,
        Panic,
        Block(mpsc::Sender<()>, mpsc::Receiver<()>),
    }
    #[derive(Debug, PartialEq)]
    enum Response {
        Count(u32),
        Thread(std::thread::ThreadId),
    }
    struct State {
        count: u32,
        next: Option<Instant>,
        frames: Option<mpsc::Sender<u32>>,
        finished: Option<mpsc::Sender<std::thread::ThreadId>>,
    }
    fn handle(state: &mut State, request: Request) -> Result<Response, PresentError> {
        match request {
            Request::Add(value) => state.count = state.count * 10 + value,
            Request::Read => {}
            Request::Thread => return Ok(Response::Thread(std::thread::current().id())),
            Request::Panic => panic!("draw failed"),
            Request::Block(entered, release) => {
                entered.send(()).unwrap();
                release.recv().unwrap();
                state.count = state.count * 10 + 1;
            }
        }
        Ok(Response::Count(state.count))
    }
    fn spawn(finished: Option<mpsc::Sender<std::thread::ThreadId>>) -> Worker<Request, Response> {
        Worker::spawn_scheduled(
            move || State {
                count: 0,
                next: None,
                frames: None,
                finished,
            },
            |state| {
                if let Some(finished) = &state.finished {
                    finished.send(std::thread::current().id()).unwrap();
                }
            },
            |state| state.next,
            |_| {},
            handle,
        )
    }

    #[test]
    fn due_ticks_are_served_during_a_sustained_control_flood() {
        let (start, ready) = mpsc::channel();
        let (frames, received) = mpsc::channel();
        let worker = Worker::spawn_scheduled(
            move || {
                ready.recv().unwrap();
                State {
                    count: 0,
                    next: Some(Instant::now()),
                    frames: Some(frames),
                    finished: None,
                }
            },
            |_| {},
            |state| state.next,
            |state| {
                state.frames.as_ref().unwrap().send(state.count).unwrap();
                state.next = Some(Instant::now() + Duration::from_secs(1));
            },
            |state, _: ()| {
                state.count += 1;
                Ok(state.count)
            },
        );
        for _ in 0..100 {
            worker.post(()).unwrap();
        }
        start.send(()).unwrap();
        assert_eq!(received.recv_timeout(Duration::from_secs(3)).unwrap(), 16);
        assert_eq!(worker.request(()).unwrap(), 101);
    }

    #[test]
    fn requests_wait_for_completion_and_preserve_order() {
        let worker = Arc::new(spawn(None));
        let (entered, acquired) = mpsc::channel();
        let (release, wait) = mpsc::channel();
        let progress = Arc::new(AtomicUsize::new(0));
        let caller = {
            let worker = worker.clone();
            let progress = progress.clone();
            std::thread::spawn(move || {
                assert_eq!(
                    worker.request(Request::Block(entered, wait)).unwrap(),
                    Response::Count(1)
                );
                progress.fetch_add(1, Ordering::SeqCst);
                assert_eq!(
                    worker.request(Request::Add(2)).unwrap(),
                    Response::Count(12)
                );
            })
        };
        acquired.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(progress.load(Ordering::SeqCst), 0);
        release.send(()).unwrap();
        caller.join().unwrap();
        assert_eq!(worker.request(Request::Read).unwrap(), Response::Count(12));
    }

    #[test]
    fn panic_is_latched_and_finish_runs_on_the_worker() {
        let (finished, done) = mpsc::channel();
        let worker = spawn(Some(finished));
        let thread = worker.request(Request::Thread).unwrap();
        assert_ne!(thread, Response::Thread(std::thread::current().id()));
        assert!(matches!(
            worker.request(Request::Panic),
            Err(PresentError::PresenterFailed(_))
        ));
        for _ in 0..10 {
            assert!(matches!(
                worker.request(Request::Read),
                Err(PresentError::PresenterFailed(_))
            ));
        }
        assert_eq!(
            Response::Thread(done.recv_timeout(Duration::from_secs(5)).unwrap()),
            thread
        );
    }

    #[test]
    fn posted_panic_latches_failure_and_finishes_once() {
        let (finished, done) = mpsc::channel();
        let mut worker = spawn(Some(finished));
        let thread = worker.request(Request::Thread).unwrap();
        worker.post(Request::Panic).unwrap();
        assert_eq!(
            Response::Thread(done.recv_timeout(Duration::from_secs(3)).unwrap()),
            thread
        );
        assert!(matches!(
            worker.request(Request::Read),
            Err(PresentError::PresenterFailed(_))
        ));
        worker.shutdown();
        assert_eq!(done.try_recv(), Err(mpsc::TryRecvError::Disconnected));
    }

    #[test]
    fn concurrent_requesters_receive_their_own_reused_reply() {
        let worker = Arc::new(Worker::spawn_scheduled(
            || (),
            |_| {},
            |_| None,
            |_| {},
            |_, value: u32| Ok(value),
        ));
        let callers: Vec<_> = (0..8)
            .map(|id| {
                let worker = worker.clone();
                std::thread::spawn(move || {
                    for n in 0..1000 {
                        let value = id * 1000 + n;
                        assert_eq!(worker.request(value).unwrap(), value);
                    }
                })
            })
            .collect();
        for caller in callers {
            caller.join().unwrap();
        }
    }

    #[test]
    fn initialization_failure_releases_a_waiting_request() {
        let worker = Worker::spawn_scheduled(
            || panic!("init failed"),
            |_: &mut ()| {},
            |_| None,
            |_| {},
            |_, _: ()| Ok(()),
        );
        assert!(matches!(
            worker.request(()),
            Err(PresentError::PresenterFailed(_))
        ));
    }
    #[test]
    fn a_full_queue_releases_blocked_senders_before_hook_teardown() {
        let (entered, blocked) = mpsc::channel();
        let (release, resume) = mpsc::channel();
        let (finish, finished) = mpsc::channel();
        let worker = Arc::new(Worker::spawn_scheduled(
            move || (entered, resume, finished),
            |state| {
                state.2.recv().unwrap();
            },
            |_| None,
            |_| {},
            |state, fail| {
                if fail {
                    state.0.send(()).unwrap();
                    state.1.recv().unwrap();
                    panic!("dispatch failed");
                }
                Ok(())
            },
        ));
        worker.post(true).unwrap();
        blocked.recv_timeout(Duration::from_secs(3)).unwrap();
        for _ in 0..128 {
            worker.post(false).unwrap();
        }
        let (returned, result) = mpsc::channel();
        let producer = {
            let worker = worker.clone();
            let finish = finish.clone();
            std::thread::spawn(move || {
                returned.send(worker.post(false)).unwrap();
                let _ = finish.send(());
            })
        };
        assert!(matches!(
            result.recv_timeout(Duration::from_millis(20)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));
        release.send(()).unwrap();
        let result = result.recv_timeout(Duration::from_secs(1));
        let _ = finish.send(());
        producer.join().unwrap();
        assert!(matches!(result, Ok(Err(PresentError::PresenterFailed(_)))));
    }
}
