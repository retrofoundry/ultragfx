use std::sync::Arc;

#[derive(Default)]
pub(crate) struct SnapshotPool {
    textures: Vec<Arc<wgpu::Texture>>,
}

impl SnapshotPool {
    pub(crate) fn retain(
        &mut self,
        sources: impl Iterator<Item = (wgpu::Extent3d, wgpu::TextureFormat)>,
    ) {
        let keys: Vec<_> = sources.collect();
        self.textures
            .retain(|texture| keys.contains(&(texture.size(), texture.format())));
    }

    pub(crate) fn copy(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        source: &wgpu::Texture,
    ) -> Arc<wgpu::Texture> {
        let texture = self
            .textures
            .iter()
            .find(|texture| {
                Arc::strong_count(texture) == 1
                    && texture.size() == source.size()
                    && texture.format() == source.format()
            })
            .cloned()
            .unwrap_or_else(|| {
                let texture = Arc::new(device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("scanout-snapshot"),
                    size: source.size(),
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: source.format(),
                    usage: wgpu::TextureUsages::COPY_DST
                        | wgpu::TextureUsages::COPY_SRC
                        | wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                }));
                self.textures.push(texture.clone());
                texture
            });
        encoder.copy_texture_to_texture(
            source.as_image_copy(),
            texture.as_image_copy(),
            source.size(),
        );
        texture
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;

    #[test]
    fn pooled_textures_wait_for_both_cpu_release_and_gpu_completion() {
        let (device, queue, _) = super::super::headless_device();
        let source = device.create_texture(&super::super::framebuffers::color_descriptor(
            "source",
            (8, 8),
        ));
        let mut pool = SnapshotPool::default();
        let mut encoder = device.create_command_encoder(&Default::default());
        let first = pool.copy(&device, &mut encoder, &source);
        queue.submit([encoder.finish()]);
        let first_texture = (*first).clone();
        queue.on_submitted_work_done(move || drop(first));
        let mut encoder = device.create_command_encoder(&Default::default());
        let second = pool.copy(&device, &mut encoder, &source);
        assert_ne!(*second, first_texture);
        queue.submit([encoder.finish()]);
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        let mut encoder = device.create_command_encoder(&Default::default());
        let third = pool.copy(&device, &mut encoder, &source);
        assert_eq!(
            *third, first_texture,
            "GPU completion releases the texture for reuse"
        );
        let fourth = pool.copy(&device, &mut encoder, &source);
        assert_ne!(*fourth, first_texture, "CPU ownership still prevents reuse");
        drop(third);
        let reused = pool.copy(&device, &mut encoder, &source);
        assert_eq!(
            *reused, first_texture,
            "completed, unreferenced texture should be reused"
        );
        queue.submit([encoder.finish()]);
    }

    #[test]
    fn retired_extents_release_pool_ownership_without_invalidating_live_images() {
        let (device, queue, _) = super::super::headless_device();
        let source =
            device.create_texture(&super::super::framebuffers::color_descriptor("old", (8, 8)));
        let mut pool = SnapshotPool::default();
        let mut encoder = device.create_command_encoder(&Default::default());
        let image = pool.copy(&device, &mut encoder, &source);
        queue.submit([encoder.finish()]);
        let old = Arc::downgrade(&image);
        let gpu = image.clone();
        queue.on_submitted_work_done(move || drop(gpu));
        pool.retain(std::iter::empty());
        assert!(old.upgrade().is_some());
        drop(image);
        assert!(old.upgrade().is_some(), "GPU lease released too early");
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        assert!(
            old.upgrade().is_none(),
            "retired extent still owned by the pool"
        );
    }

    #[test]
    fn reset_releases_snapshot_pool_ownership() {
        let (device, queue, _) = super::super::headless_device();
        let mut renderer =
            super::super::SceneRenderer::new(&device, wgpu::TextureFormat::Rgba8Unorm, 8, 8, false);
        let source = device.create_texture(&super::super::framebuffers::color_descriptor(
            "source",
            (8, 8),
        ));
        let mut encoder = device.create_command_encoder(&Default::default());
        let image = renderer.snapshots.copy(&device, &mut encoder, &source);
        queue.submit([encoder.finish()]);
        let old = Arc::downgrade(&image);
        drop(image);
        renderer.reset();
        assert!(
            old.upgrade().is_none(),
            "reset retained the old snapshot pool"
        );
    }

    #[test]
    fn retirement_preserves_all_current_extents_and_formats() {
        let (device, queue, _) = super::super::headless_device();
        let sources: Vec<_> = [
            (8, wgpu::TextureFormat::Rgba8Unorm),
            (16, wgpu::TextureFormat::Rgba8Unorm),
            (8, wgpu::TextureFormat::Bgra8Unorm),
        ]
        .into_iter()
        .map(|(size, format)| {
            let mut descriptor =
                super::super::framebuffers::color_descriptor("source", (size, size));
            descriptor.format = format;
            device.create_texture(&descriptor)
        })
        .collect();
        let mut pool = SnapshotPool::default();
        let mut encoder = device.create_command_encoder(&Default::default());
        let images: Vec<_> = sources
            .iter()
            .map(|source| pool.copy(&device, &mut encoder, source))
            .collect();
        queue.submit([encoder.finish()]);
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        let textures: Vec<_> = images.iter().map(|image| (**image).clone()).collect();
        drop(images);
        pool.retain(
            sources
                .iter()
                .map(|source| (source.size(), source.format())),
        );
        let mut encoder = device.create_command_encoder(&Default::default());
        for (source, expected) in sources.iter().zip(textures) {
            assert_eq!(*pool.copy(&device, &mut encoder, source), expected);
        }
        queue.submit([encoder.finish()]);
    }
}
