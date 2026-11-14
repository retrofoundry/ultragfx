use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Weak},
};

use super::{
    inputs::TextureInputs,
    texture_resources::{upload_rgba, Image, RgbaImage, TextureResources},
    TileSamplingArray, LOD_BINDING_BASE,
};
use crate::hle::texture_request::TextureSource;

// FIFO caches at most 120 KiB of GPU bytes; the 18-task OoT Forest corpus uses 66 arrays (63,360 bytes).
const SAMPLING_CACHE_ENTRIES: usize = 128;
type SamplingBytes = [u8; std::mem::size_of::<TileSamplingArray>()];

#[derive(Default)]
pub(super) struct SamplingCache {
    entries: HashMap<SamplingBytes, SamplingEntry>,
    fifo: VecDeque<SamplingBytes>,
}

struct SamplingEntry {
    buffer: Arc<wgpu::Buffer>,
    binding: Weak<MaterialBinding>,
}

impl SamplingCache {
    pub fn previous(
        &self,
        mat: &TextureInputs,
        resources: &TextureResources,
        profiling: &crate::profiling::Recorder,
    ) -> Option<Arc<MaterialBinding>> {
        let bytes: SamplingBytes = bytemuck::cast(mat.sampling);
        self.entries
            .get(&bytes)?
            .binding
            .upgrade()
            .filter(|binding| binding.may_reuse(mat, resources, profiling))
    }

    fn get(
        &mut self,
        device: &wgpu::Device,
        tiles: &TileSamplingArray,
        profiling: &crate::profiling::Recorder,
    ) -> &mut SamplingEntry {
        let bytes: SamplingBytes = bytemuck::cast(*tiles);
        if !self.entries.contains_key(&bytes) {
            if self.entries.len() == SAMPLING_CACHE_ENTRIES {
                self.entries.remove(&self.fifo.pop_front().unwrap());
            }
            self.fifo.push_back(bytes);
            self.entries.insert(
                bytes,
                SamplingEntry {
                    buffer: Arc::new(super::sampling_buffer_profiled(device, tiles, profiling)),
                    binding: Weak::new(),
                },
            );
        }
        self.entries.get_mut(&bytes).unwrap()
    }
}

#[derive(Clone)]
enum BindingImage {
    Framebuffer,
    Encoded(Image),
    Rgba(Arc<RgbaImage>),
    Replacement(Arc<RgbaImage>),
    Dummy,
}

enum PreparedImage {
    Framebuffer(wgpu::TextureView),
    Encoded(Image),
    Rgba(Arc<RgbaImage>),
    Replacement(Arc<RgbaImage>),
    Dummy,
}

impl PreparedImage {
    fn matches(&self, old: &BindingImage) -> bool {
        match (self, old) {
            (Self::Encoded(image), BindingImage::Encoded(old)) => Arc::ptr_eq(image, old),
            (Self::Replacement(image), BindingImage::Replacement(old)) => Arc::ptr_eq(image, old),
            (Self::Rgba(image), BindingImage::Rgba(old)) => Arc::ptr_eq(image, old),
            (Self::Dummy, BindingImage::Dummy) => true,
            _ => false,
        }
    }

    fn view<'a>(&'a self, dummy: &'a wgpu::TextureView) -> &'a wgpu::TextureView {
        match self {
            Self::Encoded(image) => &image.value.view,
            Self::Rgba(image) | Self::Replacement(image) => &image.view,
            Self::Dummy => dummy,
            Self::Framebuffer(view) => view,
        }
    }

    fn binding(self) -> BindingImage {
        match self {
            Self::Encoded(image) => BindingImage::Encoded(image),
            Self::Rgba(image) => BindingImage::Rgba(image),
            Self::Replacement(image) => BindingImage::Replacement(image),
            Self::Dummy => BindingImage::Dummy,
            Self::Framebuffer(_) => BindingImage::Framebuffer,
        }
    }
}

