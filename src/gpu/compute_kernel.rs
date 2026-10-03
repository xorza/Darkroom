use std::iter;

use wgpu::util::DeviceExt;

use crate::gpu::Gpu;

/// The helpers every shader is prefixed with.
const PACKED_WGSL: &str = include_str!("packed.wgsl");

/// Threads per workgroup, as `WORKGROUP_SIZE` in `packed.wgsl`.
const WORKGROUP_SIZE: u32 = 256;

/// A compute shader over one uniform block and storage buffers: binding 0 is the
/// uniform, bindings 1.. the buffers, read-only except the last, which it writes.
///
/// Each invocation owns one element of work by its linear index, and dispatch lays the
/// workgroups out in 2-D, so a job of any size stays within the device's per-dimension
/// workgroup limit (65 535 by default — 24 MP of `RGB_F32` is 72 M words).
#[derive(Debug)]
pub(crate) struct ComputeKernel {
    pipeline: wgpu::ComputePipeline,
    layout: wgpu::BindGroupLayout,
    label: &'static str,
}

impl ComputeKernel {
    /// Compiles `shader` behind the shared helpers, with `buffers` storage bindings.
    pub(crate) fn new(gpu: &Gpu, label: &'static str, shader: &str, buffers: u32) -> Self {
        let device = &gpu.device;
        let source = format!("{PACKED_WGSL}\n{shader}");
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some(label),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });

        let entry = |binding: u32, ty: wgpu::BufferBindingType| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let entries: Vec<_> = iter::once(entry(0, wgpu::BufferBindingType::Uniform))
            .chain((1..=buffers).map(|binding| {
                entry(
                    binding,
                    wgpu::BufferBindingType::Storage {
                        read_only: binding < buffers,
                    },
                )
            }))
            .collect();
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some(label),
            entries: &entries,
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some(label),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some(label),
            layout: Some(&pipeline_layout),
            module: &module,
            entry_point: Some("main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            cache: None,
        });

        Self {
            pipeline,
            layout,
            label,
        }
    }

    /// Runs `invocations` invocations over `buffers` — in binding order, the written one
    /// last — with `params` as the uniform block. `clear` zeroes the written buffer first,
    /// for a shader that ORs sub-word elements into it.
    pub(crate) fn run(
        &self,
        gpu: &Gpu,
        params: &[u8],
        buffers: &[&wgpu::Buffer],
        invocations: u32,
        clear: bool,
    ) {
        let row_length = gpu.device.limits().max_compute_workgroups_per_dimension;
        self.dispatch(gpu, params, buffers, invocations, clear, row_length);
    }

    /// [`Self::run`] with rows of at most `row_length` workgroups.
    fn dispatch(
        &self,
        gpu: &Gpu,
        params: &[u8],
        buffers: &[&wgpu::Buffer],
        invocations: u32,
        clear: bool,
        row_length: u32,
    ) {
        let device = &gpu.device;
        let params = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(self.label),
            contents: params,
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let entries: Vec<_> = iter::once(&params)
            .chain(buffers.iter().copied())
            .zip(0..)
            .map(|(buffer, binding)| wgpu::BindGroupEntry {
                binding,
                resource: buffer.as_entire_binding(),
            })
            .collect();
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(self.label),
            layout: &self.layout,
            entries: &entries,
        });

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some(self.label),
        });
        if clear {
            let output = buffers.last().expect("a kernel writes one buffer");
            encoder.clear_buffer(output, 0, None);
        }
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some(self.label),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            let [x, y] = workgroups(invocations, row_length);
            let limit = device.limits().max_compute_workgroups_per_dimension;
            assert!(
                x <= limit && y <= limit,
                "{invocations} invocations exceed the dispatch limit"
            );
            pass.dispatch_workgroups(x, y, 1);
        }
        gpu.queue.submit(iter::once(encoder.finish()));
    }
}

/// Workgroup counts covering `invocations`, as rows of at most `row_length` groups.
fn workgroups(invocations: u32, row_length: u32) -> [u32; 2] {
    let groups = invocations.div_ceil(WORKGROUP_SIZE).max(1);
    let x = groups.min(row_length);
    [x, groups.div_ceil(x)]
}

#[cfg(test)]
mod tests {
    use wgpu::util::DeviceExt;

    use crate::common::internals::gpu::test_gpu;
    use crate::gpu::compute_kernel::{ComputeKernel, workgroups};
    use crate::gpu::gpu_image;

    /// 24 MP of `RGB_F32` is 72 M words: 281 250 groups of 256, which one row of at most
    /// 65 535 cannot hold — five rows can (5 · 65 535 = 327 675).
    #[test]
    fn large_jobs_spread_over_rows() {
        assert_eq!(workgroups(1, 65535), [1, 1]);
        assert_eq!(workgroups(256, 65535), [1, 1]);
        assert_eq!(workgroups(257, 65535), [2, 1]);
        assert_eq!(workgroups(72_000_000, 65535), [65535, 5]);
        assert_eq!(workgroups(u32::MAX, 65535), [65535, 257]);
    }

    /// Every invocation writes its own index, so the output is `0..n` exactly when the 2-D
    /// layout and `linear_index` agree — run here in rows of one, two and three workgroups,
    /// with `n` leaving the last row part-empty.
    #[test]
    fn rows_of_workgroups_cover_each_index_once() {
        let Some(gpu) = test_gpu() else {
            return;
        };
        let shader = "
            struct Params { count: u32 }
            @group(0) @binding(0) var<uniform> params: Params;
            @group(0) @binding(1) var<storage, read_write> output: array<u32>;
            @compute @workgroup_size(256)
            fn main(
                @builtin(global_invocation_id) gid: vec3<u32>,
                @builtin(num_workgroups) groups: vec3<u32>,
            ) {
                let i = linear_index(gid, groups);
                if i < params.count {
                    output[i] = i;
                }
            }
        ";
        let kernel = ComputeKernel::new(&gpu, "index", shader, 1);
        let count = 7 * 256 + 5;
        for row_length in [1, 2, 3] {
            let output = gpu
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: None,
                    contents: bytemuck::cast_slice(&vec![u32::MAX; count as usize]),
                    usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
                });
            let params = [count, 0, 0, 0];
            kernel.dispatch(
                &gpu,
                bytemuck::cast_slice(&params),
                &[&output],
                count,
                false,
                row_length,
            );
            let image = gpu_image::internals::download_words(&gpu, &output, count);
            assert_eq!(
                image,
                (0..count).collect::<Vec<_>>(),
                "rows of {row_length}"
            );
        }
    }
}
