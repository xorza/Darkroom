use std::borrow::Cow;

use crate::gpu::slot::Slot;
use wgpu::{BufferAsyncError, util::DeviceExt};

use crate::common::error::{Error, Result};
use crate::gpu::Gpu;
use crate::image::{Image, ImageDesc};

/// Rounds a byte length up to a multiple of 4 — wgpu's `COPY_BUFFER_ALIGNMENT`,
/// i.e. a whole number of `u32` words. The only "padding" a packed GPU buffer
/// needs (1-3 trailing bytes on the whole buffer, never per row).
fn align_to_u32(bytes: usize) -> usize {
    (bytes + 3) & !3
}

/// Wrapper for read-only buffer access.
#[derive(Debug)]
pub(crate) struct ReadBuffer<'a>(&'a wgpu::Buffer);

impl ReadBuffer<'_> {
    /// Returns the entire buffer as a binding resource.
    pub(crate) fn as_entire_binding(&self) -> wgpu::BindingResource<'_> {
        self.0.as_entire_binding()
    }
}

/// Wrapper for writable buffer access.
#[derive(Debug)]
pub(crate) struct WriteBuffer<'a>(&'a wgpu::Buffer);

impl WriteBuffer<'_> {
    /// Returns the entire buffer as a binding resource.
    pub(crate) fn as_entire_binding(&self) -> wgpu::BindingResource<'_> {
        self.0.as_entire_binding()
    }

    /// Returns a reference to the underlying buffer for queue operations.
    pub(crate) fn buffer(&self) -> &wgpu::Buffer {
        self.0
    }
}

/// Image data stored on the GPU as a buffer.
///
/// The buffer holds the **tightly-packed** pixel bytes — no row padding (storage
/// buffers impose none; that's a texture rule). The only concession to wgpu is
/// that the buffer's *total* size is rounded up to a multiple of 4
/// (`COPY_BUFFER_ALIGNMENT`); those few trailing bytes are never read back. The
/// shaders address the buffer per-`u32`-word over this packed layout, so there
/// is no stride.
#[derive(Debug)]
pub struct GpuImage {
    buffer: wgpu::Buffer,
    pub(crate) desc: ImageDesc,
}

