pub(crate) mod compute_kernel;
pub(crate) mod gpu_image;
pub(crate) mod packed_layout;

use std::sync::OnceLock;

use crate::error::GpuError;
use crate::gpu::compute_kernel::ComputeKernel;

/// A GPU device and its queue, and each op's compute kernel, compiled on its first use.
#[derive(Debug)]
pub struct Gpu {
    pub(crate) device: wgpu::Device,
    pub(crate) queue: wgpu::Queue,
    kernels: [OnceLock<ComputeKernel>; GpuOp::COUNT],
}

/// The ops with a GPU kernel: a closed set, each kernel cached in its slot of [`Gpu`].
#[derive(Debug, Clone, Copy)]
pub(crate) enum GpuOp {
    ContrastBrightness,
    Blend,
    AffineWarp,
}

impl GpuOp {
    const COUNT: usize = 3;
}

impl Gpu {
    /// Creates a new GPU context, initializing wgpu with default settings.
    ///
    /// The device gets wgpu's downlevel limits, which every adapter wgpu supports
    /// meets, raised to the adapter's own buffer limits: an astrophotography frame
    /// (24 MP of `RGBA_F32` is 384 MB) needs every byte of buffer the adapter allows,
    /// and asking for more than it has would fail the device outright.
    pub fn new() -> Result<Self, GpuError> {
        let instance = Self::instance();

        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: None,
            force_fallback_adapter: false,
            apply_limit_buckets: false,
        }))?;

        let supported = adapter.limits();
        let limits = wgpu::Limits {
            max_buffer_size: supported.max_buffer_size,
            max_storage_buffer_binding_size: supported.max_storage_buffer_binding_size,
            ..wgpu::Limits::downlevel_defaults()
        };

        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
                required_limits: limits,
                ..Default::default()
            }))?;

        Ok(Self {
            device,
            queue,
            kernels: Default::default(),
        })
    }

    /// `op`'s kernel, compiled by `compile` on this device the first time it is asked for.
    pub(crate) fn kernel(
        &self,
        op: GpuOp,
        compile: impl FnOnce(&Self) -> ComputeKernel,
    ) -> &ComputeKernel {
        self.kernels[op as usize].get_or_init(|| compile(self))
    }

    /// Polls the device, blocking until all pending operations complete.
    pub(crate) fn wait(&self) {
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("polling the device");
    }

    /// The instance every device opens through: every backend, narrowed by the environment.
    fn instance() -> wgpu::Instance {
        wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::all().with_env(),
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        })
    }

    /// The largest buffer this device binds as storage — the largest image it can process.
    pub(crate) fn max_buffer_byte_len(&self) -> u64 {
        let limits = self.device.limits();
        bindable_byte_len(
            limits.max_buffer_size,
            limits.max_storage_buffer_binding_size,
        )
    }
}

/// The largest buffer a device of these limits binds, at most `u32::MAX` bytes: the shaders index
/// a buffer's bytes in a `u32`, which wraps past it, and Metal reports buffers past 4 GiB.
const fn bindable_byte_len(max_buffer_size: u64, max_binding_size: u64) -> u64 {
    let device = if max_buffer_size < max_binding_size {
        max_buffer_size
    } else {
        max_binding_size
    };
    if device < u32::MAX as u64 {
        device
    } else {
        u32::MAX as u64
    }
}

#[cfg(feature = "internals")]
pub(crate) mod internals {
    use crate::gpu::Gpu;

    impl Gpu {
        /// Whether the host has an adapter [`Gpu::new`] could open, found by listing them
        /// without opening a device.
        pub(crate) fn adapter_exists() -> bool {
            let adapters = Self::instance().enumerate_adapters(wgpu::Backends::all());
            !pollster::block_on(adapters).is_empty()
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::gpu::bindable_byte_len;
    use crate::internals::gpu::test_gpu;

    /// The smaller of the two limits, held to the `u32` the shaders index bytes in: 128 MiB
    /// buffers bind as they are, and Metal's 8 GiB buffer behind a 4 GiB binding gives
    /// `u32::MAX`, one byte short of the binding.
    #[test]
    fn the_bindable_length_is_the_smaller_limit_within_u32() {
        for (buffer, binding, bindable) in [
            (1 << 30, 128 << 20, 128 << 20),
            (128 << 20, 1 << 30, 128 << 20),
            (8 << 30, 4 << 30, u64::from(u32::MAX)),
            (u64::MAX, u64::MAX, u64::from(u32::MAX)),
        ] {
            assert_eq!(
                bindable_byte_len(buffer, binding),
                bindable,
                "{buffer} {binding}"
            );
        }
    }

    /// A device exists when the host has a GPU, and it binds at least wgpu's downlevel
    /// 128 MiB storage buffer.
    #[test]
    fn a_device_binds_large_buffers() {
        let Some(gpu) = test_gpu() else {
            return;
        };
        assert!(
            gpu.max_buffer_byte_len() >= 128 << 20,
            "{}",
            gpu.max_buffer_byte_len()
        );
    }
}
