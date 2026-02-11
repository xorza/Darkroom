use std::borrow::Cow;

use common::slot::Slot;
use wgpu::{BufferAsyncError, util::DeviceExt};

use crate::prelude::*;

/// Wrapper for read-only buffer access.
#[derive(Debug)]
pub struct ReadBuffer<'a>(pub(crate) &'a wgpu::Buffer);

impl ReadBuffer<'_> {
    /// Returns the entire buffer as a binding resource.
    pub fn as_entire_binding(&self) -> wgpu::BindingResource<'_> {
        self.0.as_entire_binding()
    }
}

/// Wrapper for writable buffer access.
#[derive(Debug)]
pub struct WriteBuffer<'a>(pub(crate) &'a wgpu::Buffer);

impl WriteBuffer<'_> {
    /// Returns the entire buffer as a binding resource.
    pub fn as_entire_binding(&self) -> wgpu::BindingResource<'_> {
        self.0.as_entire_binding()
    }

    /// Returns a reference to the underlying buffer for queue operations.
    pub fn buffer(&self) -> &wgpu::Buffer {
        self.0
    }
}

/// Image data stored on the GPU as a buffer.
#[derive(Debug)]
pub struct GpuImage {
    pub(crate) buffer: wgpu::Buffer,
    pub(crate) desc: ImageDesc,
}

