use crate::{
    hle::texture_request::{EncodedInput, EncodedTextureRequest},
    profiling::Recorder,
};

const SHADER: &str = include_str!("texture_decode.wgsl");
pub(crate) const PARAM_BYTES: u64 = 64;

pub(crate) struct DecodePipeline {
    pipeline: wgpu::ComputePipeline,
    layout: wgpu::BindGroupLayout,
}

pub(crate) struct DecodeStaging {
    pub(crate) input: wgpu::Buffer,
    params: wgpu::Buffer,
}

impl DecodeStaging {
    pub(crate) fn capacity(&self) -> u64 {
        self.input.size()
    }
}

pub(crate) struct DecodeJob {
    pub texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    pub staging: DecodeStaging,
    bind_group: wgpu::BindGroup,
    extent: [u32; 2],
}

pub(crate) fn required_input_bytes(request: &EncodedTextureRequest) -> u64 {
    match request.input() {
        EncodedInput::Tmem(_) => 4096,
    }
}

struct DecodeParams {
    words: [[u32; 4]; 4],
}

impl DecodeParams {
    fn new(request: &EncodedTextureRequest) -> Self {
        let recipe = request.recipe();
        Self {
            words: [
                [
                    recipe.allocation_extent()[0],
                    recipe.allocation_extent()[1],
                    recipe.logical[0],
                    recipe.logical[1],
                ],
                [
                    recipe.representation as u32,
                    recipe.fmt.into(),
                    recipe.siz.into(),
                    u32::from(recipe.base) * 8,
                ],
                [
                    u32::from(recipe.line) * 8,
                    recipe.palette.into(),
                    recipe.tlut.into(),
                    if recipe.palette_lanes {
                        recipe.output[1]
                    } else {
                        0
                    },
                ],
                [0; 4],
            ],
        }
    }

    fn bytes(&self) -> [u8; PARAM_BYTES as usize] {
        let mut result = [0; PARAM_BYTES as usize];
        for (word, bytes) in self
            .words
            .iter()
            .flatten()
            .zip(result.as_chunks_mut::<4>().0)
        {
            *bytes = word.to_le_bytes();
        }
        result
    }
}

impl DecodePipeline {
    pub(crate) fn new(device: &wgpu::Device) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("texture-decode"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("texture-decode"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(4096),
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(PARAM_BYTES),
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::StorageTexture {
                        access: wgpu::StorageTextureAccess::WriteOnly,
                        format: wgpu::TextureFormat::Rgba8Unorm,
                        view_dimension: wgpu::TextureViewDimension::D2,
                    },
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("texture-decode"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("texture-decode"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
        Self { pipeline, layout }
    }

    pub(crate) fn create_job(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        request: &EncodedTextureRequest,
        profiling: &Recorder,
        staging: Option<DecodeStaging>,
    ) -> DecodeJob {
        let EncodedInput::Tmem(bank) = request.input();
        let params = DecodeParams::new(request);
        let staging = staging
            .filter(|v| v.capacity() >= bank.len() as u64)
            .unwrap_or_else(|| {
                let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("texture-decode-input"),
                    size: bank.len() as u64,
                    usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
                let params = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("texture-decode-params"),
                    size: PARAM_BYTES,
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
                profiling.buffer("texture_decode_input", buffer.size(), 0);
                profiling.buffer("texture_decode_uniform", params.size(), 0);
                DecodeStaging {
                    input: buffer,
                    params,
                }
            });
        queue.write_buffer(&staging.input, 0, &bank[..]);
        queue.write_buffer(&staging.params, 0, &params.bytes());
        count_uploads(profiling, bank.len() as u64, 1);
        let (texture, view, extent) = self.output(device, &params, profiling);
        let bind_group = self.bind_group(
            device,
            staging.input.as_entire_buffer_binding(),
            staging.params.as_entire_buffer_binding(),
            &view,
        );
        DecodeJob {
            texture,
            view,
            staging,
            bind_group,
            extent,
        }
    }

    pub(crate) fn framebuffer_output(
        &self,
        device: &wgpu::Device,
        request: &EncodedTextureRequest,
        profiling: &Recorder,
        materialize: bool,
        scale: u32,
        metadata: u32,
    ) -> ([u8; PARAM_BYTES as usize], wgpu::TextureView, [u32; 2]) {
        let mut params = DecodeParams::new(request);
        params.words[3][0] = metadata;
        params.words[3][3] = scale;
        params.words[0][0] *= scale;
        params.words[0][1] *= scale;
        params.words[3][1] = 1024;
        params.words[3][2] = u32::from(materialize);
        let (_, view, extent) = self.output(device, &params, profiling);
        (params.bytes(), view, extent)
    }

    fn output(
        &self,
        device: &wgpu::Device,
        params: &DecodeParams,
        profiling: &Recorder,
    ) -> (wgpu::Texture, wgpu::TextureView, [u32; 2]) {
        let extent = [params.words[0][0], params.words[0][1]];
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("texture-decode-output"),
            size: wgpu::Extent3d {
                width: extent[0],
                height: extent[1],
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::STORAGE_BINDING
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        profiling.count("tmem.decode_output_allocations", 1);
        profiling.count(
            "tmem.decode_output_bytes",
            u64::from(extent[0]) * u64::from(extent[1]) * 4,
        );
        let view = texture.create_view(&Default::default());
        (texture, view, extent)
    }

    pub(crate) fn bind_group(
        &self,
        device: &wgpu::Device,
        input: wgpu::BufferBinding,
        params: wgpu::BufferBinding,
        view: &wgpu::TextureView,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("texture-decode"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::Buffer(input),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Buffer(params),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(view),
                },
            ],
        })
    }

    pub(crate) fn encode<'a>(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        jobs: impl IntoIterator<Item = (&'a wgpu::BindGroup, [u32; 2])>,
        profiling: &Recorder,
    ) {
        let mut jobs = jobs.into_iter().peekable();
        if jobs.peek().is_none() {
            return;
        }
        profiling.count("tmem.decode_compute_passes", 1);
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("texture-decode"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&self.pipeline);
        for (bind_group, extent) in jobs {
            pass.set_bind_group(0, bind_group, &[]);
            pass.dispatch_workgroups(extent[0].div_ceil(8), extent[1].div_ceil(8), 1);
            profiling.count("tmem.decode_dispatches", 1);
        }
    }
}

