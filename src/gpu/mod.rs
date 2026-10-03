pub(crate) mod compute_kernel;
pub(crate) mod context;
pub(crate) mod gpu_image;
pub(crate) mod packed_layout;

use std::sync::Arc;

use crate::common::error::{Error, Result};

/// GPU context holding wgpu device and queue for compute operations.
#[derive(Debug, Clone)]
pub struct Gpu {
    pub(crate) device: Arc<wgpu::Device>,
    pub(crate) queue: Arc<wgpu::Queue>,
}

impl Gpu {
    /// Creates a new GPU context, initializing wgpu with default settings.
    ///
    /// The device gets wgpu's downlevel limits, which every adapter wgpu supports
    /// meets, raised to the adapter's own buffer limits: an astrophotography frame
    /// (24 MP of `RGBA_F32` is 384 MB) needs every byte of buffer the adapter allows,
    /// and asking for more than it has would fail the device outright.
    pub fn new() -> Result<Self> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::all().with_env(),
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });

        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: None,
            force_fallback_adapter: false,
            apply_limit_buckets: false,
        }))
        .map_err(|e| Error::Gpu(format!("failed to find suitable GPU adapter: {e}")))?;

        let supported = adapter.limits();
        let limits = wgpu::Limits {
            max_buffer_size: supported.max_buffer_size,
            max_storage_buffer_binding_size: supported.max_storage_buffer_binding_size,
            ..wgpu::Limits::downlevel_defaults()
        };

        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            required_limits: limits,
            ..Default::default()
        }))
        .map_err(|e| Error::Gpu(format!("failed to create device: {e}")))?;

        Ok(Self {
            device: Arc::new(device),
            queue: Arc::new(queue),
        })
    }

    /// Polls the device, blocking until all pending operations complete.
    pub(crate) fn wait(&self) {
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("polling the device");
    }

    /// The largest buffer this device binds as storage — the largest image it can process.
    pub(crate) fn max_buffer_bytes(&self) -> u64 {
        let limits = self.device.limits();
        limits
            .max_buffer_size
            .min(limits.max_storage_buffer_binding_size)
    }
}

#[cfg(test)]
mod tests {
    use crate::common::internals::gpu::test_gpu;

    /// A device exists when the host has a GPU, and it binds at least wgpu's downlevel
    /// 128 MiB storage buffer.
    #[test]
    fn a_device_binds_large_buffers() {
        let Some(gpu) = test_gpu() else {
            return;
        };
        assert!(
            gpu.max_buffer_bytes() >= 128 << 20,
            "{}",
            gpu.max_buffer_bytes()
        );
    }
}