pub(super) struct MaterialBinding {
    sampling: TileSamplingArray,
    sampling_buffer: Arc<wgpu::Buffer>,
    wrap: [u8; 2],
    tex1_wrap: [usize; 2],
    images: Vec<BindingImage>,
    pub bind_group: wgpu::BindGroup,
    pub high_resolution_mask: u32,
}

impl MaterialBinding {
    pub fn may_reuse(
        &self,
        mat: &TextureInputs,
        resources: &TextureResources,
        profiling: &crate::profiling::Recorder,
    ) -> bool {
        if self.sampling != mat.sampling || self.wrap != [mat.wrap_s, mat.wrap_t] {
            return false;
        }
        if resources.replacements.active.is_some()
            && !self
                .images
                .iter()
                .any(|image| matches!(image, BindingImage::Framebuffer))
        {
            return true;
        }
        roles(mat)
            .into_iter()
            .zip(&self.images)
            .enumerate()
            .all(|(index, (source, image))| match (source, image) {
                (Some(binding), BindingImage::Encoded(image)) => match &binding.source {
                    TextureSource::Encoded(request) => {
                        (image.admitted || resources.cache.is_pending(image))
                            && request.key_profiled(profiling) == image.key
                    }
                    _ => false,
                },
                (Some(binding), BindingImage::Rgba(image)) => match &binding.source {
                    TextureSource::Rgba(bytes) => {
                        image.source == *bytes
                            && image.extent
                                == if index == 0 {
                                    mat.allocation_extent
                                } else {
                                    binding.sampling.allocation_extent()
                                }
                    }
                    _ => false,
                },
                (None, BindingImage::Dummy) => true,
                _ => false,
            })
    }

    pub fn rgba_bytes(&self) -> u64 {
        self.images
            .iter()
            .map(|image| match image {
                BindingImage::Rgba(image) => {
                    u64::from(image.extent[0]) * u64::from(image.extent[1]) * 4
                }
                _ => 0,
            })
            .sum()
    }
}

pub(super) struct BindingBuilder<'a> {
    pub device: &'a wgpu::Device,
    pub queue: &'a wgpu::Queue,
    pub layout: &'a wgpu::BindGroupLayout,
    pub samplers: &'a [[wgpu::Sampler; 3]; 3],
    pub dummy: &'a wgpu::TextureView,
    pub copies: &'a mut super::framebuffers::copies::Copies,
    pub resources: &'a mut TextureResources,
    pub sampling_cache: &'a mut SamplingCache,
    pub profiling: &'a crate::profiling::Recorder,
    pub presentation: bool,
}

