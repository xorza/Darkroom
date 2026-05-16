pub(crate) mod gpu_image;
mod slot;

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
    pub fn new() -> Result<Self> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::all(),
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });

        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: None,
            force_fallback_adapter: false,
        }))
        .map_err(|e| Error::Gpu(format!("failed to find suitable GPU adapter: {e}")))?;

        // Request higher limits for large image processing (astrophotography images can be 24+ megapixels)
        // Support up to 1GB buffers
        let limits = wgpu::Limits {
            max_buffer_size: 1024 * 1024 * 1024,
            max_storage_buffer_binding_size: 1024 * 1024 * 1024,
            ..Default::default()
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
    pub fn wait(&self) {
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .unwrap();
    }

    /// Polls the device asynchronously until all pending operations complete.
    pub async fn wait_async(&self) {
        loop {
            if self
                .device
                .poll(wgpu::PollType::Poll)
                .unwrap()
                .is_queue_empty()
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gpu_context_creation() {
        let result = Gpu::new();
        if let Err(e) = &result {
            eprintln!("GPU context creation failed (expected on headless systems): {e}");
            return;
        }
        let _ctx = result.unwrap();
    }
}
