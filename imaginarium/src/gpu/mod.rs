mod gpu_image;

use std::sync::Arc;

pub use self::gpu_image::{GpuImage, ReadBuffer, WriteBuffer};

use crate::common::{Error, Result};

/// GPU context holding wgpu device and queue for compute operations.
#[derive(Debug, Clone)]
pub struct Gpu {
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
}

impl Gpu {
    /// Creates a new GPU context, initializing wgpu with default settings.
    pub fn new() -> Result<Self> {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::all(),
            ..Default::default()
        });

        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: None,
            force_fallback_adapter: false,
        }))
        .map_err(|e| Error::Gpu(format!("failed to find suitable GPU adapter: {}", e)))?;

        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))
                .map_err(|e| Error::Gpu(format!("failed to create device: {}", e)))?;

        Ok(Self {
            device: Arc::new(device),
            queue: Arc::new(queue),
        })
    }

    /// Returns a reference to the wgpu device.
    pub fn device(&self) -> &wgpu::Device {
        &self.device
    }

    /// Returns a clone of the Arc to the wgpu device.
    pub fn device_arc(&self) -> Arc<wgpu::Device> {
        Arc::clone(&self.device)
    }

    /// Returns a reference to the wgpu queue.
    pub fn queue(&self) -> &wgpu::Queue {
        &self.queue
    }

    /// Returns a clone of the Arc to the wgpu queue.
    pub fn queue_arc(&self) -> Arc<wgpu::Queue> {
        Arc::clone(&self.queue)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gpu_context_creation() {
        let result = Gpu::new();
        if let Err(e) = &result {
            eprintln!(
                "GPU context creation failed (expected on headless systems): {}",
                e
            );
            return;
        }
        let _ctx = result.unwrap();
    }
}