impl BindingBuilder<'_> {
    pub fn build(
        &mut self,
        mat: &TextureInputs,
        old: Option<Arc<MaterialBinding>>,
    ) -> Arc<MaterialBinding> {
        let _span = self.profiling.span("resources");
        let roles = roles(mat);
        let mut mapped_sampling = None;
        let mut high_resolution_mask = 0;
        let mut images = Vec::with_capacity(roles.len());
        for (index, role) in roles.into_iter().enumerate() {
            let Some(binding) = role else {
                images.push(PreparedImage::Dummy);
                continue;
            };
            let name = [
                if mat.mip_levels.is_empty() {
                    "texture0"
                } else {
                    "lod0"
                },
                "texture1",
                "detail",
                "lod1",
                "lod2",
                "lod3",
                "lod4",
                "lod5",
                "lod6",
                "lod7",
            ][index];
            let image = match &binding.source {
                TextureSource::Framebuffer(request) => {
                    let (view, scale) = self.copies.texture(
                        self.device,
                        request,
                        self.profiling,
                        self.presentation,
                    );
                    if scale > 1 {
                        high_resolution_mask |= 1 << index;
                        mapped_sampling.get_or_insert(mat.sampling)[index].image[3] = scale;
                    }
                    PreparedImage::Framebuffer(view)
                }
                TextureSource::Encoded(request) => {
                    let replacement = self.resources.resolve_replacement(
                        self.device,
                        self.queue,
                        request,
                        &binding.sampling,
                        self.profiling,
                    );
                    if let Some((image, scale)) = replacement {
                        high_resolution_mask |= u32::from(scale > 1) << index;
                        mapped_sampling.get_or_insert(mat.sampling)[index].image[3] = scale;
                        PreparedImage::Replacement(image)
                    } else {
                        PreparedImage::Encoded(self.resources.resolve(
                            self.device,
                            self.queue,
                            request,
                            name,
                            self.profiling,
                        ))
                    }
                }
                TextureSource::Rgba(bytes) => {
                    let extent = if index == 0 {
                        mat.allocation_extent
                    } else {
                        binding.sampling.allocation_extent()
                    };
                    let reused = old.as_ref().and_then(|old| match &old.images[index] {
                        BindingImage::Rgba(image)
                            if image.extent == extent && image.source == *bytes =>
                        {
                            Some(image.clone())
                        }
                        _ => None,
                    });
                    PreparedImage::Rgba(reused.unwrap_or_else(|| {
                        upload_rgba(
                            self.device,
                            self.queue,
                            &binding.source,
                            extent,
                            name,
                            self.profiling,
                        )
                    }))
                }
            };
            images.push(image);
        }
        let wrap = [mat.wrap_s, mat.wrap_t];
        let tex1_wrap = mat.tex1.as_ref().map_or([2; 2], |input| {
            std::array::from_fn(|axis| (input.sampling.modes[axis] as usize).min(2))
        });
        let sampling = self.sampling_cache.get(
            self.device,
            mapped_sampling.as_ref().unwrap_or(&mat.sampling),
            self.profiling,
        );
        for old in old.into_iter().chain(sampling.binding.upgrade()) {
            if old.sampling == mat.sampling
                && Arc::ptr_eq(&old.sampling_buffer, &sampling.buffer)
                && old.wrap == wrap
                && old.tex1_wrap == tex1_wrap
                && old.high_resolution_mask == high_resolution_mask
                && images
                    .iter()
                    .zip(&old.images)
                    .all(|(image, old)| image.matches(old))
            {
                sampling.binding = Arc::downgrade(&old);
                return old;
            }
        }
        let samp1 = &self.samplers[tex1_wrap[0]][tex1_wrap[1]];
        let mut entries = vec![super::sampling_entry(&sampling.buffer)];
        for (index, image) in images.iter().enumerate() {
            let binding = if index < 3 {
                index as u32 * 2
            } else {
                LOD_BINDING_BASE + index as u32 - 3
            };
            entries.push(wgpu::BindGroupEntry {
                binding,
                resource: wgpu::BindingResource::TextureView(image.view(self.dummy)),
            });
        }
        for (binding, sampler) in [
            (
                1,
                &self.samplers[(wrap[0] as usize).min(2)][(wrap[1] as usize).min(2)],
            ),
            (3, samp1),
            (5, &self.samplers[2][2]),
        ] {
            entries.push(wgpu::BindGroupEntry {
                binding,
                resource: wgpu::BindingResource::Sampler(sampler),
            });
        }
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("n64-bg"),
            layout: self.layout,
            entries: &entries,
        });
        self.profiling.count("tmem.sampling_bindings_created", 1);
        let binding = Arc::new(MaterialBinding {
            sampling: mat.sampling,
            sampling_buffer: sampling.buffer.clone(),
            wrap,
            tex1_wrap,
            images: images.into_iter().map(PreparedImage::binding).collect(),
            bind_group,
            high_resolution_mask,
        });
        sampling.binding = Arc::downgrade(&binding);
        binding
    }
}

