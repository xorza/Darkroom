use std::{iter, result};

use tokio::sync::oneshot;

use crate::error::{Error, Result};
use crate::gpu::Gpu;
use crate::image::InterleavedImage;
use crate::image::image_desc::InterleavedDesc;

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
    pub(crate) desc: InterleavedDesc,
}

impl GpuImage {
    /// Uploads a CPU image: the bytes go straight into the mapped buffer, one copy.
    pub fn from_image(ctx: &Gpu, image: &InterleavedImage) -> Result<Self> {
        let desc = image.desc();
        let buffer = Self::buffer(ctx, desc, true)?;
        {
            let mut mapped = buffer
                .slice(..)
                .get_mapped_range_mut()
                .expect("a buffer mapped at creation maps whole");
            let size = desc.byte_len();
            let padding = mapped.len() - size;
            mapped.slice(..size).copy_from_slice(image.bytes());
            mapped.slice(size..).copy_from_slice(&[0; 3][..padding]);
        }
        buffer.unmap();
        Ok(Self { buffer, desc })
    }

    /// Creates an empty GPU image with the given (packed) descriptor.
    pub fn new_empty(ctx: &Gpu, desc: InterleavedDesc) -> Result<Self> {
        Ok(Self {
            buffer: Self::buffer(ctx, desc, false)?,
            desc,
        })
    }

    /// A storage buffer for `desc`, or an error when the device cannot bind one that large.
    fn buffer(ctx: &Gpu, desc: InterleavedDesc, mapped_at_creation: bool) -> Result<wgpu::Buffer> {
        let size = buffer_size(desc);
        if size > ctx.max_buffer_bytes() {
            return Err(Error::Gpu(format!(
                "a {desc} image needs a {size}-byte buffer; this device binds at most {}",
                ctx.max_buffer_bytes()
            )));
        }
        Ok(ctx.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("gpu_image_buffer"),
            size,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_SRC
                | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation,
        }))
    }

    /// Reads the image back to the CPU, blocking until the device has run it.
    pub fn to_image(&self, ctx: &Gpu) -> Result<InterleavedImage> {
        let staging = self.start_readback(ctx).wait(ctx)?;
        self.finish_readback(&staging)
    }

    /// Reads the image back to the CPU asynchronously.
    ///
    /// The readback completes when the device is polled; the polling can happen
    /// from another thread, and the callback then wakes this future.
    pub async fn to_image_async(&self, ctx: &Gpu) -> Result<InterleavedImage> {
        let readback = self.start_readback(ctx);
        readback
            .mapped
            .await
            .expect("the map callback runs once")
            .map_err(|error| Error::Gpu(error.to_string()))?;
        self.finish_readback(&readback.staging)
    }

    /// Copies the image into a staging buffer and asks for it mapped.
    fn start_readback(&self, ctx: &Gpu) -> Readback {
        Readback::start(ctx, &self.buffer, buffer_size(self.desc))
    }

    /// Copies the mapped staging buffer into a CPU image, the round-to-4 padding dropped.
    fn finish_readback(&self, staging: &wgpu::Buffer) -> Result<InterleavedImage> {
        let mut image = InterleavedImage::new_black(self.desc)?;
        {
            let mapped = staging
                .slice(..)
                .get_mapped_range()
                .expect("the map callback reported success");
            image
                .bytes_mut()
                .copy_from_slice(&mapped[..self.desc.byte_len()]);
        }
        staging.unmap();
        Ok(image)
    }

    /// Creates a copy of this GPU image with a new buffer.
    pub fn clone_buffer(&self, ctx: &Gpu) -> Result<Self> {
        let copy = Self::new_empty(ctx, self.desc)?;
        let mut encoder = ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("gpu_image_clone_encoder"),
            });
        encoder.copy_buffer_to_buffer(&self.buffer, 0, &copy.buffer, 0, buffer_size(self.desc));
        ctx.queue.submit(iter::once(encoder.finish()));
        Ok(copy)
    }

    /// The buffer, for binding as a shader's input.
    pub(crate) const fn read_buffer(&self) -> &wgpu::Buffer {
        &self.buffer
    }

    /// The buffer, for binding as a shader's output.
    #[expect(
        clippy::needless_pass_by_ref_mut,
        reason = "`&mut self` keeps a shader from writing an image the caller only lent"
    )]
    pub(crate) const fn write_buffer(&mut self) -> &wgpu::Buffer {
        &self.buffer
    }
}