impl GpuImage {
    /// Creates a new GPU image from CPU image data.
    pub fn from_image(ctx: &Gpu, image: &Image) -> Self {
        let desc = image.desc().with_aligned_stride();
        let bytes: Cow<[u8]> = if image.desc().stride == desc.stride {
            // Already aligned — zero-copy borrow
            Cow::Borrowed(image.bytes())
        } else {
            // Need to re-stride: copy pixel rows with new aligned stride (no full image clone)
            let src = image.bytes();
            let src_stride = image.desc().stride;
            let row_bytes = image.desc().row_bytes();
            let mut buf = vec![0u8; desc.size_in_bytes()];
            for y in 0..desc.height {
                buf[y * desc.stride..y * desc.stride + row_bytes]
                    .copy_from_slice(&src[y * src_stride..y * src_stride + row_bytes]);
            }
            Cow::Owned(buf)
        };

        let buffer = ctx
            .device()
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("gpu_image_buffer"),
                contents: &bytes,
                usage: wgpu::BufferUsages::STORAGE
                    | wgpu::BufferUsages::COPY_SRC
                    | wgpu::BufferUsages::COPY_DST,
            });

        Self { buffer, desc }
    }

    /// Creates an empty GPU image with the given descriptor.
    pub fn new_empty(ctx: &Gpu, desc: ImageDesc) -> Self {
        let desc = desc.with_aligned_stride();
        let size = desc.size_in_bytes() as u64;

        let buffer = ctx.device().create_buffer(&wgpu::BufferDescriptor {
            label: Some("gpu_image_buffer"),
            size,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_SRC
                | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Self { buffer, desc }
    }

    /// Downloads GPU image data to CPU.
    pub fn to_image(&self, ctx: &Gpu) -> Result<Image> {
        let size = self.desc().size_in_bytes() as u64;

        let staging_buffer = ctx.device().create_buffer(&wgpu::BufferDescriptor {
            label: Some("gpu_image_staging"),
            size,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let mut encoder = ctx
            .device()
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("gpu_image_download_encoder"),
            });

        encoder.copy_buffer_to_buffer(&self.buffer, 0, &staging_buffer, 0, size);
        ctx.queue().submit(std::iter::once(encoder.finish()));

        let buffer_slice = staging_buffer.slice(..);
        buffer_slice.map_async(wgpu::MapMode::Read, |_| {});

        ctx.wait();

        let data = buffer_slice.get_mapped_range();
        let bytes = data.to_vec();
        drop(data);
        staging_buffer.unmap();

        Image::new_with_data(self.desc, bytes)
    }

    /// Downloads GPU image data to CPU asynchronously.
    ///
    /// Note: This method requires the GPU device to be polled (via `ctx.wait()` or
    /// `ctx.wait_async()`) for the download to complete. The polling can happen
    /// from another thread - the callback will fire when polled, waking up this future.
    pub async fn to_image_async(&self, ctx: &Gpu) -> Result<Image> {
        let size = self.desc().size_in_bytes() as u64;

        let staging_buffer = ctx.device().create_buffer(&wgpu::BufferDescriptor {
            label: Some("gpu_image_staging"),
            size,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let mut encoder = ctx
            .device()
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("gpu_image_download_encoder"),
            });

        encoder.copy_buffer_to_buffer(&self.buffer, 0, &staging_buffer, 0, size);
        ctx.queue().submit(std::iter::once(encoder.finish()));

        let slot = Slot::<std::result::Result<(), BufferAsyncError>>::default();
        let buffer_slice = staging_buffer.slice(..);
        buffer_slice.map_async(wgpu::MapMode::Read, {
            let slot = slot.clone();
            move |result| {
                slot.send(result);
            }
        });

        slot.take_or_wait()
            .await
            .unwrap()
            .map_err(|err| Error::Gpu(err.to_string()))?;

        let data = buffer_slice.get_mapped_range();
        let bytes = data.to_vec();
        drop(data);
        staging_buffer.unmap();

        Image::new_with_data(self.desc, bytes)
    }

    /// Creates a copy of this GPU image with a new buffer.
    pub fn clone_buffer(&self, ctx: &Gpu) -> Self {
        let size = self.desc().size_in_bytes() as u64;

        let buffer = ctx.device().create_buffer(&wgpu::BufferDescriptor {
            label: Some("gpu_image_buffer"),
            size,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_SRC
                | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let mut encoder = ctx
            .device()
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("gpu_image_clone_encoder"),
            });

        encoder.copy_buffer_to_buffer(&self.buffer, 0, &buffer, 0, size);

        ctx.queue().submit(std::iter::once(encoder.finish()));

        Self {
            buffer,
            desc: *self.desc(),
        }
    }

    /// Returns the image descriptor.
    pub fn desc(&self) -> &ImageDesc {
        &self.desc
    }

    /// Returns a read-only buffer wrapper for binding in shaders.
    pub fn read_buffer(&self) -> ReadBuffer<'_> {
        ReadBuffer(&self.buffer)
    }

    /// Returns a writable buffer wrapper for binding in shaders.
    ///
    /// Note: `&mut self` is intentional to prevent accidental writes to non-mutable buffers.
    pub fn write_buffer(&mut self) -> WriteBuffer<'_> {
        WriteBuffer(&self.buffer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::test_utils::{load_lena_rgba_u8_61x38, test_gpu};

    #[test]
    fn test_to_image() {
        let Some(ctx) = test_gpu() else {
            return;
        };

        let image = load_lena_rgba_u8_61x38();
        let gpu_image = GpuImage::from_image(&ctx, &image);

        let result = gpu_image.to_image(&ctx).unwrap();

        assert_eq!(result.desc().width, 61);
        assert_eq!(result.desc().height, 38);
    }

    #[tokio::test]
    async fn test_to_image_async() {
        let Some(ctx) = test_gpu() else {
            return;
        };

        let image = load_lena_rgba_u8_61x38();
        let gpu_image = GpuImage::from_image(&ctx, &image);

        // Spawn a task to poll the GPU while we wait for the download
        let ctx_clone = ctx.clone();
        let poll_handle = tokio::spawn(async move {
            ctx_clone.wait_async().await;
        });

        let result = gpu_image.to_image_async(&ctx).await.unwrap();
        poll_handle.await.unwrap();

        assert_eq!(result.desc().width, 61);
        assert_eq!(result.desc().height, 38);
    }
}