fn roles(mat: &TextureInputs) -> [Option<&crate::hle::texture_request::TextureBindingInput>; 10] {
    if mat.framebuffer_only {
        return [None; 10];
    }
    let count = super::uploaded_level_count(mat.num_levels);
    std::array::from_fn(|index| match index {
        0 => Some(&mat.texture),
        1 => mat.tex1.as_ref(),
        2 => mat.detail_tex.as_ref(),
        _ if (index - 2) < count as usize => mat.mip_levels.get(index - 2),
        _ => None,
    })
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use crate::{
        profiling::{Mode, Recorder},
        render::{headless_device, SceneRenderer},
    };

    #[test]
    fn obsolete_bindings_do_not_prevent_texture_eviction() {
        use crate::{
            hle::{
                rdp::{Rdp, TileDescriptor},
                texture_request::prepare,
            },
            render::texture_residency::{Limits, Residency},
        };

        let (device, queue, dual) = headless_device();
        let mut renderer = SceneRenderer::new(&device, wgpu::TextureFormat::Rgba8Unorm, 1, 1, dual);
        renderer.texture_resources.cache = Residency::new(Limits {
            entries: 1,
            gpu_bytes: 4,
            cpu_bytes: 8192,
        });
        let recorder = Recorder::new(Mode::Counters);
        renderer.profiling = recorder.clone();
        let tile = TileDescriptor {
            fmt: 4,
            siz: 1,
            width: 1,
            height: 1,
            ..Default::default()
        };
        let mut input = TextureInputs::from(&crate::render::tests::test_material());
        let mut rdp = Rdp::default();
        rdp.tmem_bank.write_tile(&[1; 4096], 0, 0, 1, 512, 4096, 1);
        input.texture = prepare(&rdp, &tile, 0, &recorder).unwrap();
        input.sampling[0] = input.texture.sampling;
        renderer.upload_materials(&device, &queue, std::slice::from_ref(&input));
        let mut encoder = device.create_command_encoder(&Default::default());
        renderer.texture_resources.encode(&mut encoder, &recorder);
        queue.submit([encoder.finish()]);
        renderer.texture_resources.submitted(&queue, &recorder);
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        renderer.texture_resources.collect(&recorder);
        recorder.drain();

        rdp.tmem_bank.write_tile(&[2; 4096], 0, 0, 1, 512, 4096, 1);
        input.texture = prepare(&rdp, &tile, 0, &recorder).unwrap();
        renderer.upload_materials(&device, &queue, &[input]);
        let snapshot = recorder.drain();
        assert_eq!(snapshot.counters.get("tmem.evictions"), Some(&1));
        assert_eq!(snapshot.counters.get("tmem.bypasses"), None);
    }

    #[test]
    fn sampling_cache_outlives_material_bindings() {
        let (device, queue, dual) = headless_device();
        let mut renderer = SceneRenderer::new(&device, wgpu::TextureFormat::Rgba8Unorm, 1, 1, dual);
        let recorder = Recorder::new(Mode::Counters);
        renderer.profiling = recorder.clone();
        let mut input = TextureInputs::from(&crate::render::tests::test_material());
        input.framebuffer_only = true;
        for _ in 0..3 {
            renderer.material_bindings.clear();
            renderer.upload_materials(&device, &queue, std::slice::from_ref(&input));
        }
        assert_eq!(recorder.drain().counters["buffer.uniforms.creations"], 1);
    }

    #[test]
    fn cached_bindings_validate_images_and_both_samplers() {
        let (device, queue, dual) = headless_device();
        let mut renderer = SceneRenderer::new(&device, wgpu::TextureFormat::Rgba8Unorm, 1, 1, dual);
        let recorder = Recorder::new(Mode::Counters);
        renderer.profiling = recorder.clone();
        let mut input = TextureInputs::from(&crate::render::tests::test_material());
        input.framebuffer_only = true;
        renderer.upload_materials(&device, &queue, std::slice::from_ref(&input));
        let original = renderer.material_bindings[0].clone();
        input.wrap_s = 1;
        renderer.upload_materials(&device, &queue, std::slice::from_ref(&input));
        assert_ne!(
            original.bind_group,
            renderer.material_bindings[0].bind_group
        );
        let wrapped = renderer.material_bindings[0].clone();
        input.tex1 = Some(input.texture.clone());
        input.tex1.as_mut().unwrap().sampling.modes[0] = 1;
        renderer.upload_materials(&device, &queue, std::slice::from_ref(&input));
        assert_ne!(wrapped.bind_group, renderer.material_bindings[0].bind_group);
        let dummy = renderer.material_bindings[0].clone();
        input.framebuffer_only = false;
        renderer.upload_materials(&device, &queue, std::slice::from_ref(&input));
        assert_ne!(dummy.bind_group, renderer.material_bindings[0].bind_group);
        let snapshot = recorder.drain();
        assert_eq!(snapshot.counters["buffer.uniforms.creations"], 1);
        assert_eq!(snapshot.counters["tmem.sampling_bindings_created"], 4);
    }

    #[test]
    fn sampling_cache_keys_final_bytes_including_mapped_scale() {
        let (device, _, _) = headless_device();
        let recorder = Recorder::new(Mode::Counters);
        let mut cache = SamplingCache::default();
        let mut tiles = TileSamplingArray::default();
        let native = cache.get(&device, &tiles, &recorder).buffer.clone();
        tiles[0].image[3] = 2;
        let mapped = cache.get(&device, &tiles, &recorder).buffer.clone();
        assert!(!Arc::ptr_eq(&native, &mapped));
        assert!(Arc::ptr_eq(
            &mapped,
            &cache.get(&device, &tiles, &recorder).buffer
        ));
        tiles[9].palette[3] = 1;
        assert!(!Arc::ptr_eq(
            &mapped,
            &cache.get(&device, &tiles, &recorder).buffer
        ));
        assert_eq!(recorder.drain().counters["buffer.uniforms.creations"], 3);
    }

    #[test]
    fn sampling_cache_evicts_in_insertion_order_at_the_bound() {
        let (device, _, _) = headless_device();
        let recorder = Recorder::new(Mode::Counters);
        let mut cache = SamplingCache::default();
        let mut tiles = TileSamplingArray::default();
        let first = cache.get(&device, &tiles, &recorder).buffer.clone();
        for index in 1..SAMPLING_CACHE_ENTRIES {
            tiles[0].bounds[0] = index as i32;
            cache.get(&device, &tiles, &recorder);
        }
        tiles[0].bounds[0] = 0;
        assert!(Arc::ptr_eq(
            &first,
            &cache.get(&device, &tiles, &recorder).buffer
        ));
        tiles[0].bounds[0] = SAMPLING_CACHE_ENTRIES as i32;
        cache.get(&device, &tiles, &recorder);
        assert_eq!(cache.entries.len(), SAMPLING_CACHE_ENTRIES);
        assert_eq!(cache.fifo.len(), SAMPLING_CACHE_ENTRIES);
        tiles[0].bounds[0] = 0;
        assert!(!Arc::ptr_eq(
            &first,
            &cache.get(&device, &tiles, &recorder).buffer
        ));
        assert_eq!(cache.entries.len(), SAMPLING_CACHE_ENTRIES);
        assert_eq!(recorder.drain().counters["buffer.uniforms.creations"], 130);
    }

    #[test]
    fn retained_binding_owns_sampling_buffer_after_eviction() {
        let (device, queue, dual) = headless_device();
        let mut renderer = SceneRenderer::new(&device, wgpu::TextureFormat::Rgba8Unorm, 1, 1, dual);
        let mut input = TextureInputs::from(&crate::render::tests::test_material());
        input.framebuffer_only = true;
        renderer.upload_materials(&device, &queue, &[input.clone()]);
        let retained = renderer.material_bindings[0].clone();
        let buffer = Arc::downgrade(&retained.sampling_buffer);
        let binding = Arc::downgrade(&retained);
        for index in 1..=SAMPLING_CACHE_ENTRIES {
            input.sampling[0].bounds[0] = index as i32;
            renderer.upload_materials(&device, &queue, &[input.clone()]);
        }
        assert_eq!(
            renderer.sampling_cache.entries.len(),
            SAMPLING_CACHE_ENTRIES
        );
        assert!(buffer.upgrade().is_some());
        drop(retained);
        assert!(binding.upgrade().is_none());
        assert!(buffer.upgrade().is_none());
    }
}
