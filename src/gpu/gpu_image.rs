use std::{iter, result};

use tokio::sync::oneshot;

use crate::common::error::{Error, Result};
use crate::gpu::Gpu;
use crate::image::Image;
use crate::image::image_desc::ImageDesc;

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
    /// Uploads a CPU image: the bytes go straight into the mapped buffer, one copy.
    pub fn from_image(ctx: &Gpu, image: &Image) -> Result<Self> {
        let desc = image.desc();
        let buffer = Self::buffer(ctx, desc, true)?;
        {
            let mut mapped = buffer
                .slice(..)
                .get_mapped_range_mut()
                .expect("a buffer mapped at creation maps whole");
            let size = desc.size_in_bytes();
            let padding = mapped.len() - size;
            mapped.slice(..size).copy_from_slice(image.bytes());
            mapped.slice(size..).copy_from_slice(&[0; 3][..padding]);
        }
        buffer.unmap();
        Ok(Self { buffer, desc })
    }

    /// Creates an empty GPU image with the given (packed) descriptor.
    pub fn new_empty(ctx: &Gpu, desc: ImageDesc) -> Result<Self> {
        Ok(Self {
            buffer: Self::buffer(ctx, desc, false)?,
            desc,
        })
    }

    /// A storage buffer for `desc`, or an error when the device cannot bind one that large.
    fn buffer(ctx: &Gpu, desc: ImageDesc, mapped_at_creation: bool) -> Result<wgpu::Buffer> {
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

    /// Downloads GPU image data to CPU, blocking until the device has run it.
    pub fn to_image(&self, ctx: &Gpu) -> Result<Image> {
        let staging = self.start_download(ctx).wait(ctx)?;
        self.finish_download(&staging)
    }

    /// Downloads GPU image data to CPU asynchronously.
    ///
    /// The download completes when the device is polled; the polling can happen
    /// from another thread, and the callback then wakes this future.
    pub async fn to_image_async(&self, ctx: &Gpu) -> Result<Image> {
        let download = self.start_download(ctx);
        download
            .mapped
            .await
            .expect("the map callback runs once")
            .map_err(|error| Error::Gpu(error.to_string()))?;
        self.finish_download(&download.staging)
    }

    /// Copies the image into a staging buffer and asks for it mapped.
    fn start_download(&self, ctx: &Gpu) -> Download {
        Download::start(ctx, &self.buffer, buffer_size(self.desc))
    }

    /// Copies the mapped staging buffer into a CPU image, the round-to-4 padding dropped.
    fn finish_download(&self, staging: &wgpu::Buffer) -> Result<Image> {
        let mut image = Image::new_black(self.desc)?;
        {
            let mapped = staging
                .slice(..)
                .get_mapped_range()
                .expect("the map callback reported success");
            image
                .bytes_mut()
                .copy_from_slice(&mapped[..self.desc.size_in_bytes()]);
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

    /// The buffer, for binding as a shader's output. `&mut self` keeps a shader from
    /// writing an image the caller only lent.
    pub(crate) const fn write_buffer(&mut self) -> &wgpu::Buffer {
        &self.buffer
    }
}

/// A download in flight: the staging buffer and the signal that it is mapped.
#[derive(Debug)]
struct Download {
    staging: wgpu::Buffer,
    mapped: oneshot::Receiver<result::Result<(), wgpu::BufferAsyncError>>,
}

impl Download {
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
                label: Some("gpu_image_download_encoder"),
            });
        encoder.copy_buffer_to_buffer(buffer, 0, &staging, 0, size);
        ctx.queue.submit(iter::once(encoder.finish()));

        let (sender, mapped) = oneshot::channel();
        staging
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                let _ = sender.send(result);
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
const fn buffer_size(desc: ImageDesc) -> u64 {
    desc.size_in_bytes().next_multiple_of(4) as u64
}

#[cfg(test)]
pub(crate) mod internals {
    use crate::gpu::Gpu;
    use crate::gpu::gpu_image::Download;

    /// The first `count` words of a storage buffer, read back through a staging copy.
    pub(crate) fn download_words(gpu: &Gpu, buffer: &wgpu::Buffer, count: u32) -> Vec<u32> {
        let staging = Download::start(gpu, buffer, u64::from(count) * 4)
            .wait(gpu)
            .unwrap();
        let words = bytemuck::cast_slice(&staging.slice(..).get_mapped_range().unwrap()).to_vec();
        staging.unmap();
        words
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::thread;

    use crate::common::color_format::ALL_FORMATS;
    use crate::common::internals::create_test_image;
    use crate::common::internals::gpu::test_gpu;
    use crate::gpu::gpu_image::GpuImage;

    /// Every format, at a width whose packed size is not a whole number of words, comes
    /// back byte for byte — through both download paths and a buffer copy.
    #[test]
    fn upload_and_download_round_trip() {
        let Some(gpu) = test_gpu() else {
            return;
        };
        for format in ALL_FORMATS {
            let image = create_test_image(format, 7, 3, 11);
            let uploaded = GpuImage::from_image(&gpu, &image).unwrap();
            assert_eq!(
                uploaded.to_image(&gpu).unwrap().bytes(),
                image.bytes(),
                "{format}"
            );

            let copy = uploaded.clone_buffer(&gpu).unwrap();
            let done = AtomicBool::new(false);
            let downloaded = thread::scope(|scope| {
                scope.spawn(|| {
                    while !done.load(Ordering::Acquire) {
                        gpu.device.poll(wgpu::PollType::Poll).unwrap();
                    }
                });
                let downloaded = pollster::block_on(copy.to_image_async(&gpu)).unwrap();
                done.store(true, Ordering::Release);
                downloaded
            });
            assert_eq!(downloaded.bytes(), image.bytes(), "{format}, async");
        }
    }
}
