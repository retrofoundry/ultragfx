pub struct TestQueue {
    queue: wgpu::Queue,
    device: wgpu::Device,
}

impl TestQueue {
    pub fn new(device: &wgpu::Device, queue: wgpu::Queue) -> Self {
        Self {
            queue,
            device: device.clone(),
        }
    }
}

impl std::ops::Deref for TestQueue {
    type Target = wgpu::Queue;

    fn deref(&self) -> &Self::Target {
        &self.queue
    }
}

impl Drop for TestQueue {
    fn drop(&mut self) {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.device
                .poll(wgpu::PollType::wait_indefinitely())
                .unwrap();
        }));
        // Poll runs callbacks too; their panics must not replace an assertion already unwinding.
        if !std::thread::panicking() {
            if let Err(panic) = result {
                std::panic::resume_unwind(panic);
            }
        }
    }
}