impl DecodeJob {
    pub(crate) fn dispatch(&self) -> (&wgpu::BindGroup, [u32; 2]) {
        (&self.bind_group, self.extent)
    }
}

pub(crate) fn count_uploads(profiling: &Recorder, input_bytes: u64, jobs: u64) {
    profiling.count("buffer.texture_decode_input.upload_bytes", input_bytes);
    profiling.count(
        "buffer.texture_decode_uniform.upload_bytes",
        PARAM_BYTES * jobs,
    );
    profiling.count("tmem.decode_input_upload_calls", 1);
    profiling.count("tmem.decode_uniform_upload_calls", 1);
    profiling.count("tmem.decode_input_upload_bytes", input_bytes);
    profiling.count("tmem.decode_uniform_upload_bytes", PARAM_BYTES * jobs);
}

#[cfg(test)]
#[path = "texture_decode_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "texture_decode_vectors.rs"]
mod vectors;

#[cfg(test)]
use crate::profiling;
#[cfg(all(
    test,
    not(target_arch = "wasm32"),
    not(all(feature = "capture", feature = "profiling"))
))]
use crate::tests::readback_export as export;

#[cfg(any(test, all(feature = "profiling", feature = "capture")))]
pub(crate) async fn readback_requests(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    requests: &[crate::profiling::Request],
) -> Result<Vec<Vec<u8>>, String> {
    let requests = requests
        .iter()
        .map(|v| v.encoded_request())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("{e:?}"))?;
    if requests.is_empty() {
        return Ok(Vec::new());
    }
    let profiling = Recorder::default();
    let pipeline = DecodePipeline::new(device);
    let jobs: Vec<_> = requests
        .iter()
        .map(|request| pipeline.create_job(device, queue, request, &profiling, None))
        .collect();
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("texture-decode-readback"),
    });
    pipeline.encode(
        &mut encoder,
        jobs.iter().map(DecodeJob::dispatch),
        &profiling,
    );
    let buffers: Vec<_> = jobs
        .iter()
        .map(|job| {
            let row_bytes = (job.extent[0] * 4).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
                * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
            let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("texture-decode-readback"),
                size: u64::from(row_bytes) * u64::from(job.extent[1]),
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            encoder.copy_texture_to_buffer(
                job.texture.as_image_copy(),
                wgpu::TexelCopyBufferInfo {
                    buffer: &buffer,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(row_bytes),
                        rows_per_image: None,
                    },
                },
                job.texture.size(),
            );
            (buffer, row_bytes)
        })
        .collect();
    queue.submit([encoder.finish()]);
    let ready: Vec<_> = buffers
        .iter()
        .map(|(buffer, _)| {
            #[cfg(not(target_arch = "wasm32"))]
            let (sender, receiver) = std::sync::mpsc::channel();
            #[cfg(target_arch = "wasm32")]
            let (sender, receiver) = futures_channel::oneshot::channel();
            buffer.map_async(wgpu::MapMode::Read, .., move |result| {
                let _ = sender.send(result);
            });
            receiver
        })
        .collect();
    #[cfg(not(target_arch = "wasm32"))]
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .map_err(|e| e.to_string())?;
    let mut result = Vec::with_capacity(jobs.len());
    for ((job, (buffer, row_bytes)), ready) in jobs.iter().zip(&buffers).zip(ready) {
        #[cfg(not(target_arch = "wasm32"))]
        let ready = ready.recv();
        #[cfg(target_arch = "wasm32")]
        let ready = ready.await;
        ready
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())?;
        let mapped = buffer.get_mapped_range(..);
        let mut pixels = Vec::with_capacity((job.extent[0] * job.extent[1] * 4) as usize);
        for row in 0..job.extent[1] as usize {
            let start = row * *row_bytes as usize;
            pixels.extend_from_slice(&mapped[start..start + job.extent[0] as usize * 4]);
        }
        drop(mapped);
        buffer.unmap();
        result.push(pixels);
    }
    Ok(result)
}

#[cfg(all(
    test,
    not(target_arch = "wasm32"),
    not(all(feature = "capture", feature = "profiling"))
))]
#[path = "texture_decode_gpu_tests.rs"]
mod gpu_tests;

#[cfg(all(
    test,
    not(target_arch = "wasm32"),
    not(all(feature = "capture", feature = "profiling"))
))]
#[path = "texture_decode_test_support.rs"]
mod test_support;