impl GpuImage {
    /// Creates a new GPU image from (packed) CPU image data.
    pub fn from_image(ctx: &Gpu, image: &Image) -> Self {
        let desc = image.desc();
        let packed = desc.size_in_bytes();
        let buf_size = align_to_u32(packed); // wgpu buffers: size multiple of 4
        let bytes: Cow<[u8]> = if packed == buf_size {
            Cow::Borrowed(image.bytes())
        } else {
            // Only the trailing 1-3 bytes of the whole buffer are padding.
            let mut buf = image.bytes().to_vec();
            buf.resize(buf_size, 0);
            Cow::Owned(buf)
        };

        let buffer = ctx
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("gpu_image_buffer"),
                contents: &bytes,
                usage: wgpu::BufferUsages::STORAGE
                    | wgpu::BufferUsages::COPY_SRC
                    | wgpu::BufferUsages::COPY_DST,
            });

        Self { buffer, desc }
    }

    /// Creates an empty GPU image with the given (packed) descriptor.
    pub fn new_empty(ctx: &Gpu, desc: ImageDesc) -> Self {
        let size = align_to_u32(desc.size_in_bytes()) as u64;

        let buffer = ctx.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("gpu_image_buffer"),
            size,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_SRC
                | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Self { buffer, desc }
    }

    /// Bytes occupied by the GPU buffer (packed size rounded up to a multiple of 4).
    pub(crate) fn buffer_size(&self) -> u64 {
        align_to_u32(self.desc.size_in_bytes()) as u64
    }

    /// Builds a packed CPU image from a freshly downloaded buffer, dropping the
    /// 1-3 trailing round-to-4 bytes.
    fn to_packed_image(&self, mut bytes: Vec<u8>) -> Result<Image> {
        bytes.truncate(self.desc.size_in_bytes());
        Image::new_with_data(self.desc, bytes)
    }

    /// Downloads GPU image data to CPU.
    pub fn to_image(&self, ctx: &Gpu) -> Result<Image> {
        let size = self.buffer_size();

        let staging_buffer = ctx.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("gpu_image_staging"),
            size,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let mut encoder = ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("gpu_image_download_encoder"),
            });

        encoder.copy_buffer_to_buffer(&self.buffer, 0, &staging_buffer, 0, size);
        ctx.queue.submit(std::iter::once(encoder.finish()));

        let buffer_slice = staging_buffer.slice(..);
        let slot = Slot::<std::result::Result<(), BufferAsyncError>>::default();
        buffer_slice.map_async(wgpu::MapMode::Read, {
            let slot = slot.clone();
            move |result| {
                slot.send(result);
            }
        });

        ctx.wait();

        // A concurrent poll on a shared device may claim this callback and fire
        // it just after our wait returns — block on the handoff, not the poll.
        slot.take_blocking()
            .unwrap()
            .map_err(|err| Error::Gpu(err.to_string()))?;

        let data = buffer_slice
            .get_mapped_range()
            .expect("map staging read range");
        let bytes = data.to_vec();
        drop(data);
        staging_buffer.unmap();

        // Drop the trailing round-to-4 bytes; the CPU image is exactly packed.
        self.to_packed_image(bytes)
    }

    /// Downloads GPU image data to CPU asynchronously.
    ///
    /// Note: This method requires the GPU device to be polled (via `ctx.wait()` or
    /// `ctx.wait_async()`) for the download to complete. The polling can happen
    /// from another thread - the callback will fire when polled, waking up this future.
    pub async fn to_image_async(&self, ctx: &Gpu) -> Result<Image> {
        let size = self.buffer_size();

        let staging_buffer = ctx.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("gpu_image_staging"),
            size,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let mut encoder = ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("gpu_image_download_encoder"),
            });

        encoder.copy_buffer_to_buffer(&self.buffer, 0, &staging_buffer, 0, size);
        ctx.queue.submit(std::iter::once(encoder.finish()));

        let slot = Slot::<std::result::Result<(), BufferAsyncError>>::default();
        let buffer_slice = staging_buffer.slice(..);
        buffer_slice.map_async(wgpu::MapMode::Read, {
            let slot = slot.clone();
            move |result| {
                slot.send(result);
            }
        });

        slot.take_async()
            .await
            .unwrap()
            .map_err(|err| Error::Gpu(err.to_string()))?;

        let data = buffer_slice
            .get_mapped_range()
            .expect("map staging read range");
        let bytes = data.to_vec();
        drop(data);
        staging_buffer.unmap();

        // Drop the trailing round-to-4 bytes; the CPU image is exactly packed.
        self.to_packed_image(bytes)
    }

    /// Creates a copy of this GPU image with a new buffer.
    pub fn clone_buffer(&self, ctx: &Gpu) -> Self {
        let size = self.buffer_size();

        let buffer = ctx.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("gpu_image_buffer"),
            size,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_SRC
                | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let mut encoder = ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("gpu_image_clone_encoder"),
            });

        encoder.copy_buffer_to_buffer(&self.buffer, 0, &buffer, 0, size);

        ctx.queue.submit(std::iter::once(encoder.finish()));

        Self {
            buffer,
            desc: self.desc,
        }
    }

    /// Returns a read-only buffer wrapper for binding in shaders.
    pub(crate) fn read_buffer(&self) -> ReadBuffer<'_> {
        ReadBuffer(&self.buffer)
    }

    /// Returns a writable buffer wrapper for binding in shaders.
    ///
    /// Note: `&mut self` is intentional to prevent accidental writes to non-mutable buffers.
    pub(crate) fn write_buffer(&mut self) -> WriteBuffer<'_> {
        WriteBuffer(&self.buffer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::internals::{gpu::test_gpu, load_lena_rgba_u8_61x38};

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