/// A readback in flight: the staging buffer and the signal that it is mapped.
#[derive(Debug)]
struct Readback {
    staging: wgpu::Buffer,
    mapped: oneshot::Receiver<result::Result<(), wgpu::BufferAsyncError>>,
}

impl Readback {
    /// Copies the first `size` bytes of `buffer` into a staging buffer and asks for it mapped.
    fn start(ctx: &Gpu, buffer: &wgpu::Buffer, size: u64) -> Self {
        let staging = ctx.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("gpu_image_staging"),
            size,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut encoder = ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("gpu_image_readback_encoder"),
            });
        encoder.copy_buffer_to_buffer(buffer, 0, &staging, 0, size);
        ctx.queue.submit(iter::once(encoder.finish()));

        let (sender, mapped) = oneshot::channel();
        staging
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                sender.send(result).ok();
            });
        Self { staging, mapped }
    }

    /// The mapped staging buffer, blocking until the device has run the copy.
    fn wait(self, ctx: &Gpu) -> Result<wgpu::Buffer> {
        ctx.wait();
        // A concurrent poll on a shared device may claim this callback and fire
        // it just after our wait returns — block on the handoff, not the poll.
        pollster::block_on(self.mapped)
            .expect("the map callback runs once")
            .map_err(|error| Error::Gpu(error.to_string()))?;
        Ok(self.staging)
    }
}

/// The packed byte size rounded up to a multiple of 4 — wgpu's
/// `COPY_BUFFER_ALIGNMENT`, i.e. a whole number of `u32` words. The only
/// "padding" a packed GPU buffer needs (1-3 trailing bytes on the whole buffer,
/// never per row).
const fn buffer_size(desc: InterleavedDesc) -> u64 {
    desc.byte_len().next_multiple_of(4) as u64
}

#[cfg(test)]
pub(crate) mod internals {
    use crate::gpu::Gpu;
    use crate::gpu::gpu_image::Readback;

    /// The first `count` words of a storage buffer, read back through a staging copy.
    pub(crate) fn read_back_words(gpu: &Gpu, buffer: &wgpu::Buffer, count: u32) -> Vec<u32> {
        let staging = Readback::start(gpu, buffer, u64::from(count) * 4)
            .wait(gpu)
            .unwrap();
        let words = bytemuck::cast_slice(&staging.slice(..).get_mapped_range().unwrap()).to_vec();
        staging.unmap();
        words
    }
}

#[cfg(test)]
mod tests {
    use common::{Size2us, thread};
    use std::sync::atomic::{AtomicBool, Ordering};

    use crate::color::color_format::ColorFormat;
    use crate::gpu::gpu_image::GpuImage;
    use crate::internals::create_test_image;
    use crate::internals::gpu::test_gpu;

    /// Every format, at a width whose packed size is not a whole number of words, comes
    /// back byte for byte — through both readback paths and a buffer copy.
    #[test]
    fn upload_and_read_back_round_trip() {
        let Some(gpu) = test_gpu() else {
            return;
        };
        for format in ColorFormat::ALL {
            let image = create_test_image(format, Size2us::new(7, 3), 11);
            let uploaded = GpuImage::from_image(&gpu, &image).unwrap();
            assert_eq!(
                uploaded.to_image(&gpu).unwrap().bytes(),
                image.bytes(),
                "{format}"
            );

            let copy = uploaded.clone_buffer(&gpu).unwrap();
            let done = AtomicBool::new(false);
            let read_back = thread::scope(|scope| {
                scope.spawn("gpu-poll", || {
                    while !done.load(Ordering::Acquire) {
                        gpu.device.poll(wgpu::PollType::Poll).unwrap();
                    }
                });
                let read_back = pollster::block_on(copy.to_image_async(&gpu)).unwrap();
                done.store(true, Ordering::Release);
                read_back
            });
            assert_eq!(read_back.bytes(), image.bytes(), "{format}, async");
        }
    }
}
