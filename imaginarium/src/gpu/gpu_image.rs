use std::borrow::Cow;

use tokio::sync::oneshot;
use wgpu::util::DeviceExt;

use crate::prelude::*;

/// A pending GPU image download operation.
///
/// Use `GpuImage::start_download` to create pending downloads, then call
/// `PendingDownload::wait_all` to wait for multiple downloads at once.
pub struct PendingDownload {
    staging_buffer: wgpu::Buffer,
    desc: ImageDesc,
    rx: oneshot::Receiver<std::result::Result<(), wgpu::BufferAsyncError>>,
}

impl PendingDownload {
    /// Waits for this download to complete and returns the image.
    /// Assumes the GPU has already been polled to completion.
    fn wait(mut self) -> Result<Image> {
        self.rx
            .try_recv()
            .expect("channel closed unexpectedly")
            .map_err(|e| Error::Gpu(format!("Failed to map buffer: {:?}", e)))?;

        let buffer_slice = self.staging_buffer.slice(..);
        let data = buffer_slice.get_mapped_range();
        let bytes = data.to_vec();

        drop(data);
        self.staging_buffer.unmap();

        Image::new_with_data(self.desc, bytes)
    }

    /// Waits for all pending downloads and returns the images.
    ///
    /// This is more efficient than downloading images one by one because
    /// it submits all copy commands before waiting.
    pub fn wait_all(
        downloads: impl IntoIterator<Item = PendingDownload>,
        ctx: &Gpu,
    ) -> Result<Vec<Image>> {
        let downloads: Vec<_> = downloads.into_iter().collect();
        if downloads.is_empty() {
            return Ok(Vec::new());
        }

        // Poll until all downloads complete
        ctx.wait();

        // Collect results
        downloads.into_iter().map(|d| d.wait()).collect()
    }

    /// Waits for all pending downloads asynchronously and returns the images.
    pub async fn wait_all_async(
        downloads: impl IntoIterator<Item = PendingDownload>,
        ctx: &Gpu,
    ) -> Result<Vec<Image>> {
        let downloads: Vec<_> = downloads.into_iter().collect();
        if downloads.is_empty() {
            return Ok(Vec::new());
        }

        // Poll non-blocking until all downloads complete
        ctx.wait_async().await;

        // Collect results
        downloads.into_iter().map(|d| d.wait()).collect()
    }
}

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
        let (bytes, desc): (Cow<[u8]>, ImageDesc) = if image.desc().is_aligned() {
            (Cow::Borrowed(image.bytes()), *image.desc())
        } else {
            let strided = image.clone().with_stride();
            (Cow::Owned(strided.bytes().to_vec()), *strided.desc())
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

    /// Starts a GPU image download operation without waiting for completion.
    ///
    /// Use `PendingDownload::wait_all` to wait for multiple downloads at once.
    ///
    /// # Example
    /// ```ignore
    /// let downloads: Vec<_> = images.iter()
    ///     .map(|img| img.start_download(ctx))
    ///     .collect();
    /// let cpu_images = PendingDownload::wait_all(downloads, ctx)?;
    /// ```
    pub fn start_download(&self, ctx: &Gpu) -> PendingDownload {
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
        let (tx, rx) = oneshot::channel();
        buffer_slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });

        PendingDownload {
            staging_buffer,
            desc: *self.desc(),
            rx,
        }
    }

    /// Downloads GPU image data to CPU.
    pub fn to_image(&self, ctx: &Gpu) -> Result<Image> {
        let download = self.start_download(ctx);
        let mut images = PendingDownload::wait_all(std::iter::once(download), ctx)?;
        Ok(images.pop().unwrap())
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
    use crate::ColorFormat;

    fn create_test_gpu() -> Option<Gpu> {
        Gpu::new().ok()
    }

    #[test]
    fn test_single_download() {
        let Some(ctx) = create_test_gpu() else {
            return;
        };

        let desc = ImageDesc::new(64, 64, ColorFormat::RGBA_U8);
        let image = Image::new_empty(desc).unwrap();
        let gpu_image = GpuImage::from_image(&ctx, &image);

        let download = gpu_image.start_download(&ctx);
        let images = PendingDownload::wait_all(std::iter::once(download), &ctx).unwrap();

        assert_eq!(images.len(), 1);
        assert_eq!(images[0].desc().width, 64);
        assert_eq!(images[0].desc().height, 64);
    }

    #[test]
    fn test_multiple_downloads() {
        let Some(ctx) = create_test_gpu() else {
            return;
        };

        let sizes = [(32, 32), (64, 64), (128, 128)];
        let gpu_images: Vec<_> = sizes
            .iter()
            .map(|(w, h)| {
                let desc = ImageDesc::new(*w, *h, ColorFormat::RGBA_U8);
                let image = Image::new_empty(desc).unwrap();
                GpuImage::from_image(&ctx, &image)
            })
            .collect();

        let downloads: Vec<_> = gpu_images
            .iter()
            .map(|img| img.start_download(&ctx))
            .collect();
        let images = PendingDownload::wait_all(downloads, &ctx).unwrap();

        assert_eq!(images.len(), 3);
        for (i, (w, h)) in sizes.iter().enumerate() {
            assert_eq!(images[i].desc().width, *w);
            assert_eq!(images[i].desc().height, *h);
        }
    }

    #[test]
    fn test_empty_downloads() {
        let Some(ctx) = create_test_gpu() else {
            return;
        };

        let images = PendingDownload::wait_all(std::iter::empty(), &ctx).unwrap();
        assert!(images.is_empty());
    }
}
